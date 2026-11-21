use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use agent_types::ToolCall;
use rua_agent::runtime::agents_instructions::TurnAgentsInstructions;
use rua_core::agents_instruction_discovery::{
    AgentsInstructionDiscoveryRequest, AgentsInstructionLoader, AgentsInstructionTargetKind,
    AGENTS_INSTRUCTIONS_MAX_BYTES,
};
use rua_core::agents_instructions::{AgentsInstructionDecision, AgentsInstructions};
use rua_core::tool::ToolContext;
use rua_core::{ChatAddress, RuaError};
use rua_server::services::{
    chat_file_read_admission::TargetFileReadContext,
    conversation_authority::{
        ConversationActiveTurnPhase, ConversationActiveTurnStart, ConversationDriverKind,
    },
    conversation_authority_placement::{
        parse_conversation_authority_placement, ConversationAuthorityEpoch,
    },
    device_context::current_sync_device_id,
    project_instructions::RuntimeAgentsInstructionLoader,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[allow(dead_code)]
#[path = "../contract_chats_http/support.rs"]
mod support;

const AGENT: &str = "instruction-reader";

struct Fixture {
    app: support::TestApp,
    address: ChatAddress,
    device: String,
    epoch: ConversationAuthorityEpoch,
    config_epoch: String,
    cwd: PathBuf,
    loader: Arc<RuntimeAgentsInstructionLoader>,
}

impl Fixture {
    async fn new() -> Self {
        let app = support::build_app();
        support::create_agent(&app.app, AGENT, "Instruction reader").await;
        let chat = support::seed_materialized_chat(&app, AGENT, "Instructions").await;
        let address = ChatAddress::new(chat["id"].as_str().unwrap());
        let device = current_sync_device_id(&app.state.sync_device).unwrap();
        let epoch = parse_conversation_authority_placement(&chat["meta"])
            .unwrap()
            .authority_epoch;
        let config_epoch = chat["meta"]["hook_configuration"]["epoch"]
            .as_str()
            .unwrap()
            .to_owned();
        let cwd = app
            .state
            .registry
            .get_or_create(AGENT)
            .await
            .unwrap()
            .workspace_dir()
            .to_owned();
        let state = &app.state;
        let local = OnceLock::new();
        assert!(local
            .set(Arc::new(TargetFileReadContext::new(
                state.rua_root.clone(),
                device.clone(),
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
        Self {
            app,
            address,
            device,
            epoch,
            config_epoch,
            cwd,
            loader,
        }
    }

    async fn start_turn(&self) -> ToolContext {
        let turn = self
            .app
            .state
            .conversation_authority
            .start_active_turn(ConversationActiveTurnStart {
                address: self.address.clone(),
                driver: ConversationDriverKind::Rua,
                authority_device_id: self.device.clone(),
                authority_epoch: self.epoch,
                driver_detail: None,
            })
            .await;
        assert_eq!(turn.phase, ConversationActiveTurnPhase::Preparing);
        ToolContext {
            agent_id: AGENT.into(),
            session_id: self.address.chat_id().to_string(),
            local_device_id: Some(self.device.clone()),
            working_dir: self.cwd.clone(),
            metadata: std::collections::HashMap::from([
                (
                    rua_agent::hooks::SECURITY_CHAT_ADDRESS_METADATA_KEY.into(),
                    json!(self.address),
                ),
                ("turn_id".into(), json!(turn.turn_id)),
                ("chat_configuration_epoch".into(), json!(self.config_epoch)),
            ]),
            ..Default::default()
        }
    }

    fn request(&self) -> AgentsInstructionDiscoveryRequest {
        AgentsInstructionDiscoveryRequest {
            device_id: self.device.clone(),
            target_path: self.cwd.to_str().unwrap().into(),
            target_kind: AgentsInstructionTargetKind::Directory,
            max_bytes: AGENTS_INSTRUCTIONS_MAX_BYTES,
            ..Default::default()
        }
    }

    async fn prepare(&self, ctx: &ToolContext) -> TurnAgentsInstructions {
        TurnAgentsInstructions::prepare(
            self.loader.clone(),
            Some("Project resource inventory stays available".into()),
            ctx,
            CancellationToken::new(),
        )
        .await
        .unwrap()
    }

    async fn assert_no_approvals(&self) {
        let snapshot = self
            .app
            .state
            .security
            .approval_authority()
            .snapshot_at(&self.address)
            .await;
        assert!(snapshot.pending.is_empty());
        assert!(snapshot.resolved.is_empty());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preparing_turn_captures_real_local_instructions_and_next_turn_reads_fresh() {
    let f = Fixture::new().await;
    let path = f.cwd.join("AGENTS.md");
    std::fs::write(&path, "FIRST_TURN_INSTRUCTIONS").unwrap();
    let first_ctx = f.start_turn().await;
    assert!(!first_ctx.metadata.contains_key("tool_call_id"));
    let first = f.prepare(&first_ctx).await;
    let first_prompt = first.snapshot_for_model().unwrap();
    assert!(first_prompt.contains("FIRST_TURN_INSTRUCTIONS"));
    assert!(first_prompt.contains(&f.device));
    assert!(first_prompt.contains(path.canonicalize().unwrap().to_str().unwrap()));
    assert!(first_prompt.contains("Project resource inventory stays available"));

    std::fs::write(&path, "SECOND_TURN_INSTRUCTIONS").unwrap();
    let read = ToolCall::new("read-current-directory", "read", json!({"file_path":path}));
    assert_eq!(
        first
            .before_tool(&read, &first_ctx, CancellationToken::new())
            .await
            .unwrap(),
        AgentsInstructionDecision::Continue
    );
    assert_eq!(
        first.snapshot_for_model().as_deref(),
        Some(first_prompt.as_str())
    );

    f.app
        .state
        .conversation_authority
        .finish_active_turn(&f.address)
        .await;
    let second_ctx = f.start_turn().await;
    let second = f.prepare(&second_ctx).await.snapshot_for_model().unwrap();
    assert!(second.contains("SECOND_TURN_INSTRUCTIONS"));
    assert!(!second.contains("FIRST_TURN_INSTRUCTIONS"));

    std::fs::remove_file(path).unwrap();
    f.app
        .state
        .conversation_authority
        .finish_active_turn(&f.address)
        .await;
    let third_ctx = f.start_turn().await;
    let third = f.prepare(&third_ctx).await.snapshot_for_model().unwrap();
    assert!(!third.contains("FIRST_TURN_INSTRUCTIONS"));
    assert!(!third.contains("SECOND_TURN_INSTRUCTIONS"));
    assert!(third.contains("Project resource inventory stays available"));
    f.assert_no_approvals().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_explicit_local_directory_requires_model_review_before_the_tool() {
    let f = Fixture::new().await;
    std::fs::create_dir(f.cwd.join(".git")).unwrap();
    std::fs::write(f.cwd.join("AGENTS.md"), "ROOT_INSTRUCTIONS").unwrap();
    let nested = f.cwd.join("photos");
    std::fs::create_dir(&nested).unwrap();
    std::fs::write(nested.join("AGENTS.md"), "PHOTO_DIRECTORY_INSTRUCTIONS").unwrap();
    let ctx = f.start_turn().await;
    let coordinator = f.prepare(&ctx).await;
    let initial = coordinator.snapshot_for_model().unwrap();
    assert!(initial.contains("ROOT_INSTRUCTIONS"));
    assert!(!initial.contains("PHOTO_DIRECTORY_INSTRUCTIONS"));
    let edit = ToolCall::new(
        "create-photo-index",
        "edit",
        json!({"patch":format!(
            "*** Begin Patch\n*** Add File: {}\n+planned content\n*** End Patch",
            nested.join("index.txt").display()
        )}),
    );
    let decision = coordinator
        .before_tool(&edit, &ctx, CancellationToken::new())
        .await
        .unwrap();
    assert!(matches!(
        decision,
        AgentsInstructionDecision::Reconsider { .. }
    ));
    assert!(!nested.join("index.txt").exists());
    let updated = coordinator.snapshot_for_model().unwrap();
    assert!(updated.contains("ROOT_INSTRUCTIONS"));
    assert!(updated.contains("PHOTO_DIRECTORY_INSTRUCTIONS"));
    assert_eq!(
        coordinator
            .before_tool(&edit, &ctx, CancellationToken::new())
            .await
            .unwrap(),
        AgentsInstructionDecision::Continue
    );
    f.assert_no_approvals().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopping_finished_and_replaced_turns_cannot_load_local_instructions() {
    let f = Fixture::new().await;
    std::fs::write(f.cwd.join("AGENTS.md"), "DO_NOT_LOAD_AFTER_STOP").unwrap();
    let ctx = f.start_turn().await;
    f.app
        .state
        .conversation_authority
        .request_stop(None, "console", &f.address)
        .await
        .unwrap();
    let stopped = f
        .loader
        .discover(&f.request(), &ctx, CancellationToken::new())
        .await
        .unwrap();
    assert!(stopped.sources.is_empty());
    assert!(stopped
        .diagnostics
        .iter()
        .any(|s| s.contains("no longer active")));
    f.app
        .state
        .conversation_authority
        .finish_active_turn(&f.address)
        .await;
    let finished = f
        .loader
        .discover(&f.request(), &ctx, CancellationToken::new())
        .await
        .unwrap();
    assert!(finished.sources.is_empty());
    let next = f.start_turn().await;
    assert_ne!(ctx.metadata["turn_id"], next.metadata["turn_id"]);
    let old = f
        .loader
        .discover(&f.request(), &ctx, CancellationToken::new())
        .await
        .unwrap();
    assert!(old.sources.is_empty());
    let current = f
        .loader
        .discover(&f.request(), &next, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(current.sources.len(), 1);
    f.assert_no_approvals().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_or_stale_chat_context_skips_local_instructions_without_approval() {
    let f = Fixture::new().await;
    std::fs::write(f.cwd.join("AGENTS.md"), "CURRENT_CONTEXT_ONLY").unwrap();
    let ctx = f.start_turn().await;
    let mut missing_device = ctx.clone();
    missing_device.local_device_id = None;
    let mut missing_turn = ctx.clone();
    missing_turn.metadata.remove("turn_id");
    let mut missing_address = ctx.clone();
    missing_address
        .metadata
        .remove(rua_agent::hooks::SECURITY_CHAT_ADDRESS_METADATA_KEY);
    let mut stale_configuration = ctx.clone();
    stale_configuration
        .metadata
        .insert("chat_configuration_epoch".into(), json!("obsolete"));
    for context in [
        ToolContext::default(),
        missing_device,
        missing_turn,
        missing_address,
        stale_configuration,
    ] {
        let result = f
            .loader
            .discover(&f.request(), &context, CancellationToken::new())
            .await
            .unwrap();
        assert!(result.sources.is_empty());
        assert!(!result.diagnostics.is_empty());
    }
    f.assert_no_approvals().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_propagates_through_local_provider_and_coordinator() {
    let f = Fixture::new().await;
    std::fs::write(f.cwd.join("AGENTS.md"), "CANCELLED_CONTEXT").unwrap();
    let ctx = f.start_turn().await;
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        f.loader.discover(&f.request(), &ctx, cancel.clone()).await,
        Err(RuaError::Cancelled)
    ));
    assert!(matches!(
        TurnAgentsInstructions::prepare(f.loader.clone(), None, &ctx, cancel).await,
        Err(RuaError::Cancelled)
    ));
    f.assert_no_approvals().await;
}
