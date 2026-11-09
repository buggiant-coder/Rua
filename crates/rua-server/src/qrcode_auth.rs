use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::errors::ApiError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QRCodeResult {
    pub scan_url: String,
    pub poll_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PollResult {
    pub status: String,
    #[serde(default)]
    pub credentials: serde_json::Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QRCodeAuthParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
}

#[async_trait]
pub trait QRCodeAuthHandler: Send + Sync {
    async fn fetch_qrcode(&self, agent_id: &str) -> Result<QRCodeResult, ApiError>;
    async fn poll_status(&self, token: &str, agent_id: &str) -> Result<PollResult, ApiError>;

    async fn fetch_qrcode_with_params(
        &self,
        agent_id: &str,
        _params: &QRCodeAuthParams,
    ) -> Result<QRCodeResult, ApiError> {
        self.fetch_qrcode(agent_id).await
    }

    async fn poll_status_with_params(
        &self,
        token: &str,
        agent_id: &str,
        _params: &QRCodeAuthParams,
    ) -> Result<PollResult, ApiError> {
        self.poll_status(token, agent_id).await
    }
}

pub struct QRCodeHandlerRegistry {
    handlers: RwLock<HashMap<String, Arc<dyn QRCodeAuthHandler>>>,
}

impl QRCodeHandlerRegistry {
    pub fn new() -> Self {
        Self {
            handlers: RwLock::new(HashMap::new()),
        }
    }

    pub async fn register(&self, channel: &str, handler: Arc<dyn QRCodeAuthHandler>) {
        self.handlers
            .write()
            .await
            .insert(channel.to_string(), handler);
    }

    pub fn register_blocking(&self, channel: &str, handler: Arc<dyn QRCodeAuthHandler>) {
        match self.handlers.try_write() {
            Ok(mut guard) => {
                guard.insert(channel.to_string(), handler);
            }
            Err(_) => {
                tracing::error!(
                    channel,
                    "qrcode registry contended at register_blocking; handler not registered"
                );
            }
        }
    }

    pub async fn get(&self, channel: &str) -> Option<Arc<dyn QRCodeAuthHandler>> {
        self.handlers.read().await.get(channel).cloned()
    }
}

impl Default for QRCodeHandlerRegistry {
    fn default() -> Self {
        Self::new()
    }
}
