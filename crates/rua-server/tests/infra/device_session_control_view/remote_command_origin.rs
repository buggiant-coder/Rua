use super::*;
use rua_agent::runtime::agents_instructions::TurnAgentsInstructions;
use rua_contract::Tool;
use rua_core::agents_instructions::{
    AgentsInstructionDecision, AgentsInstructionTarget, AgentsInstructions,
};
use rua_core::tool::ToolContext;
use rua_security::{ApprovalReuseMatcher, PermissionRuleEffect, PermissionRuleInput};
use rua_server::services::chat_command::ChatCommandAction;
use rua_server::services::chat_command_tools::{
    ChatCommandToolService, RemoteCommandRuntimeAdapter,
};
use rua_server::services::chat_file_read_admission::TargetFileReadContext;
use rua_server::services::project_instructions::RuntimeAgentsInstructionLoader;
use rua_tool::core::shell::RemoteCommandRuntimeHandle;
use serde_json::json;
use std::sync::OnceLock;

#[path = "remote_command_origin/dropped_adoption.rs"]
mod dropped_adoption;

#[path = "remote_command_origin/task_approval_permissions.rs"]
mod task_approval_permissions;

struct Origin {
    fixture: RemoteInputConfirmationFixture,
    ctx: ToolContext,
    release: Arc<Notify>,
}

impl Origin {
    async fn new(fixture: RemoteInputConfirmationFixture, calls: &[ToolCall]) -> Self {
        let release = Arc::new(Notify::new());
        let (address, turn, _) = prepare_running_file_calls(&fixture, calls, &release).await;
        let owner = &fixture.target_state;
        owner.sync_enabled.store(true, Ordering::SeqCst);
        for call in calls {
            fixture
                .source_state
                .security
                .approval_authority()
                .create_agent_rule(
                    "default",
                    PermissionRuleInput {
                        label: "Permit the fixture command on its selected Device".into(),
                        enabled: true,
                        effect: PermissionRuleEffect::Allow,
                        matcher: ApprovalReuseMatcher::ToolCall {
                            tool_name: "bash".into(),
                            input_params: Some(if call.name == "task_create" {
                                call.arguments["runner"]["arguments"].to_string()
                            } else {
                                call.arguments.to_string()
                            }),
                        },
                    },
                )
                .await
                .unwrap();
        }
        let tasks = Arc::new(OnceLock::new());
        assert!(tasks
            .set(Arc::new(
                rua_server::services::chat_task::ChatTaskService::new(
                    owner.rua_root.clone(),
                    fixture.target.device_id.clone(),
                    owner.task_runtime.clone(),
                    Arc::new(
                        rua_server::services::task_authority::ServerTaskAuthorityResolver::new(
                            owner.rua_root.clone(),
                            "desktop",
                            owner.sync_device.clone(),
                        )
                    ),
                    Arc::new(TargetFileReadContext::new(
                        owner.rua_root.clone(),
                        fixture.target.device_id.clone(),
                        owner.project_catalog.clone(),
                        owner.agent_service.clone(),
                        owner.tool_catalog.clone(),
                        owner.tool_config_store.clone(),
                        owner.security.clone(),
                    )),
                    owner.device_network.clone(),
                    owner.conversation_authority.clone(),
                )
            ))
            .is_ok());
        let service = Arc::new(ChatCommandToolService::new(
            owner.sync_enabled.clone(),
            owner.sync_device.clone(),
            owner.device_network.clone(),
            owner.conversation_authority.clone(),
            owner.security.approval_authority().clone(),
            tasks.clone(),
        ));
        let binding = OnceLock::new();
        assert!(binding.set(service).is_ok());
        let ctx = ToolContext {
            agent_id: "default".into(),
            session_id: fixture.chat_id.clone(),
            local_device_id: Some(fixture.target.device_id.clone()),
            working_dir: owner.rua_root.canonicalize().unwrap(),
            tool_result_store_dir: Some(owner.rua_root.join("origin-tool-results")),
            task_runtime: Some(Arc::new(
                rua_tool::extended::task_tools::TaskRuntimeHandle::new(owner.task_runtime.clone())
                    .with_router(Arc::new(
                        rua_server::services::chat_task_tools::RuntimeTaskToolRouter::new(tasks),
                    )),
            )),
            remote_command_runtime: Some(Arc::new(RemoteCommandRuntimeHandle::new(Arc::new(
                RemoteCommandRuntimeAdapter::new(Arc::new(binding)),
            )))),
            metadata: [
                (
                    rua_agent::hooks::SECURITY_CHAT_ADDRESS_METADATA_KEY.into(),
                    json!(address),
                ),
                ("turn_id".into(), json!(turn.turn_id)),
            ]
            .into(),
            ..Default::default()
        };
        Self {
            fixture,
            ctx,
            release,
        }
    }

    async fn instructions(&self) -> Arc<TurnAgentsInstructions> {
        let owner = &self.fixture.target_state;
        let local = OnceLock::new();
        assert!(local
            .set(Arc::new(TargetFileReadContext::new(
                owner.rua_root.clone(),
                self.fixture.target.device_id.clone(),
                owner.project_catalog.clone(),
                owner.agent_service.clone(),
                owner.tool_catalog.clone(),
                owner.tool_config_store.clone(),
                owner.security.clone()
            )))
            .is_ok());
        Arc::new(
            TurnAgentsInstructions::prepare(
                Arc::new(RuntimeAgentsInstructionLoader {
                    local: Arc::new(local),
                    authority: Arc::downgrade(&owner.conversation_authority),
                    network: Arc::downgrade(&owner.device_network),
                }),
                None,
                &self.ctx,
                CancellationToken::new(),
            )
            .await
            .unwrap(),
        )
    }

    fn context(&self, call: &ToolCall) -> ToolContext {
        let mut context = self.ctx.clone();
        context
            .metadata
            .insert("tool_call_id".into(), json!(call.id));
        context
    }

    async fn shutdown(self) {
        self.release.notify_one();
        self.fixture.shutdown().await;
    }
}

struct RejectAfterPreparation {
    approvals: Arc<rua_security::ApprovalAuthority>,
    arguments: serde_json::Value,
}

#[async_trait::async_trait]
impl AgentsInstructions for RejectAfterPreparation {
    fn snapshot_for_model(&self) -> Option<String> {
        None
    }

    async fn before_tool(
        &self,
        _call: &ToolCall,
        _ctx: &ToolContext,
        _cancellation: CancellationToken,
    ) -> rua_core::Result<AgentsInstructionDecision> {
        Ok(AgentsInstructionDecision::Continue)
    }

    async fn before_targets(
        &self,
        _targets: &[AgentsInstructionTarget],
        _ctx: &ToolContext,
        _cancellation: CancellationToken,
    ) -> rua_core::Result<AgentsInstructionDecision> {
        self.approvals
            .create_agent_rule(
                "default",
                PermissionRuleInput {
                    label: "User revokes the command before launch".into(),
                    enabled: true,
                    effect: PermissionRuleEffect::Deny,
                    matcher: ApprovalReuseMatcher::ToolCall {
                        tool_name: "bash".into(),
                        input_params: Some(self.arguments.to_string()),
                    },
                },
            )
            .await
            .unwrap();
        Ok(AgentsInstructionDecision::Continue)
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn origin_reports_a_rejected_start_as_not_executed_and_releases_preparation() {
    let fixture = RemoteInputConfirmationFixture::new(Duration::ZERO, false).await;
    let cwd = fixture.source_state.rua_root.canonicalize().unwrap();
    let call = ToolCall::new(
        "rejected-start",
        "bash",
        json!({"device_id":fixture.source.device_id,"cwd":cwd,
        "command":"printf unexpected > must-not-exist"}),
    );
    let mut f = Origin::new(fixture, std::slice::from_ref(&call)).await;
    f.ctx.agents_instructions = Some(Arc::new(RejectAfterPreparation {
        approvals: f.fixture.source_state.security.approval_authority().clone(),
        arguments: call.arguments.clone(),
    }));
    let shell = rua_tool::ShellTool::new(Arc::new(rua_tool::RuntimeCommandExecutor::new()));
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        shell.call(call.arguments.clone(), f.context(&call)),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(result.is_error, "{}", result.text_content());
    assert_eq!(result.metadata["execution_status"], "not_executed");
    assert!(!result.metadata.contains_key("execution_state"));
    assert!(result.text_content().contains("Denied"));
    assert!(!cwd.join("must-not-exist").exists());
    {
        let requests = f.fixture.file_reads.command_requests.lock().unwrap();
        assert_eq!(
            requests.iter().map(|r| r.action).collect::<Vec<_>>(),
            vec![
                ChatCommandAction::Prepare,
                ChatCommandAction::Start,
                ChatCommandAction::Cancel
            ]
        );
    }
    f.shutdown().await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn origin_target_instructions_cancel_preparation_before_spawn_then_allow_the_reviewed_call() {
    let fixture = RemoteInputConfirmationFixture::new(Duration::ZERO, false).await;
    let cwd = fixture
        .source_state
        .rua_root
        .canonicalize()
        .unwrap()
        .join("command-scope");
    std::fs::create_dir_all(cwd.join(".git")).unwrap();
    std::fs::write(
        cwd.join("AGENTS.md"),
        "PHONE_CWD_REQUIRES_REVIEW_BEFORE_COMMAND",
    )
    .unwrap();
    let args = json!({"device_id":fixture.source.device_id,"cwd":cwd,"command":"printf once >> starts; printf target-complete"});
    let calls = [
        ToolCall::new("command-before-review", "bash", args.clone()),
        ToolCall::new("command-after-review", "bash", args),
    ];
    let mut f = Origin::new(fixture, &calls).await;
    let coordinator = f.instructions().await;
    assert!(!coordinator
        .snapshot_for_model()
        .unwrap_or_default()
        .contains("PHONE_CWD_REQUIRES_REVIEW_BEFORE_COMMAND"));
    f.ctx.agents_instructions = Some(coordinator.clone());
    let shell = rua_tool::ShellTool::new(Arc::new(rua_tool::RuntimeCommandExecutor::new()));
    let first = tokio::time::timeout(
        Duration::from_secs(15),
        shell.call(calls[0].arguments.clone(), f.context(&calls[0])),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(first.metadata["execution_status"], "not_executed");
    assert_eq!(first.metadata["agents_instructions"], "reconsider");
    assert!(!cwd.join("starts").exists());
    {
        let requests = f.fixture.file_reads.command_requests.lock().unwrap();
        let before: Vec<_> = requests
            .iter()
            .filter(|r| r.context.call.tool_call_id == calls[0].id)
            .collect();
        assert!(before
            .iter()
            .any(|r| r.action == ChatCommandAction::Prepare));
        assert!(before.iter().any(|r| r.action == ChatCommandAction::Cancel));
        assert!(!before.iter().any(|r| r.action == ChatCommandAction::Start));
    }
    assert!(coordinator
        .snapshot_for_model()
        .unwrap()
        .contains("PHONE_CWD_REQUIRES_REVIEW_BEFORE_COMMAND"));
    let second = tokio::time::timeout(
        Duration::from_secs(15),
        shell.call(calls[1].arguments.clone(), f.context(&calls[1])),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!second.is_error, "{}", second.text_content());
    assert_eq!(second.metadata["execution_state"], "succeeded");
    assert!(second.text_content().contains("Exit code: 0"));
    assert_eq!(
        second.metadata["execution_location"],
        json!({"device_id":f.fixture.source.device_id,"cwd":cwd})
    );
    assert_eq!(std::fs::read_to_string(cwd.join("starts")).unwrap(), "once");
    assert!(!second.metadata.contains_key("raw_capture"));
    assert!(!second.metadata.contains_key("live_output_path"));
    f.shutdown().await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_remote_task_discovers_instructions_then_retries_on_its_original_device() {
    use rua_tool::extended::task_tools::{
        TaskCreateTool, TaskListTool, TaskReadTool, TaskRetryTool,
    };
    let fixture = RemoteInputConfirmationFixture::new(Duration::ZERO, false).await;
    let cwd = fixture
        .source_state
        .rua_root
        .canonicalize()
        .unwrap()
        .join("explicit-background");
    std::fs::create_dir(&cwd).unwrap();
    std::fs::write(cwd.join("AGENTS.md"), "BACKGROUND_TARGET_RULE").unwrap();
    let arguments = json!({"runner":{"type":"tool","tool_name":"bash","arguments":{
        "device_id":fixture.source.device_id,"cwd":cwd,
        "command":"printf x >> starts; if [ -f ran ]; then printf second-attempt; else touch ran; printf first-attempt; exit 3; fi"
    }}});
    let first = ToolCall::new("background-discovery", "task_create", arguments.clone());
    let second = ToolCall::new("background-accepted", "task_create", arguments.clone());
    let mut invalid_arguments = arguments.clone();
    invalid_arguments["runner"]["arguments"]["cwd"] = json!(42);
    let invalid = ToolCall::new("background-invalid-cwd", "task_create", invalid_arguments);
    let retry = ToolCall::new(
        "background-retry",
        "task_retry",
        json!({"task_id":"resolved-after-create"}),
    );
    let missing_cwd_retry = ToolCall::new(
        "background-retry-missing-cwd",
        "task_retry",
        json!({"task_id":"resolved-after-create"}),
    );
    let f = Origin::new(
        fixture,
        &[
            invalid.clone(),
            first.clone(),
            second.clone(),
            missing_cwd_retry.clone(),
            retry.clone(),
        ],
    )
    .await;
    let error = TaskCreateTool
        .call(invalid.arguments.clone(), f.context(&invalid))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("cwd"), "{error}");
    assert!(f
        .fixture
        .source_state
        .task_runtime
        .list(
            &rua_task::TaskOwner::new(&f.fixture.chat_id),
            Default::default()
        )
        .await
        .unwrap()
        .is_empty());
    assert!(!cwd.join("starts").exists());
    let coordinator = f.instructions().await;
    coordinator.snapshot_for_model();
    let mut context = f.context(&first);
    context.agents_instructions = Some(coordinator.clone());
    let decision = TaskCreateTool
        .call(arguments.clone(), context)
        .await
        .unwrap();
    assert_eq!(decision.metadata["execution_status"], "not_executed");
    assert!(!cwd.join("starts").exists());
    assert!(coordinator
        .snapshot_for_model()
        .unwrap()
        .contains("BACKGROUND_TARGET_RULE"));
    let mut context = f.context(&second);
    context.agents_instructions = Some(coordinator.clone());
    let created = TaskCreateTool.call(arguments, context).await.unwrap();
    let created: serde_json::Value = serde_json::from_str(&created.text_content()).unwrap();
    let id = created["task_id"].as_str().unwrap();
    let owner = rua_task::TaskOwner::new(&f.fixture.chat_id);
    let failed = f
        .fixture
        .source_state
        .task_runtime
        .await_terminal(&owner, id, Duration::from_secs(8))
        .await
        .unwrap();
    assert_eq!(failed.state, rua_task::TaskState::Failed);
    assert_eq!(std::fs::read_to_string(cwd.join("starts")).unwrap(), "x");
    let current_header =
        WorkspaceSessionStore::new(f.fixture.target_state.chat_workspace().as_path())
            .load_chat_header(&f.fixture.chat_id)
            .await
            .unwrap()
            .unwrap();
    let current_directory: rua_session::ChatExecutionDirectory = serde_json::from_value(
        current_header.extra["hook_configuration"]["capability_configuration"]
            ["execution_directory"]
            .clone(),
    )
    .unwrap();
    assert_eq!(current_directory.device_id, f.fixture.target.device_id);
    assert!(current_directory.path.is_dir());
    assert_ne!(current_directory.path, cwd);
    let unavailable_cwd = cwd.with_file_name("explicit-background-moved");
    std::fs::rename(&cwd, &unavailable_cwd).unwrap();
    let fallback = f.context(&missing_cwd_retry);
    assert!(fallback.working_dir.is_dir());
    assert_ne!(fallback.working_dir, cwd);
    assert!(!fallback.working_dir.join("starts").exists());
    let rejected = TaskRetryTool.call(json!({"task_id":id}), fallback).await;
    // Restore the fixture before asserting so a failing regression leaves no
    // relocated execution directory behind.
    std::fs::rename(&unavailable_cwd, &cwd).unwrap();
    let error = rejected.expect_err("Retry must not replace its missing original cwd");
    assert!(error.to_string().contains("task_unavailable"), "{error}");
    assert!(error.to_string().contains(cwd.to_str().unwrap()), "{error}");
    let unchanged = f
        .fixture
        .source_state
        .task_runtime
        .get(&owner, id)
        .await
        .unwrap();
    assert_eq!(unchanged.attempt_no, 1);
    assert_eq!(unchanged.attempts.len(), 1);
    assert_eq!(unchanged.spec, failed.spec);
    assert_eq!(unchanged.attempts[0].device, failed.attempts[0].device);
    assert_eq!(std::fs::read_to_string(cwd.join("starts")).unwrap(), "x");
    assert!(!f.ctx.working_dir.join("starts").exists());
    assert!(f
        .fixture
        .target_state
        .task_runtime
        .list(&owner, Default::default())
        .await
        .unwrap()
        .is_empty());
    let mut context = f.context(&retry);
    context.agents_instructions = Some(coordinator);
    context.working_dir = f.fixture.target_state.rua_root.clone();
    let retried = TaskRetryTool
        .call(json!({"task_id":id}), context)
        .await
        .unwrap();
    let retried: serde_json::Value = serde_json::from_str(&retried.text_content()).unwrap();
    assert_eq!(retried["attempt_no"], 2);
    let completed = f
        .fixture
        .source_state
        .task_runtime
        .await_terminal(&owner, id, Duration::from_secs(8))
        .await
        .unwrap();
    assert_eq!(completed.state, rua_task::TaskState::Completed);
    assert_eq!(
        completed.spec.origin.tool_call_id.as_deref(),
        Some(second.id.as_str())
    );
    assert_eq!(
        completed.attempts[1]
            .authorization_origin
            .tool_call_id
            .as_deref(),
        Some(retry.id.as_str())
    );
    assert!(completed
        .attempts
        .iter()
        .all(|attempt| attempt.device.device_id.as_deref()
            == Some(f.fixture.source.device_id.as_str())
            && attempt.device.public_cwd.as_deref() == cwd.to_str()));
    assert_eq!(std::fs::read_to_string(cwd.join("starts")).unwrap(), "xx");
    let listing = TaskListTool.call(json!({}), f.ctx.clone()).await.unwrap();
    let listing: serde_json::Value = serde_json::from_str(&listing.text_content()).unwrap();
    assert_eq!(listing["tasks"][0]["task_id"], id);
    assert_eq!(
        listing["tasks"][0]["execution"]["device_id"],
        f.fixture.source.device_id
    );
    let read = TaskReadTool
        .call(json!({"task_id":id,"max_bytes":32}), f.ctx.clone())
        .await
        .unwrap();
    let read: serde_json::Value = serde_json::from_str(&read.text_content()).unwrap();
    assert_eq!(read["task_id"], id);
    assert!(read["cursor"].is_string());
    assert!(f
        .fixture
        .target_state
        .task_runtime
        .list(&owner, Default::default())
        .await
        .unwrap()
        .is_empty());
    f.shutdown().await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn origin_recovers_lost_adopt_receipt_into_the_same_target_task() {
    let fixture = RemoteInputConfirmationFixture::new(Duration::ZERO, false).await;
    let cwd = fixture
        .source_state
        .rua_root
        .canonicalize()
        .unwrap()
        .join("adopt-output");
    std::fs::create_dir(&cwd).unwrap();
    let call = ToolCall::new(
        "lost-adopt",
        "bash",
        json!({
            "device_id":fixture.source.device_id,"cwd":cwd,"yield_seconds":1,
            "command":"printf once >> starts; while [ ! -f release ]; do sleep 0.05; done; printf adopted-completed"
        }),
    );
    let f = Origin::new(fixture, std::slice::from_ref(&call)).await;
    f.fixture
        .file_reads
        .drop_next_command_adopt_reply
        .store(true, Ordering::SeqCst);
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        rua_tool::ShellTool::new(Arc::new(rua_tool::RuntimeCommandExecutor::new()))
            .call(call.arguments.clone(), f.context(&call)),
    )
    .await
    .unwrap()
    .unwrap();
    let task_id = result.metadata["background_task_id"]
        .as_str()
        .expect("target accepted Task");
    assert_eq!(result.metadata["execution_id"], task_id);
    let owner = rua_task::TaskOwner::new(&f.fixture.chat_id);
    assert!(f
        .fixture
        .target_state
        .task_runtime
        .list(&owner, Default::default())
        .await
        .unwrap()
        .is_empty());
    let record = f
        .fixture
        .source_state
        .task_runtime
        .get(&owner, task_id)
        .await
        .unwrap();
    assert_eq!(
        record.attempts[0].device.device_id.as_deref(),
        Some(f.fixture.source.device_id.as_str())
    );
    assert_eq!(
        record.attempts[0].device.public_cwd.as_deref(),
        cwd.to_str()
    );
    assert_eq!(std::fs::read_to_string(cwd.join("starts")).unwrap(), "once");
    {
        let requests = f.fixture.file_reads.command_requests.lock().unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.action == ChatCommandAction::Start)
                .count(),
            1
        );
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.action == ChatCommandAction::Adopt)
                .count(),
            1
        );
        assert!(requests
            .iter()
            .all(|request| request.context.execution_id == task_id));
        assert!(requests
            .iter()
            .any(|request| request.action == ChatCommandAction::Status));
    }
    let scope =
        rua_core::ChatStore::resolve(&f.fixture.target_state.rua_root, owner.chat_address.clone());
    let reference = rua_session::task_refs::ChatTaskRef::load(
        &scope.root().join("sessions").join(&f.fixture.chat_id),
        &f.fixture.chat_id,
        task_id,
    )
    .unwrap()
    .unwrap();
    assert!(reference.last_target_evidence.is_some());
    std::fs::write(cwd.join("release"), "continue").unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let record = f
                .fixture
                .source_state
                .task_runtime
                .get(&owner, task_id)
                .await
                .unwrap();
            if record.state.is_terminal() {
                assert_eq!(record.state, rua_task::TaskState::Completed);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    f.shutdown().await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn origin_recovers_a_lost_start_receipt_by_query_without_restarting_the_process() {
    let fixture = RemoteInputConfirmationFixture::new(Duration::ZERO, false).await;
    let cwd = fixture
        .source_state
        .rua_root
        .canonicalize()
        .unwrap()
        .join("command-output");
    std::fs::create_dir(&cwd).unwrap();
    let call = ToolCall::new(
        "lost-start",
        "bash",
        json!({"device_id":fixture.source.device_id,"cwd":cwd,
        "command":"printf once >> starts; sleep 0.4; printf target-finished","yield_seconds":1}),
    );
    let f = Origin::new(fixture, std::slice::from_ref(&call)).await;
    f.fixture
        .file_reads
        .drop_next_command_start_reply
        .store(true, Ordering::SeqCst);
    let definition = rua_tool::ShellTool::spec().to_tool_definition();
    let schema = definition.parameters.clone();
    let mut toolkit = react_agent::AgentToolkit::new();
    toolkit.register(
        definition,
        Arc::new(
            react_agent::ContractToolExecutor::new(Arc::new(rua_tool::ShellTool::new(Arc::new(
                rua_tool::RuntimeCommandExecutor::new(),
            ))))
            .with_schema(schema),
        ),
    );
    let hook: Arc<dyn react_agent::AgentHook> = Arc::new(react_agent::hooks::NoopHook);
    let (events, _receiver) = tokio::sync::mpsc::channel(16);
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        react_agent::react_loop::execute_single_tool(
            &call,
            &toolkit,
            &hook,
            &f.context(&call),
            &react_agent::ToolDisclosureConfig::default(),
            0,
            &CancellationToken::new(),
            Duration::from_millis(100),
            &events,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!result.is_error, "{}", result.text_content());
    assert_eq!(result.metadata["execution_state"], "succeeded");
    assert!(result.text_content().contains("Exit code: 0"));
    assert!(result.text_content().contains("target-finished"));
    assert!(!result.metadata.contains_key("background_task_id"));
    assert_eq!(std::fs::read_to_string(cwd.join("starts")).unwrap(), "once");
    {
        let requests = f.fixture.file_reads.command_requests.lock().unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|r| r.action == ChatCommandAction::Start)
                .count(),
            1
        );
        assert!(requests
            .iter()
            .any(|r| r.action == ChatCommandAction::Status));
        let id = result.metadata["execution_id"].as_str().unwrap();
        assert!(requests.iter().all(|r| r.context.execution_id == id));
    }
    f.shutdown().await;
}
