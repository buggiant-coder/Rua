use super::handoff::{HeldTaskRunner, Peer};
use super::*;
use rua_sync::identity::NetworkSecret;
use rua_sync::inbound_inbox::InboxState;
use serde_json::{json, Value};

fn assert_unrelated_pending(peer: &Peer, author: iroh_docs::AuthorId, key: &[u8]) {
    let entry = peer
        .state
        .inbound_inbox
        .pending_ready(u64::MAX)
        .unwrap()
        .into_iter()
        .find(|entry| entry.author == author && entry.key == key)
        .expect("unrelated B entry remains pending, never falsely applied or discarded");
    assert_eq!(entry.state, InboxState::Retryable);
    assert_eq!(
        entry.attempts, 1,
        "future backoff must not download unrelated content"
    );
    assert_eq!(
        entry.last_error.as_deref(),
        Some("entry content not readable yet")
    );
    let now = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap();
    assert!(peer
        .state
        .inbound_inbox
        .pending_ready(now)
        .unwrap()
        .iter()
        .all(|entry| entry.author != author || entry.key != key));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unrelated_offline_device_inbox_retry_does_not_block_ready_chat_handoff() {
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
            rua_task::TaskAttemptAuthorization::new("default").unwrap(),
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
    // Complete the relevant Chat/Task projection through normal sync before
    // introducing another Chat's unavailable log content. The entry uses B's
    // actual authenticated author; it is not a readiness dependency for this Chat.
    target.synchronize(&source).await;
    let unrelated_chat = uuid::Uuid::new_v4().to_string();
    let key = format!("unavailable-chat-log-{unrelated_chat}").into_bytes();
    let author = executor
        .state
        .sync_device
        .read()
        .unwrap()
        .as_ref()
        .unwrap()
        .to_iroh_author()
        .id();
    let deadline = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap()
        + Duration::from_secs(600).as_millis() as u64;
    for peer in [&source, &target] {
        let _apply = peer.state.inbound_apply_lock.lock().await;
        peer.state
            .inbound_inbox
            .record_remote_insert(
                author,
                &key,
                iroh_blobs::Hash::new(b"unavailable content from another Chat"),
            )
            .unwrap();
        peer.state
            .inbound_inbox
            .mark_retryable(author, &key, "entry content not readable yet", deadline)
            .unwrap();
        assert_unrelated_pending(peer, author, &key);
    }
    executor.close().await;
    let (status, body) = source
        .request(
            Method::PUT,
            &chat,
            "/authority-placement",
            json!({"device_id":target.device_id}),
        )
        .await;
    for peer in [&source, &target] {
        assert_unrelated_pending(peer, author, &key);
    }
    if status != StatusCode::OK {
        // The negative reproduction must still release the real held Task and
        // network fixtures before reporting the handoff failure.
        executor
            .state
            .task_runtime
            .cancel(&owner, &task_id)
            .await
            .unwrap();
        source.close().await;
        target.close().await;
    }
    assert_eq!(status, StatusCode::OK, "necessary Chat/Task state is synchronized; unrelated B backlog must not block A -> C: {body}");
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
    target
        .submit(&chat, "continue despite the unrelated unavailable record")
        .await;
    assert_eq!(target.placement(&chat).await.device_id, target.device_id);
    for peer in [&source, &target] {
        assert_unrelated_pending(peer, author, &key);
    }
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
