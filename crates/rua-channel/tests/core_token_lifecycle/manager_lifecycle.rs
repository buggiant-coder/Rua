use async_trait::async_trait;
use rua_channel::core::{
    AgentRequest, Channel, ChannelLifecycle, ChannelManager, OutMessage, Process, ToHandle,
};
use rua_core::{Result, RuntimeEvent};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

struct EmptyProcess;
#[async_trait]
impl Process for EmptyProcess {
    async fn handle(&self, _: AgentRequest) -> futures::stream::BoxStream<'static, RuntimeEvent> {
        Box::pin(futures::stream::empty())
    }
}

#[tokio::test]
async fn removing_one_channel_preserves_others_and_retains_failed_cleanup() {
    let manager = ChannelManager::new();
    manager.set_process(Arc::new(EmptyProcess)).await;
    let changed = Probe::new("changed");
    let unchanged = Probe::new("unchanged");
    manager.register(changed.clone()).await.unwrap();
    manager.register(unchanged.clone()).await.unwrap();
    manager.start_all().await.unwrap();
    changed.fail_stop.store(true, Ordering::SeqCst);
    assert!(manager.remove_channel("changed").await.is_err());
    assert!(manager.statuses().await.contains_key("changed"));
    assert!(manager
        .handle_incoming(
            "changed".to_string(),
            "session".to_string(),
            serde_json::json!({"query":"new work"})
        )
        .await
        .is_err());
    assert!(manager
        .send_text(
            "changed",
            rua_channel::core::content::ChannelTarget {
                agent_id: "agent".to_string(),
                conversation_id: "session".to_string(),
                sender_id: None,
                reply_to: None,
                metadata: serde_json::Value::Null,
            },
            "new output".to_string()
        )
        .await
        .is_err());
    assert_eq!(unchanged.stops.load(Ordering::SeqCst), 0);
    changed.fail_stop.store(false, Ordering::SeqCst);
    manager.remove_channel("changed").await.unwrap();
    assert!(!manager.statuses().await.contains_key("changed"));
    manager.start_all().await.unwrap();
    assert_eq!(unchanged.starts.load(Ordering::SeqCst), 1);
    manager.stop_all().await.unwrap();
}

struct Probe {
    id: String,
    starts: AtomicUsize,
    stops: AtomicUsize,
    refreshes: AtomicUsize,
    fail_start: AtomicBool,
    fail_stop: AtomicBool,
    block_refresh: AtomicBool,
}

impl Probe {
    fn new(id: &str) -> Arc<Self> {
        Arc::new(Self {
            id: id.into(),
            starts: 0.into(),
            stops: 0.into(),
            refreshes: 0.into(),
            fail_start: false.into(),
            fail_stop: false.into(),
            block_refresh: false.into(),
        })
    }
}

#[async_trait]
impl Channel for Probe {
    fn id(&self) -> &String {
        &self.id
    }
    async fn start(&self, _: Arc<dyn Process>) -> Result<()> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        if self.fail_start.load(Ordering::SeqCst) {
            return Err(rua_core::RuaError::Runtime {
                message: "start failed".into(),
            });
        }
        Ok(())
    }
    async fn stop(&self) -> Result<()> {
        self.stops.fetch_add(1, Ordering::SeqCst);
        if self.fail_stop.load(Ordering::SeqCst) {
            return Err(rua_core::RuaError::Runtime {
                message: "stop failed".into(),
            });
        }
        Ok(())
    }
    async fn send(&self, _: ToHandle, _: OutMessage) -> Result<()> {
        Ok(())
    }
    fn build_request_from_native(&self, _: serde_json::Value) -> Result<AgentRequest> {
        unreachable!()
    }
    async fn refresh_webhook_or_token(&self) -> Result<()> {
        self.refreshes.fetch_add(1, Ordering::SeqCst);
        if self.block_refresh.load(Ordering::SeqCst) {
            futures::future::pending::<()>().await;
        }
        Ok(())
    }
}

#[tokio::test]
async fn registration_requires_process_and_successful_start_to_be_running() {
    let manager = ChannelManager::new();
    let channel = Probe::new("test");
    manager.register(channel.clone()).await.unwrap();
    assert_eq!(
        manager.statuses().await["test"].lifecycle,
        ChannelLifecycle::Registered
    );
    assert!(manager
        .start_all()
        .await
        .unwrap_err()
        .to_string()
        .contains("Process"));
    assert!(!manager.statuses().await["test"].is_running());
    assert_eq!(channel.starts.load(Ordering::SeqCst), 0);
    manager.set_process(Arc::new(EmptyProcess)).await;
    manager.start_all().await.unwrap();
    manager.start_all().await.unwrap();
    assert!(manager.statuses().await["test"].is_running());
    assert_eq!(channel.starts.load(Ordering::SeqCst), 1);
    assert!(manager.register(Probe::new("test")).await.is_err());
    manager.stop_all().await.unwrap();
    manager.stop_all().await.unwrap();
    assert_eq!(channel.stops.load(Ordering::SeqCst), 1);
    assert_eq!(
        manager.statuses().await["test"].lifecycle,
        ChannelLifecycle::Stopped
    );
    manager.register(Probe::new("test")).await.unwrap();
    assert_eq!(
        manager.statuses().await["test"].lifecycle,
        ChannelLifecycle::Registered
    );
}

#[tokio::test]
async fn lifecycle_failure_is_reported_without_skipping_other_channels() {
    let manager = ChannelManager::new().with_refresh_interval_secs(0);
    manager.set_process(Arc::new(EmptyProcess)).await;
    let bad = Probe::new("bad");
    bad.fail_start.store(true, Ordering::SeqCst);
    let good = Probe::new("good");
    manager.register(bad.clone()).await.unwrap();
    manager.register(good.clone()).await.unwrap();
    assert!(manager.start_all().await.is_err());
    assert!(manager.statuses().await["good"].is_running());
    assert!(matches!(
        manager.statuses().await["bad"].lifecycle,
        ChannelLifecycle::Failed { .. }
    ));
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(bad.refreshes.load(Ordering::SeqCst), 0);
    good.fail_stop.store(true, Ordering::SeqCst);
    assert!(manager.stop_all().await.is_err());
    assert_eq!(bad.stops.load(Ordering::SeqCst), 1);
    assert!(matches!(
        manager.statuses().await["good"].lifecycle,
        ChannelLifecycle::Failed { .. }
    ));
    good.fail_stop.store(false, Ordering::SeqCst);
    manager.stop_all().await.unwrap();
}

#[tokio::test]
async fn stop_cancels_inflight_refresh_and_restart_creates_a_live_worker() {
    let manager = ChannelManager::new().with_refresh_interval_secs(0);
    manager.set_process(Arc::new(EmptyProcess)).await;
    let channel = Probe::new("test");
    channel.block_refresh.store(true, Ordering::SeqCst);
    manager.register(channel.clone()).await.unwrap();
    manager.start_all().await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while channel.refreshes.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(1), manager.stop_all())
        .await
        .unwrap()
        .unwrap();
    let before = channel.refreshes.load(Ordering::SeqCst);
    channel.block_refresh.store(false, Ordering::SeqCst);
    manager.start_all().await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while channel.refreshes.load(Ordering::SeqCst) == before {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    manager.stop_all().await.unwrap();
    assert_eq!(channel.starts.load(Ordering::SeqCst), 2);
}
