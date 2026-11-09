use std::sync::Arc;

use async_trait::async_trait;

use crate::core::channel::{self, AgentRequest, OutMessage, Process, ToHandle};
use crate::core::ChannelId;
use rua_core::{Result, RuaError};

use crate::imessage::config::IMessageConfig;

pub struct IMessageChannel {
    channel_id: ChannelId,
}

impl IMessageChannel {
    pub fn new(_config: IMessageConfig) -> Self {
        Self {
            channel_id: "imessage".into(),
        }
    }
}

#[async_trait]
impl channel::Channel for IMessageChannel {
    fn id(&self) -> &ChannelId {
        &self.channel_id
    }

    async fn start(&self, _process: Arc<dyn Process>) -> Result<()> {
        Err(RuaError::NotImplemented {
            feature: "iMessage channel requires macOS".into(),
        })
    }

    async fn stop(&self) -> Result<()> {
        Ok(())
    }

    async fn send(&self, _to: ToHandle, _msg: OutMessage) -> Result<()> {
        Err(RuaError::NotImplemented {
            feature: "iMessage channel requires macOS".into(),
        })
    }

    fn build_request_from_native(&self, _native: serde_json::Value) -> Result<AgentRequest> {
        Err(RuaError::NotImplemented {
            feature: "iMessage channel requires macOS".into(),
        })
    }
}
