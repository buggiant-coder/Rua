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

struct Origin {
    fixture: RemoteInputConfirmationFixture,
    ctx: ToolContext,
    release: Arc<Notify>,
}

impl Origin {
    async fn new(fixture: RemoteInputConfirmationFixture, calls: &[ToolCall]) -> Self {
        let release = Arc::new(Notify::new());
        let (address, turn, epoch, _) = prepare_running_file_calls(&fixture, calls, &release).await;
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
                            input_params: Some(call.arguments.to_string()),
                        },
                    },
                )
                .await
                .unwrap();
        }
        let service = Arc::new(ChatCommandToolService::new(
            owner.sync_enabled.clone(),
            owner.sync_device.clone(),
            owner.device_network.clone(),
            owner.conversation_authority.clone(),
            owner.security.approval_authority().clone(),
        ));
        let binding = OnceLock::new();
        assert!(binding.set(service).is_ok());
        let ctx = ToolContext {
            agent_id: "default".into(),
            session_id: fixture.chat_id.clone(),
            local_device_id: Some(fixture.target.device_id.clone()),
            working_dir: owner.rua_root.canonicalize().unwrap(),
            tool_result_store_dir: Some(owner.rua_root.join("origin-tool-results")),
            remote_command_runtime: Some(Arc::new(RemoteCommandRuntimeHandle::new(Arc::new(
                RemoteCommandRuntimeAdapter::new(Arc::new(binding)),
            )))),
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
