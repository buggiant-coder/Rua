use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};

use agent_types::{AgentEvent, ChatRequest, ChatResponse};

use crate::error::LlmError;

pub type EventStream = Pin<Box<dyn Stream<Item = Result<AgentEvent, LlmError>> + Send>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,

    #[serde(default)]
    pub name: String,

    pub context_window: Option<u64>,

    pub max_output_tokens: Option<u64>,

    #[serde(default)]
    pub supports_multimodal: Option<bool>,

    #[serde(default)]
    pub supports_image: Option<bool>,

    #[serde(default)]
    pub supports_video: Option<bool>,

    pub probe_source: Option<String>,

    #[serde(default)]
    pub generate_kwargs: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub id: String,

    pub name: String,

    pub provider_type: Option<String>,

    #[serde(default)]
    pub is_local: bool,

    #[serde(default)]
    pub support_model_discovery: bool,

    #[serde(default)]
    pub support_connection_check: bool,
}

#[derive(Debug, Clone)]
pub enum ConnectionStatus {
    Connected,

    Failed(String),
}

impl ConnectionStatus {
    pub fn is_connected(&self) -> bool {
        matches!(self, ConnectionStatus::Connected)
    }
}

#[async_trait]
pub trait ChatModel: Send + Sync {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError>;

    fn chat_stream(&self, request: ChatRequest) -> EventStream;

    /// Bound inactivity within each provider attempt, not retry backoff or queueing.
    /// Wrappers that retry, fall back, or delay invocation must forward this method
    /// to each inner attempt instead of timing their combined stream. Zero disables it.
    fn chat_stream_with_idle_timeout(
        &self,
        request: ChatRequest,
        idle_timeout: Duration,
    ) -> EventStream {
        let mut stream = self.chat_stream(request);
        if idle_timeout.is_zero() {
            return stream;
        }
        let provider = self.model_info().id.clone();
        Box::pin(async_stream::stream! {
            loop {
                let item = match tokio::time::timeout(idle_timeout, stream.next()).await {
                    Ok(Some(item)) => item,
                    Ok(None) => break,
                    Err(_) => {
                        drop(stream);
                        yield Err(LlmError::Timeout { provider, timeout: idle_timeout });
                        break;
                    }
                };
                let terminal = item.as_ref().map_or(true, AgentEvent::is_terminal);
                yield item;
                if terminal {
                    break;
                }
            }
        })
    }

    fn model_info(&self) -> &ModelInfo;
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn info(&self) -> &ProviderInfo;

    fn supports_model(&self, model: &str) -> bool;

    fn create_chat_model(&self, model: &str) -> Result<Box<dyn ChatModel>, LlmError>;

    async fn fetch_models(&self) -> Result<Vec<ModelInfo>, LlmError>;

    async fn check_connection(&self) -> Result<ConnectionStatus, LlmError>;
}
