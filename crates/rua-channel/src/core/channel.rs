use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::core::content::{AgentChannelRequest, ChannelTarget, OutgoingMessage};
use crate::core::ChannelId;
use rua_core::{Result, RuntimeEvent};

pub type ToHandle = ChannelTarget;

pub type OutMessage = OutgoingMessage;

pub type AgentRequest = AgentChannelRequest;

#[async_trait]
pub trait Channel: Send + Sync {
    fn id(&self) -> &ChannelId;

    async fn start(&self, process: Arc<dyn Process>) -> Result<()>;

    async fn stop(&self) -> Result<()>;

    async fn send(&self, to: ToHandle, msg: OutMessage) -> Result<()>;

    fn build_request_from_native(&self, native: serde_json::Value) -> Result<AgentRequest>;

    async fn refresh_webhook_or_token(&self) -> Result<()> {
        Ok(())
    }
}

#[async_trait]
pub trait Process: Send + Sync {
    async fn handle(&self, req: AgentRequest) -> BoxStream<'static, RuntimeEvent>;
}
