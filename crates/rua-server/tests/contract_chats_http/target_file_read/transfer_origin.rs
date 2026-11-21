use super::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use rua_agent::runtime::agents_instructions::TurnAgentsInstructions;
use rua_contract::Tool;
use rua_core::agents_instructions::{
    AgentsInstructionDecision, AgentsInstructionTarget, AgentsInstructions,
};
use rua_core::tool::ToolContext;
use rua_server::services::conversation_authority::{
    ConversationActiveTurnPhase, ConversationActiveTurnStart, ConversationDriverKind,
};
use rua_server::services::project_instructions::RuntimeAgentsInstructionLoader;
use rua_server::services::sync_transfer_tools::{
    SyncTransferRuntimeAdapter, SyncTransferToolService,
};
use rua_tool::extended::sync_transfer::{SyncTransferRuntimeHandle, SyncTransferTool};
use tokio_util::sync::CancellationToken;

struct Origin {
    app: TestApp,
    ctx: ToolContext,
    source: PathBuf,
    destination: PathBuf,
}

impl Origin {
    async fn new() -> Self {
        let app = build_agent().await;
        let chat = seed_materialized_chat(&app, AGENT, "Explicit transfer").await;
        let address = ChatAddress::new(chat["id"].as_str().unwrap());
        let device = current_sync_device_id(&app.state.sync_device).unwrap();
        let placement = parse_conversation_authority_placement(&chat["meta"]).unwrap();
        let turn = app
            .state
            .conversation_authority
            .start_active_turn(ConversationActiveTurnStart {
                address: address.clone(),
                driver: ConversationDriverKind::Rua,
                authority_device_id: device.clone(),
                authority_epoch: placement.authority_epoch,
                driver_detail: None,
            })
            .await;
        app.state
            .conversation_authority
            .transition_active_turn_phase(
                &address,
                Some(&turn.turn_id),
                ConversationActiveTurnPhase::RunningTool,
            )
            .await
            .unwrap();
        let root = app.state.rua_root.canonicalize().unwrap();
        let cwd = root.join("execution");
        let source = root.join("selected");
        let destination = root.join("received");
        for path in [&cwd, &source, &destination] {
            std::fs::create_dir_all(path).unwrap();
        }
        let service = Arc::new(SyncTransferToolService::new(
            app.state.sync_enabled.clone(),
            app.state.sync_device.clone(),
            app.state.device_network.clone(),
            app.state.chat_transfer.clone(),
            app.state.conversation_authority.clone(),
            app.state.security.approval_authority().clone(),
        ));
        let binding = OnceLock::new();
        assert!(binding.set(service).is_ok());
        let ctx = ToolContext {
            agent_id: AGENT.into(),
            session_id: address.chat_id().to_string(),
            local_device_id: Some(device),
            working_dir: cwd,
            metadata: [
                (
                    rua_agent::hooks::SECURITY_CHAT_ADDRESS_METADATA_KEY.into(),
                    json!(address),
                ),
                ("turn_id".into(), json!(turn.turn_id)),
                ("tool_call_id".into(), json!("first-copy")),
                (
                    "chat_configuration_epoch".into(),
                    chat["meta"]["hook_configuration"]["epoch"].clone(),
                ),
            ]
            .into(),
            sync_transfer_runtime: Some(Arc::new(SyncTransferRuntimeHandle::new(Arc::new(
                SyncTransferRuntimeAdapter::new(Arc::new(binding)),
            )))),
            ..Default::default()
        };
        Self {
            app,
            ctx,
            source,
            destination,
        }
    }

    fn arguments(&self, source: &Path) -> Value {
        json!({"source_device_id":self.ctx.local_device_id,"source_paths":[source],
            "target_device_id":self.ctx.local_device_id,"target_dir":self.destination})
    }

    async fn instructions(&self) -> Arc<TurnAgentsInstructions> {
        let state = &self.app.state;
        let local = OnceLock::new();
        assert!(local
            .set(Arc::new(TargetFileReadContext::new(
                state.rua_root.clone(),
                self.ctx.local_device_id.clone().unwrap(),
                state.project_catalog.clone(),
                state.agent_service.clone(),
                state.tool_catalog.clone(),
                state.tool_config_store.clone(),
                state.security.clone(),
            )))
            .is_ok());
        let loader = Arc::new(RuntimeAgentsInstructionLoader {
            local: Arc::new(local),
            authority: Arc::downgrade(&state.conversation_authority),
            network: Arc::downgrade(&state.device_network),
        });
        Arc::new(
            TurnAgentsInstructions::prepare(
                loader,
                Some("Initial model context".into()),
                &self.ctx,
                CancellationToken::new(),
            )
            .await
            .unwrap(),
        )
    }

    async fn copy(&self, args: Value) -> rua_core::tool::ToolResult {
        tokio::time::timeout(
            Duration::from_secs(15),
            SyncTransferTool.call(args, self.ctx.clone()),
        )
        .await
        .unwrap()
        .unwrap()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_transfer_uses_target_subdirectory_instructions_before_copy_and_returns_verified_paths(
) {
    let mut f = Origin::new().await;
    let source = f.source.join("album/note.txt");
    let target = f.destination.join("selected/album/note.txt");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::create_dir(f.destination.join(".git")).unwrap();
    std::fs::write(&source, "selected source contents").unwrap();
    std::fs::write(
        target.parent().unwrap().join("AGENTS.md"),
        "REVIEW_ALBUM_DESTINATION_BEFORE_COPY",
    )
    .unwrap();
    let instructions = f.instructions().await;
    assert!(!instructions
        .snapshot_for_model()
        .unwrap()
        .contains("REVIEW_ALBUM_DESTINATION_BEFORE_COPY"));
    let args = f.arguments(&f.source);
    assert_eq!(
        instructions
            .before_tool(
                &agent_types::ToolCall::new("first-copy", "sync_send", args.clone()),
                &f.ctx,
                CancellationToken::new()
            )
            .await
            .unwrap(),
        AgentsInstructionDecision::Continue
    );
    f.ctx.agents_instructions = Some(instructions.clone());
    let first = f.copy(args.clone()).await;
    assert_eq!(first.metadata["execution_status"], "not_executed");
    assert_eq!(first.metadata["agents_instructions"], "reconsider");
    assert!(!target.exists());
    assert_eq!(
        std::fs::read_to_string(&source).unwrap(),
        "selected source contents"
    );
    assert!(instructions
        .snapshot_for_model()
        .unwrap()
        .contains("REVIEW_ALBUM_DESTINATION_BEFORE_COPY"));
    f.ctx
        .metadata
        .insert("tool_call_id".into(), json!("copy-after-review"));
    let result: Value = serde_json::from_str(&f.copy(args).await.text_content()).unwrap();
    assert_eq!(result["status"], "succeeded", "{result}");
    assert_eq!(result["target_device_id"], json!(f.ctx.local_device_id));
    assert_eq!(result["files_written"], 1);
    assert_eq!(result["source_deleted"], false);
    assert_eq!(result["files"][0]["target_path"], json!(target));
    assert_eq!(
        result["files"][0]["bytes"],
        "selected source contents".len()
    );
    assert_eq!(result["files"][0]["sha256"].as_str().unwrap().len(), 64);
    assert_eq!(
        std::fs::read(&target).unwrap(),
        std::fs::read(&source).unwrap()
    );
}

struct CreateDestinationAfterPlan {
    path: PathBuf,
    created: AtomicBool,
}

#[async_trait::async_trait]
impl AgentsInstructions for CreateDestinationAfterPlan {
    fn snapshot_for_model(&self) -> Option<String> {
        None
    }
    async fn before_tool(
        &self,
        _: &agent_types::ToolCall,
        _: &ToolContext,
        _: CancellationToken,
    ) -> rua_core::Result<AgentsInstructionDecision> {
        Ok(AgentsInstructionDecision::Continue)
    }
    async fn before_targets(
        &self,
        targets: &[AgentsInstructionTarget],
        _: &ToolContext,
        _: CancellationToken,
    ) -> rua_core::Result<AgentsInstructionDecision> {
        assert!(targets
            .iter()
            .any(|target| target.path == self.path.to_str().unwrap()));
        if !self.created.swap(true, Ordering::SeqCst) {
            // A real external write races the already-fixed receive plan.
            std::fs::write(&self.path, "another operation owns this file").unwrap();
        }
        Ok(AgentsInstructionDecision::Continue)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_transfer_reports_failed_commit_without_replanning_over_an_external_file() {
    let mut f = Origin::new().await;
    let source = f.source.join("note.txt");
    let target = f.destination.join("note.txt");
    std::fs::write(&source, "original selected file").unwrap();
    f.ctx.agents_instructions = Some(Arc::new(CreateDestinationAfterPlan {
        path: target.clone(),
        created: AtomicBool::new(false),
    }));
    let result: Value =
        serde_json::from_str(&f.copy(f.arguments(&source)).await.text_content()).unwrap();
    assert_eq!(result["status"], "failed", "{result}");
    assert_eq!(result["files_written"], 0);
    assert_eq!(result["source_deleted"], false);
    assert!(result["files"].as_array().unwrap().is_empty());
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "another operation owns this file"
    );
    assert_eq!(
        std::fs::read_to_string(&source).unwrap(),
        "original selected file"
    );
    assert_eq!(std::fs::read_dir(&f.destination).unwrap().count(), 1);
}
