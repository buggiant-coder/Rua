use super::handoff::Peer;
use super::*;
use rua_device_network::DeviceSessionCommandContext;
use rua_session::chat_handoff::ChatHandoffPhase;
use rua_sync::identity::NetworkSecret;
use serde_json::{json, Value};
use std::sync::atomic::AtomicBool;

struct ActivateReceiptBarrier {
    once: AtomicBool,
    committed: Notify,
    release: Notify,
}

impl ActivateReceiptBarrier {
    fn new() -> Self {
        Self {
            once: AtomicBool::new(true),
            committed: Notify::new(),
            release: Notify::new(),
        }
    }

    async fn after(
        &self,
        payload: &[u8],
        result: Result<Vec<u8>, String>,
    ) -> Result<Vec<u8>, String> {
        let request: Value = serde_json::from_slice(payload).map_err(|e| e.to_string())?;
        let activated = result
            .as_ref()
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
            .is_some_and(|reply| {
                reply["status"] == "chat_handoff"
                    && reply["response"]["record"]["phase"] == "activated"
            });
        if activated
            && request["command"] == "chat_handoff"
            && request["payload"]["action"]["kind"] == "activate"
            && self.once.swap(false, Ordering::SeqCst)
        {
            self.committed.notify_one();
            self.release.notified().await;
        }
        result
    }
}

struct DelayedActivateReceipt {
    inner: Arc<DeviceCommandHandler>,
    barrier: Arc<ActivateReceiptBarrier>,
}

#[async_trait]
impl DeviceSessionCommandHandler for DelayedActivateReceipt {
    async fn handle_command(&self, payload: &[u8]) -> Result<Vec<u8>, String> {
        self.barrier
            .after(payload, self.inner.handle_command(payload).await)
            .await
    }

    async fn handle_command_from(&self, peer: &str, payload: &[u8]) -> Result<Vec<u8>, String> {
        self.barrier
            .after(payload, self.inner.handle_command_from(peer, payload).await)
            .await
    }

    async fn handle_command_in_session(
        &self,
        peer: &str,
        context: DeviceSessionCommandContext,
        payload: &[u8],
    ) -> Result<Vec<u8>, String> {
        self.barrier
            .after(
                payload,
                self.inner
                    .handle_command_in_session(peer, context, payload)
                    .await,
            )
            .await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn late_activated_receipt_preserves_the_new_writer_configuration_and_allows_return_handoff() {
    let secret = NetworkSecret::generate();
    let source = Arc::new(Peer::new(&secret).await);
    let barrier = Arc::new(ActivateReceiptBarrier::new());
    let barrier_for_handler = barrier.clone();
    let target = Arc::new(
        Peer::new_with_handler(&secret, move |inner| {
            Arc::new(DelayedActivateReceipt {
                inner,
                barrier: barrier_for_handler,
            })
        })
        .await,
    );
    source.pair(&[&target]);
    target.pair(&[&source]);
    let chat = uuid::Uuid::new_v4().to_string();
    source.submit(&chat, "original writer").await;
    let original_placement = source.placement(&chat).await;
    let request_source = source.clone();
    let request_chat = chat.clone();
    let destination = target.device_id.clone();
    let mut request = tokio::spawn(async move {
        request_source
            .request(
                Method::PUT,
                &request_chat,
                "/authority-placement",
                json!({"device_id":destination}),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(80), async {
        tokio::select! {
            _ = barrier.committed.notified() => {},
            result = &mut request => panic!("handoff returned before the Activated receipt barrier: {result:?}"),
        }
    })
    .await
    .expect("target durably activates before its receipt reaches the source");
    let pending = source
        .store()
        .load_chat_handoff(&chat)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pending.phase, ChatHandoffPhase::Released);
    let active = target
        .store()
        .load_chat_handoff(&chat)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(active.phase, ChatHandoffPhase::Activated);
    assert_eq!(active.handoff_id, pending.handoff_id);
    let new_placement = target.placement(&chat).await;
    assert_eq!(new_placement.device_id, target.device_id);
    assert!(new_placement.authority_epoch > original_placement.authority_epoch);

    // A legitimate explicit Chat update on the new writer, while A still waits
    // for the original handoff receipt. Changing mutable defaults alone is not
    // a Chat configuration change (ADR-0028).
    let mut security = target
        .state
        .security
        .config_store()
        .load("default")
        .unwrap();
    security.tool_guard.approval_mode = "ask_first".into();
    target
        .state
        .security
        .save_and_invalidate("default", &security)
        .unwrap();
    let (status, preview) = target
        .request(Method::GET, &chat, "/configuration", Value::Null)
        .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(
        preview["current"]["epoch"],
        active.candidate_configuration_epoch
    );
    assert_eq!(
        preview["candidate"]["security_config"]["tool_guard"]["approval_mode"],
        "ask_first"
    );
    let latest = preview["candidate"].clone();
    assert_ne!(latest["epoch"], active.candidate_configuration_epoch);
    let (status, applied) = target
        .request(
            Method::PUT,
            &chat,
            "/configuration",
            json!({
                "expected_epoch": preview["current"]["epoch"], "candidate_epoch": latest["epoch"],
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    assert_eq!(applied["applied"], true);
    // Configuration PUT persists first and publishes asynchronously. Pull only
    // through the ordinary sync path until the new snapshot is actually applied.
    let synchronized = tokio::time::timeout(Duration::from_secs(25), async {
        loop {
            source.synchronize(&target).await;
            let header = source
                .store()
                .load_chat_header(&chat)
                .await
                .unwrap()
                .unwrap();
            if header.extra["hook_configuration"] == latest
                && source.placement(&chat).await == new_placement
                && source
                    .state
                    .inbound_inbox
                    .diagnostics(8)
                    .unwrap()
                    .counts
                    .retryable
                    == 0
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    if let Err(error) = synchronized {
        let actual = source
            .store()
            .load_chat_header(&chat)
            .await
            .unwrap()
            .unwrap();
        let config = &actual.extra["hook_configuration"];
        panic!("new writer configuration was not applied: {error}; actual epoch={}, mode={}, source={}, cwd={}, expected epoch={}, mode={}, source={}, cwd={}, placement={:?}, inbox={:?}",
            config["epoch"], config["security_config"]["tool_guard"]["approval_mode"], config["authority_device_id"], config["capability_configuration"]["execution_directory"],
            latest["epoch"], latest["security_config"]["tool_guard"]["approval_mode"], latest["authority_device_id"], latest["capability_configuration"]["execution_directory"],
            source.placement(&chat).await, source.state.inbound_inbox.diagnostics(8));
    }
    let replicated = source
        .store()
        .load_chat_header(&chat)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(source.placement(&chat).await, new_placement);
    assert!(
        replicated.extra["hook_configuration"] == latest,
        "new writer snapshot must be persisted before releasing the old receipt"
    );
    assert_eq!(
        source
            .store()
            .load_chat_handoff(&chat)
            .await
            .unwrap()
            .unwrap()
            .phase,
        ChatHandoffPhase::Released
    );

    barrier.release.notify_one();
    let (status, result) = tokio::time::timeout(Duration::from_secs(25), request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{result}");
    let recovered = source
        .store()
        .load_chat_handoff(&chat)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.phase, ChatHandoffPhase::Activated);
    assert_eq!(recovered.handoff_id, pending.handoff_id);
    for peer in [&source, &target] {
        let header = peer.store().load_chat_header(&chat).await.unwrap().unwrap();
        assert!(
            header.extra["hook_configuration"] == latest,
            "late receipt changed configuration on {}: epoch={}, mode={}, source={}, cwd={}",
            peer.device_id,
            header.extra["hook_configuration"]["epoch"],
            header.extra["hook_configuration"]["security_config"]["tool_guard"]["approval_mode"],
            header.extra["hook_configuration"]["authority_device_id"],
            header.extra["hook_configuration"]["capability_configuration"]["execution_directory"]
        );
    }
    target.submit(&chat, "new writer alone").await;
    assert_eq!(source.captured.requests.lock().await.len(), 1);
    assert_eq!(target.captured.requests.lock().await.len(), 1);

    // An old Released journal must not permanently reject C -> A or resurrect
    // the first handoff's snapshot when A becomes the legitimate writer again.
    let (status, returned) = target
        .request(
            Method::PUT,
            &chat,
            "/authority-placement",
            json!({"device_id":source.device_id}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{returned}");
    let returned_placement = source.placement(&chat).await;
    assert_eq!(returned_placement.device_id, source.device_id);
    assert!(returned_placement.authority_epoch > new_placement.authority_epoch);
    let restored = source
        .store()
        .load_chat_header(&chat)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        restored.extra["hook_configuration"]["security_config"],
        latest["security_config"]
    );
    assert_ne!(
        restored.extra["hook_configuration"]["epoch"],
        active.candidate_configuration_epoch
    );
    source.submit(&chat, "returned writer").await;
    assert_eq!(source.captured.requests.lock().await.len(), 2);
    assert_eq!(target.captured.requests.lock().await.len(), 1);
    source.close().await;
    target.close().await;
}
