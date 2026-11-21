use super::*;
use rua_contract::Tool;
use rua_core::tool::ToolContext;
use rua_server::services::device_read_check::ForegroundReadAccess;
use serde_json::json;

struct FileOperations {
    fixture: RemoteInputConfirmationFixture,
    address: ChatAddress,
    turn_id: String,
    base: ToolContext,
    cards: tokio::sync::mpsc::UnboundedReceiver<String>,
    token: String,
    forward_events: tokio::task::JoinHandle<()>,
    release: Arc<Notify>,
}

impl FileOperations {
    async fn start(fixture: RemoteInputConfirmationFixture, calls: &[ToolCall]) -> Self {
        let release = Arc::new(Notify::new());
        let (address, turn, epoch, _) = prepare_running_file_calls(&fixture, calls, &release).await;
        let owner = &fixture.target_state;
        let phone = &fixture.source_state;
        let mut policy = phone.security.config_store().load("default").unwrap();
        policy.tool_guard.approval_mode = "ask_first".into();
        phone
            .security
            .save_and_invalidate("default", &policy)
            .unwrap();
        let token = approved_phone_file::logged_in(owner, "fixture-user").await;
        let (event_tx, mut events) = tokio::sync::mpsc::channel(32);
        let (cards_tx, cards) = tokio::sync::mpsc::unbounded_channel();
        let forward_events = tokio::spawn({
            let authority = owner.conversation_authority.clone();
            let address = address.clone();
            let turn_id = turn.turn_id.clone();
            let session_id = rua_core::id::SessionId::from(fixture.chat_id.clone());
            async move {
                while let Some(event) = events.recv().await {
                    for event in react_agent::agent_event_to_runtime_events(event, &session_id) {
                        authority
                            .observe_runtime_event(&address, Some(&turn_id), &event)
                            .await;
                        if let RuntimeEvent::ToolApprovalPending {
                            request_id: Some(id),
                            ..
                        } = event
                        {
                            cards_tx.send(id).unwrap();
                        }
                    }
                }
            }
        });
        let base = ToolContext {
            agent_id: "default".into(),
            session_id: fixture.chat_id.clone(),
            local_device_id: Some(fixture.target.device_id.clone()),
            device_read_access: Some(Arc::new(ForegroundReadAccess {
                authority: Arc::downgrade(&owner.conversation_authority),
                approvals: Arc::downgrade(owner.security.approval_authority()),
                network: Arc::downgrade(&owner.device_network),
            })),
            event_tx: Some(event_tx),
            metadata: [
                (
                    rua_agent::hooks::SECURITY_CHAT_ADDRESS_METADATA_KEY.into(),
                    json!(address),
                ),
                ("turn_id".into(), json!(turn.turn_id)),
                ("chat_configuration_epoch".into(), json!(epoch)),
            ]
            .into(),
            ..Default::default()
        };
        Self {
            fixture,
            address,
            turn_id: turn.turn_id,
            base,
            cards,
            token,
            forward_events,
            release,
        }
    }

    async fn invoke(
        &self,
        call: &ToolCall,
    ) -> (
        tokio::sync::watch::Sender<bool>,
        tokio::task::JoinHandle<rua_core::Result<ToolResult>>,
    ) {
        self.fixture
            .target_state
            .conversation_authority
            .observe_runtime_event(
                &self.address,
                Some(&self.turn_id),
                &RuntimeEvent::ToolCall { call: call.clone() },
            )
            .await;
        let (stop, cancel) = tokio::sync::watch::channel(false);
        let mut ctx = self.base.clone();
        ctx.cancel = Some(cancel);
        ctx.metadata.insert("tool_call_id".into(), json!(call.id));
        let call = call.clone();
        (
            stop,
            tokio::spawn(async move {
                match call.name.as_str() {
                    "read" => rua_tool::ReadFileTool.call(call.arguments, ctx).await,
                    "edit" => rua_tool::EditFileTool.call(call.arguments, ctx).await,
                    "grep" => {
                        rua_tool::extended::search::GrepSearchTool::new()
                            .call(call.arguments, ctx)
                            .await
                    }
                    "glob" => {
                        rua_tool::extended::search::GlobSearchTool::new()
                            .call(call.arguments, ctx)
                            .await
                    }
                    name => panic!("unexpected file tool {name}"),
                }
            }),
        )
    }

    async fn decide(
        &mut self,
        call: &ToolCall,
        operation: &mut tokio::task::JoinHandle<rua_core::Result<ToolResult>>,
        decision: &str,
    ) {
        let request_id = tokio::select! {
            result = operation => panic!("{} finished before the desktop decision: {result:?}", call.id),
            id = tokio::time::timeout(Duration::from_secs(8), self.cards.recv()) => id.unwrap().unwrap(),
        };
        let pending = self
            .fixture
            .target_state
            .security
            .approval_authority()
            .request(&request_id)
            .await
            .unwrap();
        assert_eq!(pending.owner.as_chat_address(), Some(&self.address));
        assert_eq!(pending.turn_id, self.turn_id);
        assert_eq!(pending.tool_call_id, call.id);
        assert_eq!(pending.tool_call.args, call.arguments);
        let response = approved_phone_file::request(
            &self.fixture.target_state,
            Method::POST,
            &format!(
                "/api/chats/{}/approvals/{request_id}/decide",
                self.fixture.chat_id
            ),
            json!({"decision":decision,"decided_by":"webui"}),
            Some(&self.token),
        )
        .await;
        assert_eq!(
            response["authenticated_user_decision"]["decision"],
            decision
        );
    }

    async fn approved(&mut self, call: &ToolCall) -> ToolResult {
        let (_stop, mut operation) = self.invoke(call).await;
        self.decide(call, &mut operation, "run_once").await;
        let result = tokio::time::timeout(Duration::from_secs(8), operation)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!result.is_error, "{}: {}", call.id, result.text_content());
        assert_eq!(
            result.metadata["file_location"]["device_id"],
            self.fixture.source.device_id
        );
        assert_eq!(result.metadata["file_location"]["dispatched"], true);
        let requests = self.fixture.file_reads.requests.lock().unwrap();
        let sent: Vec<_> = requests
            .iter()
            .filter(|r| r.tool_call_id == call.id)
            .collect();
        assert_eq!(sent.len(), 2, "one policy query and one approved execution");
        assert!(sent[0].confirmation.is_none());
        assert!(sent[1].confirmation.is_some());
        assert!(sent
            .iter()
            .all(|r| r.tool_name == call.name && r.arguments == call.arguments));
        result
    }

    async fn finish(self) {
        let snapshot = self
            .fixture
            .source_state
            .security
            .approval_authority()
            .snapshot_at(&self.address)
            .await;
        assert!(
            snapshot.pending.is_empty() && snapshot.resolved.is_empty(),
            "the resource device never asks for its own approval"
        );
        assert!(self
            .fixture
            .target_state
            .security
            .approval_authority()
            .snapshot_at(&self.address)
            .await
            .pending
            .is_empty());
        drop(self.base);
        self.forward_events.await.unwrap();
        self.release.notify_one();
        self.fixture.shutdown().await;
    }
}

fn add_file(device: &str, id: &str, path: &std::path::Path, content: &str) -> ToolCall {
    ToolCall::new(
        id,
        "edit",
        json!({"device_id":device,"patch":format!("*** Begin Patch\n*** Add File: {}\n+{content}\n*** End Patch",path.display())}),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn desktop_approval_runs_directory_search_and_edit_on_the_resource_device() {
    let fixture = RemoteInputConfirmationFixture::new(Duration::ZERO, false).await;
    let files = TempDir::new().unwrap();
    let folder = files.path().canonicalize().unwrap();
    std::fs::write(folder.join("a.txt"), "PHONE_ONLY alpha\n").unwrap();
    std::fs::write(folder.join("b.txt"), "PHONE_ONLY beta\n").unwrap();
    let device = &fixture.source.device_id;
    let created = folder.join("created.txt");
    let calls = [
        ToolCall::new(
            "directory-first",
            "read",
            json!({"device_id":device,"file_path":folder,"limit":1}),
        ),
        ToolCall::new(
            "directory-next",
            "read",
            json!({"device_id":device,"file_path":folder,"limit":1,"after":"a.txt"}),
        ),
        ToolCall::new(
            "grep-first",
            "grep",
            json!({"device_id":device,"path":folder,"pattern":"PHONE_ONLY","max_results":1}),
        ),
        ToolCall::new(
            "grep-next",
            "grep",
            json!({"device_id":device,"path":folder,"pattern":"PHONE_ONLY","max_results":1,"offset":1}),
        ),
        ToolCall::new(
            "glob-first",
            "glob",
            json!({"device_id":device,"path":folder,"pattern":"*.txt","max_results":1}),
        ),
        ToolCall::new(
            "glob-next",
            "glob",
            json!({"device_id":device,"path":folder,"pattern":"*.txt","max_results":1,"offset":1}),
        ),
        add_file(device, "create", &created, "phone new file"),
        ToolCall::new(
            "update",
            "edit",
            json!({"device_id":device,"patch":format!("*** Begin Patch\n*** Update File: {}\n@@\n-phone new file\n+phone updated file\n*** End Patch",created.display())}),
        ),
    ];
    let mut f = FileOperations::start(fixture, &calls).await;
    let first = f.approved(&calls[0]).await;
    assert!(first.text_content().starts_with("a.txt\n"));
    assert_eq!(first.metadata["continuation"]["after"], "a.txt");
    assert_eq!(
        first.metadata["continuation"]["device_id"],
        f.fixture.source.device_id
    );
    let next = f.approved(&calls[1]).await;
    assert!(next.text_content().starts_with("b.txt\n"));
    assert!(!next.text_content().contains("a.txt"));
    assert!(!next.metadata.contains_key("continuation"));
    let first = f.approved(&calls[2]).await;
    assert!(first.text_content().contains("PHONE_ONLY alpha"));
    assert_eq!(first.metadata["next_offset"], 1);
    let next = f.approved(&calls[3]).await;
    assert!(next.text_content().contains("PHONE_ONLY beta"));
    assert!(!next.text_content().contains("PHONE_ONLY alpha"));
    let first = f.approved(&calls[4]).await;
    assert!(first.text_content().contains("a.txt"));
    assert_eq!(first.metadata["next_offset"], 1);
    let next = f.approved(&calls[5]).await;
    assert!(next.text_content().contains("b.txt"));
    assert!(!next.text_content().contains("a.txt"));
    f.approved(&calls[6]).await;
    assert_eq!(
        std::fs::read_to_string(&created).unwrap(),
        "phone new file\n"
    );
    f.approved(&calls[7]).await;
    assert_eq!(
        std::fs::read_to_string(&created).unwrap(),
        "phone updated file\n"
    );
    assert_eq!(
        f.fixture.file_reads.committed_edits.load(Ordering::SeqCst),
        2
    );
    assert!(!f.fixture._target_dir.path().join("created.txt").exists());
    f.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_disabled_and_offline_operations_never_use_local_files() {
    use rua_config::types::tools::{ToolIdentity, ToolSelectionPolicy};
    use rua_security::{ApprovalReuseMatcher, PermissionRuleEffect, PermissionRuleInput};
    let fixture = RemoteInputConfirmationFixture::new(Duration::ZERO, false).await;
    let files = TempDir::new().unwrap();
    let folder = files.path().canonicalize().unwrap();
    let path = folder.join("private.txt");
    let marker = "PHONE_FILE_MUST_NOT_APPEAR";
    std::fs::write(&path, marker).unwrap();
    let device = &fixture.source.device_id;
    let calls = [
        add_file(
            device,
            "user-rejected",
            &folder.join("rejected.txt"),
            "rejected",
        ),
        add_file(
            device,
            "policy-denied",
            &folder.join("denied.txt"),
            "denied",
        ),
        ToolCall::new(
            "disabled-glob",
            "glob",
            json!({"device_id":device,"path":folder,"pattern":"*.txt"}),
        ),
        ToolCall::new(
            "offline-read",
            "read",
            json!({"device_id":device,"file_path":path}),
        ),
        add_file(
            device,
            "offline-edit",
            &folder.join("offline.txt"),
            "must not be written locally",
        ),
    ];
    let mut f = FileOperations::start(fixture, &calls).await;
    let (_stop, mut operation) = f.invoke(&calls[0]).await;
    f.decide(&calls[0], &mut operation, "reject").await;
    let result = operation.await.unwrap().unwrap();
    assert!(result.is_error);
    assert_eq!(
        result.metadata["file_location"]["target_permission_check"],
        "denied"
    );
    assert!(!folder.join("rejected.txt").exists());
    let authority = f.fixture.source_state.security.approval_authority();
    let deny = authority
        .create_agent_rule(
            "default",
            PermissionRuleInput {
                label: "Deny target edit".into(),
                enabled: true,
                effect: PermissionRuleEffect::Deny,
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "edit".into(),
                    input_params: None,
                },
            },
        )
        .await
        .unwrap();
    let (_stop, operation) = f.invoke(&calls[1]).await;
    let result = tokio::time::timeout(Duration::from_secs(8), operation)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(result.is_error);
    assert_eq!(
        result.metadata["file_location"]["target_permission_check"],
        "denied"
    );
    assert!(!folder.join("denied.txt").exists());
    authority
        .delete_agent_rule("default", &deny.id)
        .await
        .unwrap();
    let mut selection = ToolSelectionPolicy::default_enabled();
    selection.set_tool_enabled(ToolIdentity::builtin("glob"), false);
    let store = &f.fixture.source_state.tool_config_store;
    let preview = store
        .preview_selection_policy(Some("default"), selection.clone())
        .await
        .unwrap();
    store
        .set_selection_policy(Some("default"), preview.expected, selection)
        .await
        .unwrap();
    let (_stop, operation) = f.invoke(&calls[2]).await;
    let result = tokio::time::timeout(Duration::from_secs(8), operation)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(result.is_error);
    assert_eq!(
        result.metadata["file_location"]["target_permission_check"],
        "denied"
    );
    assert!(!result.text_content().contains("private.txt"));
    f.fixture.source.runtime.shutdown().await.unwrap();
    for call in &calls[3..] {
        let (_stop, operation) = f.invoke(call).await;
        let result = tokio::time::timeout(Duration::from_secs(25), operation)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(result.is_error);
        assert_eq!(result.metadata["file_location"]["dispatched"], false);
        assert!(!result.text_content().contains(marker));
    }
    assert!(!folder.join("offline.txt").exists());
    assert_eq!(
        f.fixture.file_reads.committed_edits.load(Ordering::SeqCst),
        0
    );
    assert!(f.cards.try_recv().is_err());
    f.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_dispatched_edit_prevents_a_late_request_and_lost_receipt_never_replays() {
    let fixture = RemoteInputConfirmationFixture::new(Duration::ZERO, false).await;
    let files = TempDir::new().unwrap();
    let folder = files.path().canonicalize().unwrap();
    let cancelled = folder.join("cancelled.txt");
    let committed = folder.join("committed.txt");
    let calls = [
        add_file(
            &fixture.source.device_id,
            "cancel-after-dispatch",
            &cancelled,
            "cancelled",
        ),
        add_file(
            &fixture.source.device_id,
            "lost-receipt",
            &committed,
            "written once",
        ),
    ];
    let mut f = FileOperations::start(fixture, &calls).await;
    f.fixture
        .file_reads
        .hold_next_edit
        .store(true, Ordering::SeqCst);
    let (stop, mut operation) = f.invoke(&calls[0]).await;
    f.decide(&calls[0], &mut operation, "run_once").await;
    tokio::time::timeout(
        Duration::from_secs(8),
        f.fixture.file_reads.edit_arrived.notified(),
    )
    .await
    .unwrap();
    stop.send(true).unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(8), operation)
            .await
            .unwrap()
            .unwrap(),
        Err(rua_core::RuaError::Cancelled)
    ));
    tokio::time::timeout(
        Duration::from_secs(8),
        f.fixture.file_reads.cancellation_processed.notified(),
    )
    .await
    .unwrap();
    f.fixture.file_reads.release_edit.notify_one();
    tokio::time::timeout(
        Duration::from_secs(8),
        f.fixture.file_reads.edit_processed.notified(),
    )
    .await
    .unwrap();
    assert!(!cancelled.exists());
    f.fixture
        .file_reads
        .drop_next_edit_reply
        .store(true, Ordering::SeqCst);
    let (_stop, mut operation) = f.invoke(&calls[1]).await;
    f.decide(&calls[1], &mut operation, "run_once").await;
    let result = tokio::time::timeout(Duration::from_secs(25), operation)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(result.is_error);
    assert_eq!(result.metadata["file_location"]["outcome"], "unconfirmed");
    assert_eq!(
        std::fs::read_to_string(&committed).unwrap(),
        "written once\n"
    );
    assert_eq!(
        f.fixture.file_reads.committed_edits.load(Ordering::SeqCst),
        1
    );
    let sent = f
        .fixture
        .file_reads
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.tool_call_id == "lost-receipt")
        .count();
    assert_eq!(
        sent, 2,
        "only the policy query and the original execution; no retry after a lost receipt"
    );
    f.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stopping_turn_does_not_accept_a_late_successful_edit_receipt() {
    use rua_server::services::conversation_authority::ConversationActiveTurnPhase;
    let fixture = RemoteInputConfirmationFixture::new(Duration::ZERO, false).await;
    let files = TempDir::new().unwrap();
    let committed = files.path().canonicalize().unwrap().join("committed.txt");
    let call = add_file(
        &fixture.source.device_id,
        "late-edit",
        &committed,
        "already committed",
    );
    let mut f = FileOperations::start(fixture, std::slice::from_ref(&call)).await;
    f.fixture
        .file_reads
        .hold_next_edit_reply
        .store(true, Ordering::SeqCst);
    let (stop, mut operation) = f.invoke(&call).await;
    f.decide(&call, &mut operation, "run_once").await;
    tokio::time::timeout(
        Duration::from_secs(8),
        f.fixture.file_reads.edit_reply_ready.notified(),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(&committed).unwrap(),
        "already committed\n"
    );
    // Model the interval between publishing Stopping and delivering cancellation to tools.
    let stopping = f
        .fixture
        .target_state
        .conversation_authority
        .request_stop(None, "console", &f.address)
        .await
        .unwrap();
    assert_eq!(stopping.phase, ConversationActiveTurnPhase::Stopping);
    assert!(!*stop.borrow());
    f.fixture.file_reads.release_edit_reply.notify_one();
    let result = tokio::time::timeout(Duration::from_secs(8), operation)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(result.is_error);
    assert_eq!(result.metadata["file_location"]["outcome"], "unconfirmed");
    assert_eq!(
        f.fixture.file_reads.committed_edits.load(Ordering::SeqCst),
        1
    );
    f.finish().await;
}
