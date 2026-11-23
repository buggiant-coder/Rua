use super::handoff::Peer;
use super::*;
use rua_device_network::DeviceSessionCommandContext;
use rua_session::chat_handoff::ChatHandoffPhase;
use rua_sync::identity::NetworkSecret;
use serde_json::{json, Value};
use std::sync::atomic::AtomicBool;

struct ReceiptLoss {
    prepare_seen: Notify,
    prepare_release: Notify,
    status_seen: Notify,
    status_release: Notify,
    prepare_once: AtomicBool,
    status_once: AtomicBool,
    drop_once: AtomicBool,
    commands: Mutex<Vec<(String, String)>>,
}

impl ReceiptLoss {
    fn new() -> Self {
        Self {
            prepare_seen: Notify::new(),
            prepare_release: Notify::new(),
            status_seen: Notify::new(),
            status_release: Notify::new(),
            prepare_once: AtomicBool::new(true),
            status_once: AtomicBool::new(true),
            drop_once: AtomicBool::new(true),
            commands: Mutex::new(Vec::new()),
        }
    }

    async fn before(&self, payload: &[u8]) {
        let request: Value = serde_json::from_slice(payload).unwrap();
        if request["command"] != "chat_handoff" {
            return;
        }
        let action = &request["payload"]["action"];
        let kind = action["kind"].as_str().unwrap();
        let id = action["handoff_id"]
            .as_str()
            .or_else(|| action["record"]["handoff_id"].as_str())
            .unwrap();
        self.commands.lock().unwrap().push((kind.into(), id.into()));
        if kind == "prepare" && self.prepare_once.swap(false, Ordering::SeqCst) {
            self.prepare_seen.notify_one();
            self.prepare_release.notified().await;
        }
        if kind == "status" && self.status_once.swap(false, Ordering::SeqCst) {
            self.status_seen.notify_one();
            self.status_release.notified().await;
        }
    }

    fn after(&self, payload: &[u8], result: Result<Vec<u8>, String>) -> Result<Vec<u8>, String> {
        let request: Value = serde_json::from_slice(payload).unwrap();
        let activated = result
            .as_ref()
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
            .is_some_and(|reply| {
                reply["status"] == "chat_handoff"
                    && reply["response"]["record"]["phase"] == "activated"
            });
        if request["command"] == "chat_handoff"
            && request["payload"]["action"]["kind"] == "activate"
            && activated
            && self.drop_once.swap(false, Ordering::SeqCst)
        {
            return Err("injected loss of the committed Activate receipt".into());
        }
        result
    }
}

struct LostActivateHandler {
    inner: Arc<DeviceCommandHandler>,
    loss: Arc<ReceiptLoss>,
}

#[async_trait]
impl DeviceSessionCommandHandler for LostActivateHandler {
    async fn handle_command(&self, payload: &[u8]) -> Result<Vec<u8>, String> {
        self.loss.before(payload).await;
        self.loss
            .after(payload, self.inner.handle_command(payload).await)
    }

    async fn handle_command_from(&self, peer: &str, payload: &[u8]) -> Result<Vec<u8>, String> {
        self.loss.before(payload).await;
        self.loss
            .after(payload, self.inner.handle_command_from(peer, payload).await)
    }

    async fn handle_command_in_session(
        &self,
        peer: &str,
        context: DeviceSessionCommandContext,
        payload: &[u8],
    ) -> Result<Vec<u8>, String> {
        self.loss.before(payload).await;
        self.loss.after(
            payload,
            self.inner
                .handle_command_in_session(peer, context, payload)
                .await,
        )
    }
}

async fn assert_new_input_is_fenced(source: &Peer, chat: &str, phase: ChatHandoffPhase) {
    assert_eq!(
        source
            .store()
            .load_chat_handoff(chat)
            .await
            .unwrap()
            .unwrap()
            .phase,
        phase
    );
    let address = ChatAddress::new(chat);
    let placement = source.placement(chat).await;
    let input = source
        .state
        .conversation_authority
        .accept_submission(
            &source.state.streams,
            &rua_server::streaming::StreamKey::new(
                CONSOLE_CHANNEL_ID,
                address.clone(),
                rua_server::streaming::Priority::Normal,
            ),
            ConversationActiveTurnStart {
                address: address.clone(),
                driver: ConversationDriverKind::Rua,
                authority_device_id: source.device_id.clone(),
                authority_epoch: placement.authority_epoch,
                driver_detail: None,
            },
            ConversationInputPayload {
                message: "concurrent input must wait".into(),
                model_override: None,
                input: None,
                messages: None,
                extra: Default::default(),
                attachments: Vec::new(),
                enqueued_at: Instant::now(),
            },
            None,
            None,
        )
        .await;
    let error = input.err().expect("handoff must reject concurrent input");
    assert!(error.contains("handoff"), "{error}");
    assert!(source
        .state
        .conversation_authority
        .active_turn(&address)
        .await
        .is_none());
    assert!(source
        .state
        .conversation_authority
        .queue_snapshot(&address)
        .await
        .is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_activate_receipt_recovers_by_status_with_one_identity_and_fences_the_old_writer() {
    let secret = NetworkSecret::generate();
    let source = Arc::new(Peer::new(&secret).await);
    let loss = Arc::new(ReceiptLoss::new());
    let for_handler = loss.clone();
    let target = Arc::new(
        Peer::new_with_handler(&secret, move |inner| {
            Arc::new(LostActivateHandler {
                inner,
                loss: for_handler,
            })
        })
        .await,
    );
    source.pair(&[&target]);
    target.pair(&[&source]);
    let chat = uuid::Uuid::new_v4().to_string();
    source.submit(&chat, "before the dropped receipt").await;
    let original = source.placement(&chat).await;
    let handoff = tokio::spawn({
        let source = source.clone();
        let target = target.clone();
        let chat = chat.clone();
        async move {
            source
                .request(
                    Method::PUT,
                    &chat,
                    "/authority-placement",
                    json!({"device_id":target.device_id}),
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(35), loss.prepare_seen.notified())
        .await
        .unwrap();
    assert_new_input_is_fenced(&source, &chat, ChatHandoffPhase::Prepared).await;
    loss.prepare_release.notify_one();
    tokio::time::timeout(Duration::from_secs(35), loss.status_seen.notified())
        .await
        .unwrap();
    assert_new_input_is_fenced(&source, &chat, ChatHandoffPhase::Released).await;
    assert_eq!(
        target
            .store()
            .load_chat_handoff(&chat)
            .await
            .unwrap()
            .unwrap()
            .phase,
        ChatHandoffPhase::Activated
    );
    assert_eq!(target.placement(&chat).await.device_id, target.device_id);
    loss.status_release.notify_one();
    let (status, body) = handoff.await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    let installed = source
        .store()
        .load_chat_handoff(&chat)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(installed.phase, ChatHandoffPhase::Activated);
    assert!(source.placement(&chat).await.authority_epoch > original.authority_epoch);
    assert_eq!(source.placement(&chat).await, target.placement(&chat).await);
    {
        let commands = loss.commands.lock().unwrap();
        assert_eq!(
            commands
                .iter()
                .map(|(kind, _)| kind.as_str())
                .collect::<Vec<_>>(),
            ["prepare", "activate", "status"]
        );
        assert!(commands.iter().all(|(_, id)| id == &installed.handoff_id));
    }
    target
        .submit(&chat, "only the new writer receives this input")
        .await;
    assert_eq!(source.captured.requests.lock().await.len(), 1);
    assert_eq!(target.captured.requests.lock().await.len(), 1);
    target.close().await;
    source.close().await;
}
