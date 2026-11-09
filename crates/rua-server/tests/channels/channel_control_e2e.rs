use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rua_channel::core::content::{ChannelTarget, OutgoingMessage};
use rua_channel::core::queue::{QueueConsumer, UnifiedQueueManager};

use rua_channel::core::{
    ApprovalService, ChannelManager, ControlContext, SessionCanceller, WorkspaceReloader,
};
use tokio::sync::mpsc;

struct SpyChannel {
    channel_id: String,
    sent_texts: Arc<Mutex<Vec<String>>>,
}

impl SpyChannel {
    fn new(id: &str) -> (Arc<Self>, Arc<Mutex<Vec<String>>>) {
        let texts = Arc::new(Mutex::new(Vec::new()));
        let ch = Arc::new(Self {
            channel_id: id.to_string(),
            sent_texts: Arc::clone(&texts),
        });
        (ch, texts)
    }
}

#[async_trait]
impl rua_channel::core::Channel for SpyChannel {
    fn id(&self) -> &String {
        &self.channel_id
    }
    async fn start(&self, _process: Arc<dyn rua_channel::core::Process>) -> rua_core::Result<()> {
        Ok(())
    }
    async fn stop(&self) -> rua_core::Result<()> {
        Ok(())
    }
    async fn send(&self, _target: ChannelTarget, message: OutgoingMessage) -> rua_core::Result<()> {
        for content in message.content {
            if let rua_channel::core::OutgoingContentPart::Text { text } = content {
                self.sent_texts.lock().unwrap().push(text);
            }
        }
        Ok(())
    }
    fn build_request_from_native(
        &self,
        _native: serde_json::Value,
    ) -> rua_core::Result<rua_channel::core::AgentRequest> {
        Err(rua_core::RuaError::Validation {
            message: "outbound test channel".into(),
        })
    }
}

struct MockCanceller {
    has_active: AtomicBool,
    cancel_called: AtomicBool,
    session_count: AtomicUsize,
}

#[async_trait]
impl SessionCanceller for MockCanceller {
    async fn cancel_session(&self, _session_id: &str) -> bool {
        self.cancel_called.store(true, Ordering::SeqCst);
        self.has_active.load(Ordering::SeqCst)
    }
    async fn active_session_count(&self) -> usize {
        self.session_count.load(Ordering::SeqCst)
    }
}

struct MockRegistry;

#[async_trait]
impl WorkspaceReloader for MockRegistry {
    async fn reload_all(&self) -> rua_core::Result<()> {
        Ok(())
    }
}

struct MockApproval;

#[async_trait]
impl ApprovalService for MockApproval {
    async fn approve_pending(&self, _session_id: &str) -> bool {
        false
    }
    async fn reject_pending(&self, _session_id: &str) -> bool {
        false
    }
}

struct RecordingConsumer {
    tx: mpsc::UnboundedSender<(String, String, u32, serde_json::Value)>,
}

#[async_trait]
impl QueueConsumer for RecordingConsumer {
    async fn consume(
        &self,
        mut rx: mpsc::Receiver<serde_json::Value>,
        channel_id: String,
        session_id: String,
        priority: u32,
    ) {
        while let Some(payload) = rx.recv().await {
            let _ = self
                .tx
                .send((channel_id.clone(), session_id.clone(), priority, payload));
        }
    }
}

fn make_control_ctx(has_active: bool) -> (Arc<ControlContext>, Arc<MockCanceller>) {
    let canceller = Arc::new(MockCanceller {
        has_active: AtomicBool::new(has_active),
        cancel_called: AtomicBool::new(false),
        session_count: AtomicUsize::new(1),
    });
    let ctx = Arc::new(ControlContext {
        session_canceller: canceller.clone(),
        workspace_reloader: Arc::new(MockRegistry),
        approval_service: Arc::new(MockApproval),
        started_at: std::time::Instant::now(),
        data_dir: std::env::temp_dir().join("rua-channel-control-e2e"),
        exit_fn: None,
    });
    (ctx, canceller)
}

fn make_manager_with_recorder() -> (
    ChannelManager,
    mpsc::UnboundedReceiver<(String, String, u32, serde_json::Value)>,
) {
    let (tx, rx) = mpsc::unbounded_channel();
    let consumer = Arc::new(RecordingConsumer { tx });
    let qm = UnifiedQueueManager::new(consumer);
    let mgr = ChannelManager::new().with_queue_manager(qm);
    (mgr, rx)
}

fn target(session_id: &str) -> ChannelTarget {
    ChannelTarget {
        agent_id: "default".into(),
        conversation_id: session_id.into(),
        sender_id: Some("user-1".into()),
        reply_to: None,
        metadata: serde_json::json!({}),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_cancels_active_task_and_sends_feedback() {
    let (mgr, _rx) = make_manager_with_recorder();
    let (ctx, canceller) = make_control_ctx(true);
    mgr.set_control_context(ctx).await;

    let (ch, texts) = SpyChannel::new("dingtalk");
    mgr.register(ch).await.unwrap();
    mgr.register_session_target("s1", target("s1")).await;

    mgr.handle_incoming(
        "dingtalk".into(),
        "s1".into(),
        serde_json::json!({"text": "/stop"}),
    )
    .await
    .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert!(
        canceller.cancel_called.load(Ordering::SeqCst),
        "cancel_session must be called"
    );
    {
        let sent = texts.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0], "已停止当前任务");
    }

    mgr.stop_all().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_status_on_telegram_has_pid_and_version() {
    let (mgr, _rx) = make_manager_with_recorder();
    let (ctx, _) = make_control_ctx(false);
    mgr.set_control_context(ctx).await;

    let (ch, texts) = SpyChannel::new("telegram");
    mgr.register(ch).await.unwrap();
    mgr.register_session_target("s1", target("s1")).await;

    mgr.handle_incoming(
        "telegram".into(),
        "s1".into(),
        serde_json::json!({"text": "/daemon status"}),
    )
    .await
    .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    {
        let sent = texts.lock().unwrap();
        assert_eq!(sent.len(), 1);
        let text = &sent[0];
        assert!(text.contains("pid:"), "missing pid, got: {text}");
        assert!(
            text.contains("version:"),
            "missing version field, got: {text}"
        );
        assert!(
            text.contains(env!("CARGO_PKG_VERSION")),
            "missing pkg version, got: {text}"
        );
    }

    mgr.stop_all().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_version_consistent_across_channels() {
    let (mgr, _rx) = make_manager_with_recorder();
    let (ctx, _) = make_control_ctx(false);
    mgr.set_control_context(ctx).await;

    let (ch_dt, texts_dt) = SpyChannel::new("dingtalk");
    let (ch_tg, texts_tg) = SpyChannel::new("telegram");
    mgr.register(ch_dt).await.unwrap();
    mgr.register(ch_tg).await.unwrap();
    mgr.register_session_target("s-dt", target("s-dt")).await;
    mgr.register_session_target("s-tg", target("s-tg")).await;

    let payload = serde_json::json!({"text": "/daemon version"});
    mgr.handle_incoming("dingtalk".into(), "s-dt".into(), payload.clone())
        .await
        .unwrap();
    mgr.handle_incoming("telegram".into(), "s-tg".into(), payload)
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    {
        let dt = texts_dt.lock().unwrap();
        let tg = texts_tg.lock().unwrap();
        assert_eq!(dt.len(), 1);
        assert_eq!(tg.len(), 1);
        assert_eq!(dt[0], tg[0], "version must be identical across channels");
        assert_eq!(dt[0], env!("CARGO_PKG_VERSION"));
    }

    mgr.stop_all().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prefix_mismatch_stopx_enqueues_daemon_status_controls() {
    let (mgr, mut rx) = make_manager_with_recorder();
    let (ctx, _) = make_control_ctx(false);
    mgr.set_control_context(ctx).await;

    let (ch, texts) = SpyChannel::new("dingtalk");
    mgr.register(ch).await.unwrap();
    mgr.register_session_target("s1", target("s1")).await;

    mgr.handle_incoming(
        "dingtalk".into(),
        "s1".into(),
        serde_json::json!({"text": "/stopx"}),
    )
    .await
    .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let (_, _, prio, _) = rx.try_recv().expect("/stopx must enqueue");
    assert_eq!(prio, 20, "/stopx should get default priority");

    mgr.handle_incoming(
        "dingtalk".into(),
        "s1".into(),
        serde_json::json!({"text": "/Daemon Status"}),
    )
    .await
    .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(rx.try_recv().is_err(), "/Daemon Status must not enqueue");

    {
        let sent = texts.lock().unwrap();
        assert_eq!(sent.len(), 1, "only /Daemon Status sends response");
        assert!(sent[0].contains("pid:"), "status response missing pid");
    }

    mgr.stop_all().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_and_non_text_payloads_do_not_panic() {
    let (mgr, mut rx) = make_manager_with_recorder();
    let (ctx, _) = make_control_ctx(false);
    mgr.set_control_context(ctx).await;

    mgr.handle_incoming("ch".into(), "s1".into(), serde_json::json!({}))
        .await
        .unwrap();
    mgr.handle_incoming("ch".into(), "s2".into(), serde_json::json!({"text": ""}))
        .await
        .unwrap();
    mgr.handle_incoming(
        "ch".into(),
        "s3".into(),
        serde_json::json!({"image": "base64"}),
    )
    .await
    .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let mut enqueued = Vec::new();
    while let Ok(item) = rx.try_recv() {
        enqueued.push(item);
    }
    assert_eq!(enqueued.len(), 3, "all 3 payloads must enqueue");
    for (_, _, prio, _) in &enqueued {
        assert_eq!(*prio, 20, "non-command payload gets default priority");
    }

    mgr.stop_all().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multi_channel_parallel_no_cross_contamination() {
    let (mgr, _rx) = make_manager_with_recorder();
    let (ctx, canceller) = make_control_ctx(true);
    mgr.set_control_context(ctx).await;

    let (ch_dt, texts_dt) = SpyChannel::new("dingtalk");
    let (ch_tg, texts_tg) = SpyChannel::new("telegram");
    mgr.register(ch_dt).await.unwrap();
    mgr.register(ch_tg).await.unwrap();
    mgr.register_session_target("s-dt", target("s-dt")).await;
    mgr.register_session_target("s-tg", target("s-tg")).await;

    mgr.handle_incoming(
        "dingtalk".into(),
        "s-dt".into(),
        serde_json::json!({"text": "/stop"}),
    )
    .await
    .unwrap();
    mgr.handle_incoming(
        "telegram".into(),
        "s-tg".into(),
        serde_json::json!({"text": "/daemon version"}),
    )
    .await
    .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    {
        let dt = texts_dt.lock().unwrap();
        let tg = texts_tg.lock().unwrap();

        assert_eq!(dt.len(), 1, "dingtalk should get exactly 1 response");
        assert_eq!(dt[0], "已停止当前任务");

        assert_eq!(tg.len(), 1, "telegram should get exactly 1 response");
        assert_eq!(tg[0], env!("CARGO_PKG_VERSION"));
    }

    assert!(canceller.cancel_called.load(Ordering::SeqCst));

    mgr.stop_all().await.unwrap();
}
