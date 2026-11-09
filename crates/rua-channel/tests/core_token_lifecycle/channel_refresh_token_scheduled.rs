use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rua_channel::core::channel::{AgentRequest, Channel, OutMessage, Process, ToHandle};
use rua_channel::core::{ChannelId, ChannelManager};
use rua_core::Result;

struct EmptyProcess;

#[async_trait]
impl Process for EmptyProcess {
    async fn handle(
        &self,
        _req: AgentRequest,
    ) -> futures::stream::BoxStream<'static, rua_core::RuntimeEvent> {
        Box::pin(futures::stream::empty())
    }
}

struct CountingChannel {
    id: ChannelId,
    refresh_count: AtomicUsize,
}

impl CountingChannel {
    fn new(id: &str) -> Self {
        Self {
            id: id.into(),
            refresh_count: AtomicUsize::new(0),
        }
    }

    fn refresh_calls(&self) -> usize {
        self.refresh_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Channel for CountingChannel {
    fn id(&self) -> &ChannelId {
        &self.id
    }
    async fn start(&self, _process: Arc<dyn Process>) -> Result<()> {
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
    async fn refresh_webhook_or_token(&self) -> Result<()> {
        self.refresh_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn scheduled_refresh_calls_channel() {
    let mgr = ChannelManager::new().with_refresh_interval_secs(0);

    let ch = Arc::new(CountingChannel::new("test"));
    mgr.register(ch.clone()).await.unwrap();
    mgr.set_process(Arc::new(EmptyProcess)).await;
    mgr.start_all().await.unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    assert!(
        ch.refresh_calls() >= 1,
        "expected refresh to be called at least once, got {}",
        ch.refresh_calls()
    );

    mgr.stop_all().await.unwrap();
}

#[tokio::test]
async fn refresh_failure_does_not_stop_task() {
    struct FailChannel {
        id: ChannelId,
        call_count: AtomicUsize,
    }

    #[async_trait]
    impl Channel for FailChannel {
        fn id(&self) -> &ChannelId {
            &self.id
        }
        async fn start(&self, _process: Arc<dyn Process>) -> Result<()> {
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
        async fn refresh_webhook_or_token(&self) -> Result<()> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Err(rua_core::RuaError::Runtime {
                message: "token refresh failed".into(),
            })
        }
    }

    let mgr = ChannelManager::new().with_refresh_interval_secs(0);

    let fail_ch = Arc::new(FailChannel {
        id: "fail".into(),
        call_count: AtomicUsize::new(0),
    });
    mgr.register(fail_ch.clone()).await.unwrap();
    mgr.set_process(Arc::new(EmptyProcess)).await;
    mgr.start_all().await.unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    assert!(
        fail_ch.call_count.load(Ordering::SeqCst) >= 2,
        "expected refresh to be called multiple times despite failure, got {}",
        fail_ch.call_count.load(Ordering::SeqCst)
    );
    let statuses = mgr.statuses().await;
    assert!(statuses["fail"].is_running());
    match statuses["fail"].health() {
        rua_channel::core::ChannelHealth::Degraded { reason } => {
            assert!(
                reason.contains("refresh_webhook_or_token failed")
                    && reason.contains("token refresh failed"),
                "unexpected degraded reason: {reason}"
            );
        }
        other => panic!("expected degraded health after refresh failure, got {other:?}"),
    }

    mgr.stop_all().await.unwrap();
}
