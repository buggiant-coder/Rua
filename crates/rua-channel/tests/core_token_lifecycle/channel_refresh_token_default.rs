use async_trait::async_trait;
use rua_channel::core::channel::{AgentRequest, Channel, OutMessage, Process, ToHandle};
use rua_channel::core::ChannelId;
use rua_core::Result;

struct NoopChannel {
    id: ChannelId,
}

#[async_trait]
impl Channel for NoopChannel {
    fn id(&self) -> &ChannelId {
        &self.id
    }
    async fn start(&self, _process: std::sync::Arc<dyn Process>) -> Result<()> {
        Ok(())
    }
    async fn stop(&self) -> Result<()> {
        Ok(())
    }
    async fn send(&self, _to: ToHandle, _msg: OutMessage) -> Result<()> {
        Ok(())
    }
    fn build_request_from_native(&self, _native: serde_json::Value) -> Result<AgentRequest> {
        unimplemented!()
    }
}

#[tokio::test]
async fn default_refresh_returns_ok() {
    let ch = NoopChannel { id: "noop".into() };
    let result = ch.refresh_webhook_or_token().await;
    assert!(result.is_ok());
}
