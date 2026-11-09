use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::StreamExt;

use crate::core::content::AgentChannelRequest;
use crate::core::content::OutgoingContentPart;
use crate::core::renderer::MessageRenderer;
use rua_core::{Result, RuntimeEvent};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamType {
    Message,

    Reasoning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformStreamMode {
    PlaceholderEdit,

    CardUpdate,

    ReplyStream,

    FinalOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformStreamPolicy {
    pub mode: PlatformStreamMode,
    pub min_update_interval: Duration,
    pub max_update_chars: usize,
}

impl PlatformStreamPolicy {
    pub fn placeholder_edit(min_update_interval: Duration, max_update_chars: usize) -> Self {
        Self {
            mode: PlatformStreamMode::PlaceholderEdit,
            min_update_interval,
            max_update_chars,
        }
    }

    pub fn card_update(min_update_interval: Duration, max_update_chars: usize) -> Self {
        Self {
            mode: PlatformStreamMode::CardUpdate,
            min_update_interval,
            max_update_chars,
        }
    }

    pub fn reply_stream(min_update_interval: Duration, max_update_chars: usize) -> Self {
        Self {
            mode: PlatformStreamMode::ReplyStream,
            min_update_interval,
            max_update_chars,
        }
    }

    pub fn final_only() -> Self {
        Self {
            mode: PlatformStreamMode::FinalOnly,
            min_update_interval: Duration::ZERO,
            max_update_chars: usize::MAX,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformStreamAction {
    Create {
        mode: PlatformStreamMode,
        content: String,
    },
    Update {
        mode: PlatformStreamMode,
        content: String,
        final_update: bool,
    },
    SendFinal {
        content: String,
    },
    Skip,
}

#[derive(Debug, Default, Clone)]
pub struct PlatformStreamState {
    started: bool,
    fallback_final_only: bool,
    last_update_at: Option<Duration>,
}

impl PlatformStreamState {
    pub fn on_delta(
        &mut self,
        policy: &PlatformStreamPolicy,
        now: Duration,
        accumulated: &str,
    ) -> PlatformStreamAction {
        if policy.mode == PlatformStreamMode::FinalOnly {
            return PlatformStreamAction::Skip;
        }
        if accumulated.chars().count() > policy.max_update_chars {
            self.fallback_final_only = true;
            return PlatformStreamAction::Skip;
        }
        if !self.started {
            self.started = true;
            self.last_update_at = Some(now);
            return PlatformStreamAction::Create {
                mode: policy.mode,
                content: accumulated.to_string(),
            };
        }
        if self
            .last_update_at
            .is_some_and(|last| now.saturating_sub(last) < policy.min_update_interval)
        {
            return PlatformStreamAction::Skip;
        }
        self.last_update_at = Some(now);
        PlatformStreamAction::Update {
            mode: policy.mode,
            content: accumulated.to_string(),
            final_update: false,
        }
    }

    pub fn on_end(
        &mut self,
        policy: &PlatformStreamPolicy,
        accumulated: &str,
    ) -> PlatformStreamAction {
        if accumulated.is_empty() {
            return PlatformStreamAction::Skip;
        }
        if policy.mode == PlatformStreamMode::FinalOnly || self.fallback_final_only {
            return PlatformStreamAction::SendFinal {
                content: accumulated.to_string(),
            };
        }
        if self.started {
            return PlatformStreamAction::Update {
                mode: policy.mode,
                content: accumulated.to_string(),
                final_update: true,
            };
        }
        PlatformStreamAction::SendFinal {
            content: accumulated.to_string(),
        }
    }
}

#[async_trait]
pub trait StreamHandler: Send + Sync {
    async fn on_start(&self, stream_type: StreamType) -> Result<()>;

    async fn on_delta(&self, stream_type: StreamType, accumulated: &str) -> Result<()>;

    async fn on_end(&self, stream_type: StreamType, accumulated: &str) -> Result<()>;
}

#[async_trait]
pub trait ProcessingIndicator: Send + Sync {
    async fn start(&self, req: &AgentChannelRequest) -> Result<()>;
    async fn stop(&self, req: &AgentChannelRequest) -> Result<()>;
}

pub struct StreamCollectOutcome {
    pub parts: Vec<OutgoingContentPart>,

    pub message_streamed: bool,
}

fn report_hook(res: Result<()>) {
    if let Err(e) = res {
        tracing::warn!(error = %e, "stream handler hook failed");
    }
}

pub async fn collect_reply_with_handler(
    mut stream: BoxStream<'static, RuntimeEvent>,
    renderer: &MessageRenderer,
    handler: Option<&dyn StreamHandler>,
) -> StreamCollectOutcome {
    let mut result: Vec<OutgoingContentPart> = Vec::new();

    let mut message_acc = String::new();
    let mut message_started = false;
    let mut reasoning_acc = String::new();
    let mut reasoning_started = false;
    let mut message_streamed = false;

    while let Some(event) = stream.next().await {
        if let Some(handler) = handler {
            match &event {
                RuntimeEvent::TextDelta { delta, .. } => {
                    if !message_started {
                        message_started = true;
                        message_streamed = true;
                        report_hook(handler.on_start(StreamType::Message).await);
                    }
                    message_acc.push_str(delta);
                    report_hook(handler.on_delta(StreamType::Message, &message_acc).await);
                }
                RuntimeEvent::ThinkingDelta { delta, .. } if renderer.options.show_thinking => {
                    if !reasoning_started {
                        reasoning_started = true;
                        report_hook(handler.on_start(StreamType::Reasoning).await);
                    }
                    reasoning_acc.push_str(delta);
                    report_hook(
                        handler
                            .on_delta(StreamType::Reasoning, &reasoning_acc)
                            .await,
                    );
                }
                RuntimeEvent::End { .. } => {
                    if reasoning_started {
                        report_hook(handler.on_end(StreamType::Reasoning, &reasoning_acc).await);
                        reasoning_started = false;
                    }
                    if message_started {
                        report_hook(handler.on_end(StreamType::Message, &message_acc).await);
                        message_started = false;
                    }
                }
                _ => {}
            }
        }

        let parts = renderer.render_event(&event);
        for part in parts {
            match (&part, result.last_mut()) {
                (
                    OutgoingContentPart::Text { text },
                    Some(OutgoingContentPart::Text { text: prev }),
                ) => {
                    prev.push_str(text);
                }
                (
                    OutgoingContentPart::Markdown { text },
                    Some(OutgoingContentPart::Markdown { text: prev }),
                ) => {
                    prev.push_str(text);
                }
                _ => {
                    result.push(part);
                }
            }
        }
    }

    if let Some(handler) = handler {
        if reasoning_started {
            report_hook(handler.on_end(StreamType::Reasoning, &reasoning_acc).await);
        }
        if message_started {
            report_hook(handler.on_end(StreamType::Message, &message_acc).await);
        }
    }

    StreamCollectOutcome {
        parts: result,
        message_streamed,
    }
}

pub async fn collect_reply(
    stream: BoxStream<'static, RuntimeEvent>,
    renderer: &MessageRenderer,
) -> Vec<OutgoingContentPart> {
    collect_reply_with_handler(stream, renderer, None)
        .await
        .parts
}
