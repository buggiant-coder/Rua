use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use futures::Stream;
use tracing::{debug, info, warn};

use agent_types::{AgentEvent, ChatRequest, ChatResponse};
use llm_multiplex::{ChatModel, EventStream, LlmError, ModelInfo};

use crate::retry::is_content_event;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum FallbackMode {
    #[default]
    Disabled,

    SameProviderAlternateModel,

    AlternateProvider,
}

#[derive(Debug, Clone)]
pub struct FallbackPolicy {
    pub mode: FallbackMode,

    pub max_attempts: u32,

    pub fallback_providers: Vec<String>,

    pub fallback_on_auth: bool,

    pub fallback_on_content_filter: bool,
}

impl Default for FallbackPolicy {
    fn default() -> Self {
        Self {
            mode: FallbackMode::Disabled,
            max_attempts: 2,
            fallback_providers: Vec::new(),
            fallback_on_auth: false,
            fallback_on_content_filter: false,
        }
    }
}

impl FallbackPolicy {
    pub fn should_fallback(&self, error: &llm_multiplex::LlmError) -> bool {
        if self.mode == FallbackMode::Disabled {
            return false;
        }

        match error {
            llm_multiplex::LlmError::Auth { .. } => {
                debug!("auth error: fallback_on_auth={}", self.fallback_on_auth);
                self.fallback_on_auth
            }

            llm_multiplex::LlmError::InvalidResponse { message, .. }
                if message.contains("content_filter") || message.contains("safety") =>
            {
                debug!(
                    "content filter: fallback_on_content_filter={}",
                    self.fallback_on_content_filter
                );
                self.fallback_on_content_filter
            }

            llm_multiplex::LlmError::Cancelled => false,

            llm_multiplex::LlmError::Config { .. } => false,

            e if e.is_retryable() => true,

            llm_multiplex::LlmError::ModelNotFound { .. } => true,

            _ => false,
        }
    }

    pub fn next_fallback(&self, attempt: u32, exclude: &[&str]) -> Option<&str> {
        if attempt >= self.max_attempts {
            return None;
        }
        self.fallback_providers
            .iter()
            .find(|p| !exclude.contains(&p.as_str()))
            .map(|s| s.as_str())
    }
}

pub trait FallbackResolver: Send + Sync {
    fn resolve(&self, target: &str, model_hint: &str) -> Result<Box<dyn ChatModel>, LlmError>;
}

pub struct FallbackChatModel {
    primary: Arc<dyn ChatModel>,
    policy: FallbackPolicy,
    resolver: Arc<dyn FallbackResolver>,
}

impl FallbackChatModel {
    pub fn new(
        primary: Box<dyn ChatModel>,
        policy: FallbackPolicy,
        resolver: Arc<dyn FallbackResolver>,
    ) -> Self {
        Self {
            primary: Arc::from(primary),
            policy,
            resolver,
        }
    }
}

#[async_trait]
impl ChatModel for FallbackChatModel {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        match self.primary.chat(request.clone()).await {
            Ok(resp) => return Ok(resp),
            Err(e) => {
                if !self.policy.should_fallback(&e) {
                    return Err(e);
                }

                let model_hint = self.primary.model_info().id.clone();
                let mut excluded: Vec<String> = Vec::new();
                let mut last_error = e;

                for attempt in 0..self.policy.max_attempts {
                    let refs: Vec<&str> = excluded.iter().map(|s| s.as_str()).collect();
                    match self.policy.next_fallback(attempt, &refs) {
                        None => break,
                        Some(target) => {
                            let target_owned = target.to_string();
                            excluded.push(target_owned.clone());

                            match self.resolver.resolve(&target_owned, &model_hint) {
                                Err(e) => {
                                    warn!(target = %target_owned, error = %e, "fallback resolve failed");
                                    last_error = e;
                                }
                                Ok(model) => {
                                    info!(target = %target_owned, "falling back to alternate model");
                                    match model.chat(request.clone()).await {
                                        Ok(resp) => return Ok(resp),
                                        Err(e) => {
                                            warn!(target = %target_owned, error = %e, "fallback model failed");
                                            last_error = e;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                Err(last_error)
            }
        }
    }

    fn chat_stream(&self, request: ChatRequest) -> EventStream {
        self.chat_stream_with_idle_timeout(request, Duration::ZERO)
    }

    fn chat_stream_with_idle_timeout(
        &self,
        request: ChatRequest,
        idle_timeout: Duration,
    ) -> EventStream {
        let model_hint = self.primary.model_info().id.clone();
        let stream = self
            .primary
            .chat_stream_with_idle_timeout(request.clone(), idle_timeout);
        Box::pin(FallbackStream {
            state: FallbackStreamState::Streaming {
                stream,
                request: Box::new(request),
                policy: self.policy.clone(),
                resolver: Arc::clone(&self.resolver),
                model_hint,
                content_emitted: false,
                attempt: 0,
                excluded: Vec::new(),
            },
            saw_start: false,
            idle_timeout,
        })
    }

    fn model_info(&self) -> &ModelInfo {
        self.primary.model_info()
    }
}

enum FallbackStreamState {
    Streaming {
        stream: EventStream,
        request: Box<ChatRequest>,
        policy: FallbackPolicy,
        resolver: Arc<dyn FallbackResolver>,
        model_hint: String,
        content_emitted: bool,
        attempt: u32,
        excluded: Vec<String>,
    },
    Done,
}

struct FallbackStream {
    state: FallbackStreamState,
    saw_start: bool,
    idle_timeout: Duration,
}

impl Stream for FallbackStream {
    type Item = Result<AgentEvent, LlmError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            match &mut this.state {
                FallbackStreamState::Streaming {
                    stream,
                    content_emitted,
                    ..
                } => match stream.as_mut().poll_next(cx) {
                    Poll::Ready(Some(Ok(event))) => {
                        if event.is_start() {
                            if this.saw_start {
                                continue;
                            }
                            this.saw_start = true;
                        }
                        if is_content_event(&event) {
                            *content_emitted = true;
                        }
                        return Poll::Ready(Some(Ok(event)));
                    }
                    Poll::Ready(Some(Err(e))) => {
                        let old = std::mem::replace(&mut this.state, FallbackStreamState::Done);
                        if let FallbackStreamState::Streaming {
                            content_emitted,
                            request,
                            policy,
                            resolver,
                            model_hint,
                            attempt,
                            mut excluded,
                            ..
                        } = old
                        {
                            if content_emitted || !policy.should_fallback(&e) {
                                return Poll::Ready(Some(Err(e)));
                            }

                            let mut current_attempt = attempt;
                            loop {
                                let refs: Vec<&str> = excluded.iter().map(|s| s.as_str()).collect();
                                match policy.next_fallback(current_attempt, &refs) {
                                    None => return Poll::Ready(Some(Err(e))),
                                    Some(target) => {
                                        let target_owned = target.to_string();
                                        excluded.push(target_owned.clone());
                                        current_attempt += 1;

                                        match resolver.resolve(&target_owned, &model_hint) {
                                            Err(re) => {
                                                warn!(
                                                    target = %target_owned,
                                                    error = %re,
                                                    "fallback resolve failed, trying next"
                                                );
                                                continue;
                                            }
                                            Ok(model) => {
                                                info!(
                                                    target = %target_owned,
                                                    "stream falling back to alternate model"
                                                );
                                                let new_stream = model
                                                    .chat_stream_with_idle_timeout(
                                                        (*request).clone(),
                                                        this.idle_timeout,
                                                    );
                                                this.state = FallbackStreamState::Streaming {
                                                    stream: new_stream,
                                                    request,
                                                    policy,
                                                    resolver,
                                                    model_hint,
                                                    content_emitted: false,
                                                    attempt: current_attempt,
                                                    excluded,
                                                };
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                            continue;
                        }
                    }
                    Poll::Ready(None) => {
                        this.state = FallbackStreamState::Done;
                        return Poll::Ready(None);
                    }
                    Poll::Pending => return Poll::Pending,
                },
                FallbackStreamState::Done => return Poll::Ready(None),
            }
        }
    }
}
