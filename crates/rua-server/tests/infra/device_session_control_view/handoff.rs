use super::*;
use rua_sync::endpoint::SyncNode;
use rua_sync::identity::NetworkSecret;
use serde_json::{json, Value};

pub(super) struct PersistingCaptureRuntime(pub(super) CapturingAgentRuntime);

#[async_trait]
impl AgentRuntime for PersistingCaptureRuntime {
    async fn stream(
        &self,
        req: AgentRequest,
    ) -> rua_core::Result<Pin<Box<dyn futures::Stream<Item = Result<RuntimeEvent, RuaError>> + Send>>>
    {
        // The capture-only runtime emits no durable output. Give this fixture a
        // real completed Turn so handoff checks the production history boundary.
        let store = WorkspaceSessionStore::new(req.conversation_store_dir().unwrap());
        let turn_id = req.metadata["turn_id"].as_str().unwrap();
        rua_session::TurnWriter::new(&store, req.session_id.as_str(), turn_id)
            .append_output(
                vec![rua_session::NewConversationMessage {
                    message_id: uuid::Uuid::new_v4().to_string(),
                    role: "assistant".into(),
                    created_at: chrono::Utc::now().to_rfc3339(),
                    status: "completed".into(),
                    content: json!([{ "type": "text", "text": "fixture response" }]),
                    provider: None,
                    tool: None,
                    metadata: Default::default(),
                }],
                false,
                None,
            )
            .await?;
        self.0.stream(req).await
    }

    async fn cancel(&self, session_id: &rua_core::id::SessionId) -> rua_core::Result<()> {
        self.0.cancel(session_id).await
    }

    async fn warmup(&self, context: RuntimeWarmupContext) -> rua_core::Result<()> {
        self.0.warmup(context).await
    }

    async fn shutdown(&self) -> rua_core::Result<()> {
        self.0.shutdown().await
    }
}

pub(super) struct Peer {
    _dir: TempDir,
    pub(super) state: AppState,
    pub(super) captured: CapturingAgentRuntime,
    pub(super) device_id: String,
    pub(super) addr: iroh::EndpointAddr,
}

impl Peer {
    pub(super) async fn new(secret: &NetworkSecret) -> Self {
        Self::new_with_handler(secret, |handler| handler).await
    }

    pub(super) async fn new_with_handler(
        secret: &NetworkSecret,
        wrap: impl FnOnce(Arc<DeviceCommandHandler>) -> Arc<dyn DeviceSessionCommandHandler>,
    ) -> Self {
        let dir = TempDir::new().unwrap();
        let state = AppState::from_tempdir(dir.path().into(), ServerProfile::Test);
        // Install one paired runtime explicitly. A secret present during
        // AppState construction would eagerly bind a second docs store.
        secret.save(&dir.path().join("sync")).unwrap();
        *state.sync_network_secret.write().unwrap() =
            Some(Arc::new(NetworkSecret::from_bytes(&secret.to_bytes())));
        let device = state.sync_device.read().unwrap().clone().unwrap();
        let device_id = hex::encode(device.device_id_short());
        let captured = CapturingAgentRuntime::default();
        state
            .registry
            .get_or_create("default")
            .await
            .unwrap()
            .set_react_runtime(Arc::new(PersistingCaptureRuntime(captured.clone())))
            .await;
        seed_fixture_provider(&state, vec![fixture_provider_model("available")]).await;
        let node = SyncNode::bind_with_command_handler(
            &dir.path().join("sync"),
            secret.namespace_secret(),
            &device,
            None,
            wrap(state.device_command_handler().unwrap()),
        )
        .await
        .unwrap();
        let mut addr = iroh::EndpointAddr::new(node.endpoint().id());
        for socket in node.endpoint().bound_sockets() {
            let socket = match socket {
                std::net::SocketAddr::V4(socket) if socket.ip().is_unspecified() => {
                    (std::net::Ipv4Addr::LOCALHOST, socket.port()).into()
                }
                std::net::SocketAddr::V6(socket) if socket.ip().is_unspecified() => {
                    (std::net::Ipv6Addr::LOCALHOST, socket.port()).into()
                }
                socket => socket,
            };
            addr.addrs.insert(iroh::TransportAddr::Ip(socket));
        }
        state.sync_enabled.store(true, Ordering::SeqCst);
        state.install_sync_node(node.clone()).await;
        Self {
            _dir: dir,
            state,
            captured,
            device_id,
            addr,
        }
    }

    pub(super) fn pair(&self, peers: &[&Peer]) {
        let devices = peers
            .iter()
            .map(|peer| {
                let public_key = *peer.addr.id.as_bytes();
                let mut device_id = [0; 8];
                device_id.copy_from_slice(&public_key[..8]);
                DeviceEntry {
                    device_id,
                    public_key,
                    label: peer.device_id.clone(),
                    friendly_name: peer.device_id.clone(),
                    added_at_ms: 1,
                    last_seen_ms: 1,
                    direct_addrs: direct_addrs(&peer.addr),
                }
            })
            .collect();
        rua_sync::control_store::SyncControlStore::from_data_dir(&self.state.rua_root)
            .write_device_list(&DeviceList {
                devices,
                ..Default::default()
            })
            .unwrap();
    }

    pub(super) fn store(&self) -> WorkspaceSessionStore {
        WorkspaceSessionStore::new(self.state.chat_workspace().as_path())
    }

    pub(super) async fn request(
        &self,
        method: Method,
        chat: &str,
        suffix: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        let response = tokio::time::timeout(
            Duration::from_secs(110),
            rua_server::router::build_app(self.state.clone(), None).oneshot(
                Request::builder()
                    .method(method)
                    .uri(format!("/api/chats/{chat}{suffix}"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            ),
        )
        .await
        .expect("bounded handoff HTTP response")
        .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).expect("JSON response"),
        )
    }

    pub(super) async fn submit(&self, chat: &str, text: &str) {
        let (status,body) = self.request(Method::POST,chat,"/console/submit",json!({
            "message":text,"agent_id":"default","submission_id":uuid::Uuid::new_v4().to_string(),
            "execution_directory":{"device_id":self.device_id},
            "model":{"provider_id":"fixture","model":"available"},
        })).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["accepted"], true, "{body}");
        self.idle(chat).await;
    }

    pub(super) async fn idle(&self, chat: &str) {
        let result = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let turns = self
                    .store()
                    .read_turn_range(chat, 1, usize::MAX)
                    .await
                    .unwrap();
                if !turns.is_empty()
                    && turns.iter().all(|t| t.lifecycle.status == "completed")
                    && self
                        .state
                        .conversation_authority
                        .active_turn(&ChatAddress::new(chat))
                        .await
                        .is_none()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        assert!(
            result.is_ok(),
            "Chat {chat} did not become idle; turns={:?}, active={:?}, captured={}",
            self.store()
                .read_turn_range(chat, 1, usize::MAX)
                .await
                .unwrap(),
            self.state
                .conversation_authority
                .active_turn(&ChatAddress::new(chat))
                .await,
            self.captured.requests.lock().await.len()
        );
    }

    pub(super) async fn synchronize(&self, source: &Peer) {
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        self.state
            .remote_history_sync_tx
            .send(ConversationHistorySyncRequest {
                source_device_id: source.device_id.clone(),
                result_tx,
            })
            .unwrap();
        let result = tokio::time::timeout(Duration::from_secs(35), result_rx)
            .await
            .unwrap()
            .unwrap();
        assert!(
            result.is_ok(),
            "synchronize {} from {}: {result:?}; target inbox={:?}; source inbox={:?}",
            self.device_id,
            source.device_id,
            self.state.inbound_inbox.diagnostics(8),
            source.state.inbound_inbox.diagnostics(8),
        );
    }

    pub(super) async fn placement(&self, chat: &str) -> ConversationAuthorityPlacement {
        rua_server::services::conversation_authority_target::load_optional_chat_authority_placement(
            self.state.chat_workspace().as_path(),
            chat,
        )
        .await
        .unwrap()
        .unwrap()
    }

    pub(super) async fn close(&self) {
        self.state.clear_sync_node().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cooperative_handoff_uses_target_directory_and_keeps_work_files_at_the_source() {
    let secret = NetworkSecret::generate();
    let source = Peer::new(&secret).await;
    let target = Peer::new(&secret).await;
    source.pair(&[&target]);
    target.pair(&[&source]);
    let chat = uuid::Uuid::new_v4().to_string();
    source.submit(&chat, "before handoff").await;
    let wake = source
        .store()
        .set_collaboration_wake_suppressed(&chat, true)
        .await
        .unwrap();
    assert!(wake.suppressed);
    assert!(wake.stopped_at.is_some());
    let old = source.placement(&chat).await;
    let header = source
        .store()
        .load_chat_header(&chat)
        .await
        .unwrap()
        .unwrap();
    let source_directory = PathBuf::from(
        header.extra["execution_directory"]["path"]
            .as_str()
            .unwrap(),
    );
    std::fs::write(
        source_directory.join("keep-source.txt"),
        b"work bytes stay here",
    )
    .unwrap();
    let media_id = uuid::Uuid::new_v4();
    let media = rua_session::chat_media::ChatMediaManifest::new(
        media_id,
        &chat,
        &source.device_id,
        "image/png".into(),
        b"original photo bytes",
    )
    .unwrap();
    let source_media = source.state.chat_workspace().as_path().join("media");
    std::fs::create_dir_all(&source_media).unwrap();
    media
        .store_copy(&source_media, b"original photo bytes")
        .unwrap();
    media
        .record_reference(&source.store().chat_dir(&chat), &chat)
        .unwrap();
    let target_directory = target.state.rua_root.join("photos");
    std::fs::create_dir(&target_directory).unwrap();
    std::fs::write(target_directory.join("keep-target.txt"), b"existing target").unwrap();
    let before = load_conversation_manifest(
        source.state.chat_workspace().as_path(),
        &ChatAddress::new(&chat),
    )
    .await
    .unwrap()
    .unwrap();
    let (status, body) = source
        .request(
            Method::PUT,
            &chat,
            "/authority-placement",
            json!({
                "device_id":target.device_id,"path":target_directory,
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let new = target.placement(&chat).await;
    assert_eq!(new.device_id, target.device_id);
    assert!(new.authority_epoch > old.authority_epoch);
    assert_eq!(source.placement(&chat).await, new);
    assert_eq!(
        PathBuf::from(
            body["meta"]["execution_directory"]["path"]
                .as_str()
                .unwrap()
        )
        .canonicalize()
        .unwrap(),
        target_directory.canonicalize().unwrap()
    );
    assert_eq!(body["meta"]["execution_directory"]["managed"], false);
    let after = load_conversation_manifest(
        target.state.chat_workspace().as_path(),
        &ChatAddress::new(&chat),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(after.compare(&before).matches, "history is preserved");
    assert!(!target_directory.join("keep-source.txt").exists());
    assert!(
        !target
            .state
            .chat_workspace()
            .as_path()
            .join("media")
            .join(media_id.to_string())
            .exists(),
        "handoff does not copy attachment bytes"
    );
    media.verify_copy(&source_media).unwrap();
    let target_media = rua_session::chat_media::ChatMediaManifest::load_reference(
        &target.store().chat_dir(&chat),
        media_id,
    )
    .unwrap()
    .expect("the original media source reference is preserved");
    assert_eq!(target_media.device_ids, media.device_ids);
    assert_eq!(
        std::fs::read(source_directory.join("keep-source.txt")).unwrap(),
        b"work bytes stay here"
    );
    assert_eq!(
        std::fs::read(target_directory.join("keep-target.txt")).unwrap(),
        b"existing target"
    );
    assert_eq!(
        target
            .store()
            .load_collaboration_wake_state(&chat)
            .await
            .unwrap(),
        wake,
        "handoff preserves the original background suppression and stop time"
    );
    target.submit(&chat, "after handoff").await;
    assert!(
        !target
            .store()
            .load_collaboration_wake_state(&chat)
            .await
            .unwrap()
            .suppressed
    );
    assert_eq!(source.captured.requests.lock().await.len(), 1);
    assert_eq!(target.captured.requests.lock().await.len(), 1);
    let (status, body) = target
        .request(
            Method::PUT,
            &chat,
            "/authority-placement",
            json!({
                "device_id":source.device_id,
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["meta"]["execution_directory"]["managed"], true);
    assert_eq!(
        body["meta"]["execution_directory"]["path"],
        source_directory.to_str().unwrap()
    );
    assert_eq!(
        std::fs::read(source_directory.join("keep-source.txt")).unwrap(),
        b"work bytes stay here"
    );
    source.close().await;
    target.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_handoff_directory_restores_the_original_authority_admission() {
    let secret = NetworkSecret::generate();
    let source = Peer::new(&secret).await;
    let target = Peer::new(&secret).await;
    source.pair(&[&target]);
    target.pair(&[&source]);
    let chat = uuid::Uuid::new_v4().to_string();
    source.submit(&chat, "before invalid directory").await;
    let old = source.placement(&chat).await;
    let file = target.state.rua_root.join("not-a-directory");
    std::fs::write(&file, b"unchanged target file").unwrap();
    let (status, body) = source
        .request(
            Method::PUT,
            &chat,
            "/authority-placement",
            json!({
                "device_id":target.device_id,"path":file,
            }),
        )
        .await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert!(
        body["detail"].as_str().unwrap().contains("directory"),
        "{body}"
    );
    assert_eq!(source.placement(&chat).await, old);
    assert_eq!(target.placement(&chat).await, old);
    assert_eq!(std::fs::read(&file).unwrap(), b"unchanged target file");
    source.submit(&chat, "source is still writable").await;
    assert_eq!(source.captured.requests.lock().await.len(), 2);
    assert!(target.captured.requests.lock().await.is_empty());
    source.close().await;
    target.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn offline_old_authority_cannot_be_forcefully_taken_over_by_the_target() {
    let secret = NetworkSecret::generate();
    let source = Peer::new(&secret).await;
    let target = Peer::new(&secret).await;
    source.pair(&[&target]);
    target.pair(&[&source]);
    let chat = uuid::Uuid::new_v4().to_string();
    source.submit(&chat, "source will go offline").await;
    target.synchronize(&source).await;
    let old = target.placement(&chat).await;
    source.close().await;
    let (status, body) = target
        .request(
            Method::PUT,
            &chat,
            "/authority-placement",
            json!({
                "device_id":target.device_id,
            }),
        )
        .await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert_eq!(target.placement(&chat).await, old);
    assert_eq!(source.placement(&chat).await, old);
    assert!(target.captured.requests.lock().await.is_empty());
    assert!(target
        .state
        .conversation_authority
        .active_turn(&ChatAddress::new(&chat))
        .await
        .is_none());
    target.close().await;
}

pub(super) struct HeldTaskRunner;

#[async_trait]
impl rua_task::TaskRunner for HeldTaskRunner {
    fn kind(&self) -> rua_task::TaskRunnerKind {
        rua_task::TaskRunnerKind::Tool
    }

    async fn run(
        &self,
        request: rua_task::TaskRunRequest,
    ) -> Result<rua_task::TaskRunOutput, rua_task::TaskRunnerError> {
        request.cancel.cancelled().await;
        Ok(rua_task::TaskRunOutput::cancelled())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn offline_task_device_does_not_block_cooperative_handoff_between_the_other_two_devices() {
    let secret = NetworkSecret::generate();
    let source = Peer::new(&secret).await;
    let executor = Peer::new(&secret).await;
    let target = Peer::new(&secret).await;
    source.pair(&[&executor, &target]);
    executor.pair(&[&source, &target]);
    target.pair(&[&source, &executor]);
    let chat = uuid::Uuid::new_v4().to_string();
    source.submit(&chat, "own a remote background Task").await;
    executor.synchronize(&source).await;
    let header = executor
        .store()
        .load_chat_header(&chat)
        .await
        .unwrap()
        .unwrap();
    let configuration_epoch = header.extra["hook_configuration"]["epoch"]
        .as_str()
        .unwrap()
        .to_owned();
    let cwd = executor.state.rua_root.canonicalize().unwrap();
    let owner = rua_task::TaskOwner::new(chat.clone());
    executor
        .state
        .task_runtime
        .register_runner(Arc::new(HeldTaskRunner));
    let task_id = executor
        .state
        .task_runtime
        .create(
            rua_task::TaskSpec {
                owner: owner.clone(),
                instruction: "remain alive while the network is offline".into(),
                runner: rua_task::TaskRunnerSpec::Tool {
                    tool_name: "bash".into(),
                    arguments: json!({
                        "command":"fixture held runner", "device_id":executor.device_id, "cwd":cwd,
                    }),
                },
                context_refs: vec![],
                depends_on: vec![],
                origin: Default::default(),
            },
            rua_task::TaskAttemptAuthorization::new("default")
                .unwrap()
                .with_configuration_epoch(configuration_epoch),
        )
        .await
        .unwrap();
    let record = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let record = executor
                .state
                .task_runtime
                .get(&owner, &task_id)
                .await
                .unwrap();
            if record.state == rua_task::TaskState::Running {
                break record;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    rua_server::services::chat_task::capture_task_reference(
        &executor.state.rua_root,
        &executor.device_id,
        &record,
    )
    .unwrap();
    let publication = rua_server::sync_publication::publish_sync_fact(
        &executor.state,
        rua_server::sync_publication::SyncPublicationFact::StableConversationState {
            scope_root: executor.state.chat_workspace().as_path().to_path_buf(),
            address: ChatAddress::new(&chat),
        },
    )
    .await;
    assert!(publication.errors.is_empty(), "{:?}", publication.errors);
    source.synchronize(&executor).await;
    let reference =
        rua_session::task_refs::ChatTaskRef::load(&source.store().chat_dir(&chat), &chat, &task_id)
            .unwrap()
            .expect("source received the actual execution Device evidence through normal sync");
    assert_eq!(
        reference
            .last_target_evidence
            .as_ref()
            .unwrap()
            .record
            .state,
        rua_task::TaskState::Running
    );
    executor.close().await;
    let (status, body) = source
        .request(
            Method::PUT,
            &chat,
            "/authority-placement",
            json!({"device_id":target.device_id}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let moved =
        rua_session::task_refs::ChatTaskRef::load(&target.store().chat_dir(&chat), &chat, &task_id)
            .unwrap()
            .unwrap();
    assert_eq!(moved.execution, reference.execution);
    assert_eq!(
        moved
            .last_target_evidence
            .as_ref()
            .unwrap()
            .record
            .attempt_no,
        1
    );
    assert_eq!(
        moved.last_target_evidence.as_ref().unwrap().record.state,
        rua_task::TaskState::Running
    );
    let (status, list) = target
        .request(Method::GET, &chat, "/tasks", Value::Null)
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let listed = list
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["task_id"] == task_id)
        .unwrap();
    assert_eq!(listed["availability"], "unknown");
    assert_eq!(listed["state"], "running");
    assert_eq!(
        executor
            .state
            .task_runtime
            .get(&owner, &task_id)
            .await
            .unwrap()
            .state,
        rua_task::TaskState::Running
    );
    executor
        .state
        .task_runtime
        .cancel(&owner, &task_id)
        .await
        .unwrap();
    source.close().await;
    target.close().await;
}

struct PrepareBarrier {
    entered: Notify,
    release: Notify,
    first: std::sync::atomic::AtomicBool,
}

struct HeldPrepareHandler {
    inner: Arc<DeviceCommandHandler>,
    barrier: Arc<PrepareBarrier>,
}

impl HeldPrepareHandler {
    async fn before(&self, payload: &[u8]) {
        let request: Value = serde_json::from_slice(payload).unwrap();
        if request["command"] == "chat_handoff"
            && request["payload"]["action"]["kind"] == "prepare"
            && self.barrier.first.swap(false, Ordering::SeqCst)
        {
            self.barrier.entered.notify_one();
            self.barrier.release.notified().await;
        }
    }
}

#[async_trait]
impl DeviceSessionCommandHandler for HeldPrepareHandler {
    async fn handle_command(&self, payload: &[u8]) -> Result<Vec<u8>, String> {
        self.before(payload).await;
        self.inner.handle_command(payload).await
    }

    async fn handle_command_from(&self, peer: &str, payload: &[u8]) -> Result<Vec<u8>, String> {
        self.before(payload).await;
        self.inner.handle_command_from(peer, payload).await
    }

    async fn handle_command_in_session(
        &self,
        peer: &str,
        context: rua_device_network::DeviceSessionCommandContext,
        payload: &[u8],
    ) -> Result<Vec<u8>, String> {
        self.before(payload).await;
        self.inner
            .handle_command_in_session(peer, context, payload)
            .await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn target_model_unavailability_aborts_handoff_without_releasing_the_source() {
    let secret = NetworkSecret::generate();
    let source = Peer::new(&secret).await;
    let barrier = Arc::new(PrepareBarrier {
        entered: Notify::new(),
        release: Notify::new(),
        first: std::sync::atomic::AtomicBool::new(true),
    });
    let wrapper_barrier = barrier.clone();
    let target = Peer::new_with_handler(&secret, move |inner| {
        Arc::new(HeldPrepareHandler {
            inner,
            barrier: wrapper_barrier,
        })
    })
    .await;
    source.pair(&[&target]);
    target.pair(&[&source]);
    let chat = uuid::Uuid::new_v4().to_string();
    source
        .submit(&chat, "the selected model is available here")
        .await;
    let old = source.placement(&chat).await;
    let mut request = Box::pin(source.request(
        Method::PUT,
        &chat,
        "/authority-placement",
        json!({"device_id":target.device_id}),
    ));
    tokio::select! {
        result = &mut request => panic!("handoff ended before target preparation: {result:?}"),
        entered = tokio::time::timeout(Duration::from_secs(80), barrier.entered.notified()) => {
            entered.expect("target receives preparation after normal settings sync");
        },
    }
    // The normal settings transfer has completed. Simulate only the target's
    // live provider becoming unavailable; preserve its persisted definitions.
    let provider = target.state.provider_manager.remove("fixture").unwrap();
    barrier.release.notify_one();
    let (status, body) = request.await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert!(
        body["detail"].as_str().unwrap().contains("provider"),
        "{body}"
    );
    assert_eq!(source.placement(&chat).await, old);
    assert_eq!(target.placement(&chat).await, old);
    assert_eq!(
        source
            .store()
            .load_chat_handoff(&chat)
            .await
            .unwrap()
            .unwrap()
            .phase,
        rua_session::chat_handoff::ChatHandoffPhase::Aborted
    );
    target.state.provider_manager.register(provider);
    source
        .submit(&chat, "source can continue after target failure")
        .await;
    assert_eq!(source.captured.requests.lock().await.len(), 2);
    assert!(target.captured.requests.lock().await.is_empty());
    source.close().await;
    target.close().await;
}
