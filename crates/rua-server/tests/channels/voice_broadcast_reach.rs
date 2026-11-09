use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use rua_channel::core::channel::{self, AgentRequest, OutMessage, Process, ToHandle};
use rua_channel::core::{ChannelId, ChannelManager};
use rua_core::Result;

struct MockModernChannel {
    channel_id: ChannelId,
    send_count: AtomicUsize,
}

impl MockModernChannel {
    fn new(id: &str) -> Self {
        Self {
            channel_id: id.into(),
            send_count: AtomicUsize::new(0),
        }
    }

    fn sends(&self) -> usize {
        self.send_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl channel::Channel for MockModernChannel {
    fn id(&self) -> &ChannelId {
        &self.channel_id
    }

    async fn start(&self, _process: Arc<dyn Process>) -> Result<()> {
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        Ok(())
    }

    async fn send(&self, _to: ToHandle, _msg: OutMessage) -> Result<()> {
        self.send_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn build_request_from_native(&self, _native: serde_json::Value) -> Result<AgentRequest> {
        Err(rua_core::RuaError::Validation {
            message: "mock".into(),
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn broadcast_reaches_voice_and_mock_im() {
    let mgr = ChannelManager::new();

    let voice = Arc::new(MockModernChannel::new("voice"));
    let mock_im = Arc::new(MockModernChannel::new("test-im"));

    mgr.register(voice.clone()).await.unwrap();
    mgr.register(mock_im.clone()).await.unwrap();

    let results = mgr.broadcast_text("hello world").await;

    assert_eq!(
        results.len(),
        2,
        "broadcast should reach both channels, got: {results:?}"
    );
    for (id, res) in &results {
        assert!(res.is_ok(), "channel {id} should succeed");
    }
    assert_eq!(voice.sends(), 1, "voice channel should receive broadcast");
    assert_eq!(mock_im.sends(), 1, "mock IM should receive broadcast");
}
