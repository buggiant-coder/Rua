use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rua_channel::core::console::{ConsoleChatRequest, ConsoleSubmitKind};
use rua_chat::{ChatService, ChatSetting, TURN_MODEL_KEY, TURN_THINKING_KEY};
use rua_contract::agent::{AgentRequest, AgentRuntime, RuntimeWarmupContext};
use rua_core::{ChatAddress, ChatScopeStore, RuaError, RunControl, RuntimeEvent};
use rua_provider::config_loader::DefaultModel;
use rua_server::services::active_stream_broadcast::ActiveStreamBroadcastFrame;
use rua_server::services::chat_metadata_store::{persist_acp_binding, AcpBindingMetadata};
use rua_server::services::console_chat::materialize_targeted_chat;
use rua_server::services::console_chat::{SessionSyncEmitter, CONSOLE_CHANNEL_ID};
use rua_server::services::conversation_authority::{
    ConversationActiveTurnPhase, ConversationActiveTurnStart, ConversationDriverKind,
    ConversationInputAcceptanceMode,
};
use rua_server::services::conversation_authority_placement::{
    ConversationAuthorityEpoch, ConversationAuthorityPlacement,
};
use rua_server::services::conversation_input_intake::{
    finalize_panicked_console_producer, ConversationInputDeliveryOutcome,
    ConversationInputDeliveryTransport, ConversationInputIntake, PanickedConsoleProducerContext,
};
use rua_server::services::device_command_bus::RemoteConversationInputRequest;
use rua_server::services::remote_conversation_input::RemoteConversationInputDelivery;
use rua_server::streaming::{Priority, StreamKey};
use rua_server::{AppState, ServerProfile};
use rua_session::{
    ChatMetadata, ChatSessionHeader, NewConversationMessage, NewConversationTurn, TurnLifecycle,
    WorkspaceSessionStore,
};
use tempfile::TempDir;

fn epoch(wall_ms: u64) -> ConversationAuthorityEpoch {
    ConversationAuthorityEpoch::from_parts(wall_ms, 0, [0; 8])
}

const CHAT_ID: &str = "00000000-0000-0000-0000-000000009903";

fn chat_address() -> ChatAddress {
    ChatAddress::agent("default", CHAT_ID)
}

fn stream_key() -> StreamKey {
    StreamKey::new(CONSOLE_CHANNEL_ID, chat_address(), Priority::Normal)
}

fn local_device_id(state: &AppState) -> String {
    rua_server::services::device_context::current_sync_device_id(&state.sync_device)
        .expect("test Device identity")
}

async fn seed_model_switch_provider(state: &AppState, id: &str, models: &[&str]) {
    state
        .provider_cells
        .cell(id)
        .write(
            serde_json::from_value(serde_json::json!({
                "id":id, "name":"Model switch fixture", "chat_model":"OpenAIChatModel",
                "base_url":"http://127.0.0.1:1/v1", "api_key":"fixture",
                "models":models.iter().map(|model| serde_json::json!({"id":model,"name":model,
                    "supports_reasoning":true,"thinking_param_style":"effort",
                    "reasoning_effort_options":["low","medium","high"]})).collect::<Vec<_>>()
            }))
            .unwrap(),
        )
        .await
        .unwrap();
}

async fn seed_local_authority(state: &AppState, chat_id: &str, title: &str) {
    state.wait_platform_initialization().await.unwrap();
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("workspace");
    materialize_targeted_chat(
        &runtime.workspace_dir(),
        "default",
        chat_id,
        title,
        &ConversationAuthorityPlacement {
            device_id: local_device_id(state),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            base_manifest_hash:
                "sha256:1111111111111111111111111111111111111111111111111111111111111111"
                    .to_string(),
        },
    )
    .await
    .expect("seed local authority");
    capture_fixture_configuration(state, chat_id).await;
}

async fn capture_fixture_configuration(state: &AppState, chat_id: &str) {
    let runtime = state.registry.get_or_create("default").await.unwrap();
    let store = WorkspaceSessionStore::new(runtime.workspace_dir());
    let mut header = store.load_chat_header(chat_id).await.unwrap().unwrap();
    let authority_device_id = header.extra["authority_placement"]["device_id"]
        .as_str()
        .expect("fixture authority device")
        .to_string();
    let model = header
        .extra
        .get("model")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .unwrap();
    let configuration = state
        .product_hooks
        .resolve_complete_chat_configuration(
            &ChatAddress::agent("default", chat_id),
            rua_server::services::product_hooks::ChatConfigurationMaterialization {
                selected_agent_id: "default".into(),
                running_config: Default::default(),
                security_config: Default::default(),
                agent_assignment: None,
                prompt_file_content: None,
                tool_config_snapshot: Default::default(),
                capability_configuration: None,
                model,
                thinking: None,
                collaboration_mode: serde_json::json!("default"),
                harness_configuration: None,
                authority_device_id,
            },
        )
        .await
        .unwrap();
    header.extra.insert(
        "hook_configuration".into(),
        serde_json::to_value(configuration).unwrap(),
    );
    store.save_chat_header(&header).await.unwrap();
}

#[tokio::test]
async fn authority_intake_owns_acp_binding_persistence() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    seed_local_authority(&state, CHAT_ID, "ACP authority persistence").await;
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("workspace");
    let workspace_dir = runtime.workspace_dir();
    let device_id = local_device_id(&state);

    intake(&state)
        .enter_authority_acp(
            "default",
            CHAT_ID,
            AcpBindingMetadata {
                target_device_id: Some(device_id.clone()),
                runner: "codex".to_string(),
                cwd: Some("/tmp".to_string()),
                runner_session_id: Some("runner-session".to_string()),
            },
        )
        .await
        .expect("authority enters ACP");

    let entered = rua_server::services::chat_index::get(&workspace_dir, CHAT_ID)
        .await
        .expect("read Chat Index")
        .expect("Chat Index row");
    assert_eq!(entered.meta["acp_binding"]["device_id"], device_id);
    assert_eq!(entered.meta["acp_binding"]["runner"], "codex");
    assert_eq!(
        WorkspaceSessionStore::new(&workspace_dir)
            .load_chat_header(CHAT_ID)
            .await
            .expect("load Chat Header")
            .expect("Chat Header")
            .extra["acp_binding"]["runner_session_id"],
        "runner-session"
    );

    intake(&state)
        .exit_authority_acp("default", CHAT_ID)
        .await
        .expect("authority exits ACP");
    let exited = rua_server::services::chat_index::get(&workspace_dir, CHAT_ID)
        .await
        .expect("read Chat Index")
        .expect("Chat Index row");
    assert!(exited.meta.get("acp_binding").is_none());
}

#[tokio::test]
async fn remote_input_rejects_stale_authority_epoch() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("workspace");
    let device_id = local_device_id(&state);
    materialize_targeted_chat(
        &runtime.workspace_dir(),
        "default",
        CHAT_ID,
        "Epoch test",
        &ConversationAuthorityPlacement {
            device_id: device_id.clone(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(2, 0, [0; 8]),
            base_manifest_hash:
                "sha256:2222222222222222222222222222222222222222222222222222222222222222"
                    .to_string(),
        },
    )
    .await
    .expect("materialize Chat");
    let mut request = remote_request_to(CHAT_ID, "submission-stale", 1, "stale", &device_id);
    request.authority_epoch = epoch(1);
    request.base_manifest_hash =
        "sha256:1111111111111111111111111111111111111111111111111111111111111111".to_string();

    let error = intake(&state)
        .accept_remote_conversation_input(request, chrono::Utc::now().to_rfc3339())
        .await
        .expect_err("stale Epoch must be rejected");

    assert!(error.contains("stale conversation authority Epoch"));
}

#[tokio::test]
async fn remote_input_queues_on_busy_target() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    seed_local_authority(&state, CHAT_ID, "Remote CLI busy").await;
    let target_device_id = local_device_id(&state);
    let key = stream_key();
    let (_producer_id, _cancel_rx) = state.streams.register_active_keyed(&key).await;
    state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: chat_address(),
            driver: ConversationDriverKind::Rua,
            authority_device_id: target_device_id.clone(),
            authority_epoch: epoch(1),
            driver_detail: None,
        })
        .await;
    let request = remote_request_to(
        CHAT_ID,
        "submission-cli-busy",
        1,
        "second",
        &target_device_id,
    );

    let ack = intake(&state)
        .accept_remote_conversation_input(request, chrono::Utc::now().to_rfc3339())
        .await
        .expect("busy target must queue input");

    assert_eq!(ack.mode, ConversationInputAcceptanceMode::Queued);
    let control = state
        .conversation_authority
        .control_view(&chat_address(), &serde_json::json!({}))
        .await;
    assert_eq!(control.queue.entries.len(), 1);
    assert_eq!(control.queue.entries[0].text, "second");
    assert!(control.active_turn.is_some());
}

#[derive(Clone)]
struct HoldRuntime {
    release: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl AgentRuntime for HoldRuntime {
    async fn stream(
        &self,
        req: AgentRequest,
    ) -> rua_core::Result<
        Pin<Box<dyn futures::Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>,
    > {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let release = Arc::clone(&self.release);
        tokio::spawn(async move {
            let _ = tx.send(Ok(RuntimeEvent::Start {
                session_id: req.session_id,
                agent_id: Some(req.agent_id),
            }));
            release.notified().await;
            let _ = tx.send(Ok(RuntimeEvent::End {
                finish_reason: Some("stop".to_string()),
            }));
        });
        Ok(Box::pin(futures::stream::unfold(rx, |mut rx| async {
            rx.recv().await.map(|item| (item, rx))
        })))
    }

    async fn cancel(&self, _session_id: &rua_core::id::SessionId) -> rua_core::Result<()> {
        Ok(())
    }

    async fn warmup(&self, _context: RuntimeWarmupContext) -> rua_core::Result<()> {
        Ok(())
    }

    async fn shutdown(&self) -> rua_core::Result<()> {
        Ok(())
    }
}

struct PanickingRuntime;

#[async_trait::async_trait]
impl AgentRuntime for PanickingRuntime {
    async fn stream(
        &self,
        _req: AgentRequest,
    ) -> rua_core::Result<
        Pin<Box<dyn futures::Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>,
    > {
        panic!("simulated provider panic")
    }

    async fn cancel(&self, _session_id: &rua_core::id::SessionId) -> rua_core::Result<()> {
        Ok(())
    }

    async fn warmup(&self, _context: RuntimeWarmupContext) -> rua_core::Result<()> {
        Ok(())
    }

    async fn shutdown(&self) -> rua_core::Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
struct RecordingRuntime {
    model_override: Arc<tokio::sync::Mutex<Option<String>>>,
    started: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl AgentRuntime for RecordingRuntime {
    async fn stream(
        &self,
        req: AgentRequest,
    ) -> rua_core::Result<
        Pin<Box<dyn futures::Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>,
    > {
        *self.model_override.lock().await = req.model_override.clone();
        self.started.notify_waiters();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            let _ = tx.send(Ok(RuntimeEvent::Start {
                session_id: req.session_id,
                agent_id: Some(req.agent_id),
            }));
            let _ = tx.send(Ok(RuntimeEvent::End {
                finish_reason: Some("stop".to_string()),
            }));
        });
        Ok(Box::pin(futures::stream::unfold(rx, |mut rx| async {
            rx.recv().await.map(|item| (item, rx))
        })))
    }

    async fn cancel(&self, _session_id: &rua_core::id::SessionId) -> rua_core::Result<()> {
        Ok(())
    }

    async fn warmup(&self, _context: RuntimeWarmupContext) -> rua_core::Result<()> {
        Ok(())
    }

    async fn shutdown(&self) -> rua_core::Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
struct FirstTurnHoldRuntime {
    calls: Arc<AtomicUsize>,
    model_overrides: Arc<tokio::sync::Mutex<Vec<Option<String>>>>,
    first_started: Arc<tokio::sync::Notify>,
    release_first: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl AgentRuntime for FirstTurnHoldRuntime {
    async fn stream(
        &self,
        req: AgentRequest,
    ) -> rua_core::Result<
        Pin<Box<dyn futures::Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>,
    > {
        self.model_overrides
            .lock()
            .await
            .push(req.model_override.clone());
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let first_started = Arc::clone(&self.first_started);
        let release_first = Arc::clone(&self.release_first);
        tokio::spawn(async move {
            let _ = tx.send(Ok(RuntimeEvent::Start {
                session_id: req.session_id,
                agent_id: Some(req.agent_id),
            }));
            if call == 0 {
                first_started.notify_one();
                release_first.notified().await;
            }
            let _ = tx.send(Ok(RuntimeEvent::End {
                finish_reason: Some("stop".to_string()),
            }));
        });
        Ok(Box::pin(futures::stream::unfold(rx, |mut rx| async {
            rx.recv().await.map(|item| (item, rx))
        })))
    }

    async fn cancel(&self, _session_id: &rua_core::id::SessionId) -> rua_core::Result<()> {
        Ok(())
    }

    async fn warmup(&self, _context: RuntimeWarmupContext) -> rua_core::Result<()> {
        Ok(())
    }

    async fn shutdown(&self) -> rua_core::Result<()> {
        Ok(())
    }
}

fn request(chat_id: &str, message: &str) -> ConsoleChatRequest {
    ConsoleChatRequest {
        message: message.to_string(),
        chat_id: Some(chat_id.to_string()),
        model: None,
        is_reconnect: false,
        attachments: Vec::new(),
        input: None,
        messages: None,
        extra: serde_json::Map::new(),
        channel_kind: None,
    }
}

fn assert_local_delivery(outcome: ConversationInputDeliveryOutcome, expected_chat_id: &str) {
    match outcome {
        ConversationInputDeliveryOutcome::Delivered {
            chat_id,
            transport: ConversationInputDeliveryTransport::LocalAuthority,
            accepted: true,
            queued_to_active_authority: false,
            ..
        } => assert_eq!(chat_id, expected_chat_id),
        other => panic!("expected local authority delivery, got {other:?}"),
    }
}

fn assert_authority_queue_delivery(
    outcome: ConversationInputDeliveryOutcome,
    expected_chat_id: &str,
) {
    match outcome {
        ConversationInputDeliveryOutcome::Delivered {
            chat_id,
            transport: ConversationInputDeliveryTransport::LocalAuthority,
            accepted: true,
            queued_to_active_authority: true,
            ..
        } => assert_eq!(chat_id, expected_chat_id),
        other => panic!("expected queued authority delivery, got {other:?}"),
    }
}

fn remote_request_to(
    chat_id: &str,
    submission_id: &str,
    sequence: u64,
    message: &str,
    target_device_id: &str,
) -> RemoteConversationInputRequest {
    RemoteConversationInputRequest {
        address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
        agent_id: "default".to_string(),
        source_device_id: "source-device".to_string(),
        target_device_id: target_device_id.to_string(),
        authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
        base_manifest_hash:
            "sha256:1111111111111111111111111111111111111111111111111111111111111111".to_string(),
        submission_id: submission_id.to_string(),
        device_delivery_id: submission_id.to_string(),
        device_delivery_sequence: sequence,
        text: message.to_string(),
        input: None,
        media_requirements: Vec::new(),
        created_at: "2026-06-27T00:00:00Z".to_string(),
        delivery_deadline_at: "2026-06-27T00:00:30Z".to_string(),
        metadata: serde_json::Map::new(),
    }
}

fn intake(state: &AppState) -> ConversationInputIntake {
    let session_sync = SessionSyncEmitter::new(
        state.rua_root.clone(),
        Arc::clone(&state.sync_enabled),
        Arc::clone(&state.sync_node),
        Arc::clone(&state.sync_network_secret),
        Arc::clone(&state.sync_device),
        Arc::clone(&state.sync_hlc_clock),
        Arc::clone(&state.sync_trigger),
    );
    ConversationInputIntake::new(
        Arc::clone(&state.chat_service),
        Arc::clone(&state.streams),
        Arc::clone(&state.registry),
        session_sync,
        Arc::clone(&state.sync_device),
        None,
        Some(Arc::clone(&state.active_stream_broadcast_hub)),
        RemoteConversationInputDelivery::new(
            Arc::clone(&state.sync_node),
            Arc::clone(&state.device_network),
            state.rua_root.clone(),
        ),
    )
    .with_agent_service(Arc::clone(&state.agent_service))
    .with_provider_manager(Arc::clone(&state.provider_manager))
    .with_provider_cells(Arc::clone(&state.provider_cells))
    .with_conversation_authority(Arc::clone(&state.conversation_authority))
    .with_security(Arc::clone(&state.security))
    .with_product_hooks(Arc::clone(&state.product_hooks))
    .with_agent_service(Arc::clone(&state.agent_service))
}

#[tokio::test]
async fn intake_rejects_invalid_chat_id() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);

    let outcome = intake(&state)
        .submit_delivery("default", request("not-a-uuid", "hello"))
        .await;

    match outcome {
        ConversationInputDeliveryOutcome::Rejected { detail, .. } => {
            assert!(detail.contains("invalid chat_id"));
        }
        other => panic!("expected rejected outcome, got {other:?}"),
    }
}

#[tokio::test]
async fn intake_generated_chat_persists_default_model() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    state
        .provider_manager
        .store_default_model(DefaultModel {
            provider_id: "dashscope".to_string(),
            model: "qwen3.7-plus".to_string(),
            source: None,
            extra: Default::default(),
        })
        .await;
    let mut submit = request(CHAT_ID, "hello without a client chat id");
    submit.chat_id = None;

    let outcome = intake(&state).submit_delivery("default", submit).await;
    let chat_id = match outcome {
        ConversationInputDeliveryOutcome::Delivered { chat_id, .. } => chat_id,
        other => panic!("expected generated Chat delivery, got {other:?}"),
    };
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("workspace");
    let header = WorkspaceSessionStore::new(runtime.workspace_dir())
        .load_chat_header(&chat_id)
        .await
        .expect("load header")
        .expect("header exists");

    assert_eq!(header.extra["model"]["provider_id"], "dashscope");
    assert_eq!(header.extra["model"]["model"], "qwen3.7-plus");
    assert_eq!(header.extra["model"]["thinking_strength"], "auto");
}

#[tokio::test]
async fn incomplete_replica_submit_never_materializes_local_authority() {
    for partial in ["configuration_head.json", "configurations/remote.json"] {
        let temp = TempDir::new().expect("tempdir");
        let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
        let runtime = state
            .registry
            .get_or_create("default")
            .await
            .expect("workspace");
        let store = WorkspaceSessionStore::new(runtime.workspace_dir());
        let path = store.chat_dir(CHAT_ID).join(partial);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{}\n").unwrap();
        let outcome = intake(&state)
            .submit_delivery("default", request(CHAT_ID, "hello"))
            .await;
        assert!(
            matches!(outcome, ConversationInputDeliveryOutcome::Rejected { ref detail, .. } if detail.contains("not ready")),
            "{outcome:?}"
        );
        assert!(store.load_chat_header(CHAT_ID).await.unwrap().is_none());
        assert_eq!(std::fs::read(path).unwrap(), b"{}\n");
        assert!(state
            .conversation_authority
            .active_turn(&chat_address())
            .await
            .is_none());
    }
}

#[tokio::test]
async fn expected_epoch_without_replica_rejects_before_creating_chat() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    let mut input = request(CHAT_ID, "hello");
    input.extra.insert(
        "expected_authority_epoch".into(),
        serde_json::json!("remote-epoch"),
    );
    let outcome = intake(&state).submit_delivery("default", input).await;
    assert!(
        matches!(outcome, ConversationInputDeliveryOutcome::Rejected { ref detail, .. } if detail.contains("not ready")),
        "{outcome:?}"
    );
    let runtime = state.registry.get_or_create("default").await.unwrap();
    assert!(WorkspaceSessionStore::new(runtime.workspace_dir())
        .load_chat_header(CHAT_ID)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn index_only_replica_submit_does_not_recreate_authority() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    seed_local_authority(&state, CHAT_ID, "partial replica").await;
    let runtime = state.registry.get_or_create("default").await.unwrap();
    let workspace = runtime.workspace_dir();
    let store = WorkspaceSessionStore::new(&workspace);
    std::fs::remove_dir_all(store.chat_dir(CHAT_ID)).unwrap();
    assert!(rua_server::services::chat_index::get(&workspace, CHAT_ID)
        .await
        .unwrap()
        .is_some());
    let outcome = intake(&state)
        .submit_delivery("default", request(CHAT_ID, "hello"))
        .await;
    assert!(
        matches!(outcome, ConversationInputDeliveryOutcome::Rejected { ref detail, .. } if detail.contains("not ready")),
        "{outcome:?}"
    );
    assert!(store.load_chat_header(CHAT_ID).await.unwrap().is_none());
}

#[tokio::test]
async fn intake_enqueues_new_local_input() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);

    let outcome = intake(&state)
        .submit_delivery("default", request(CHAT_ID, "hello"))
        .await;

    assert_local_delivery(outcome, CHAT_ID);
    let snapshot = state
        .conversation_authority
        .active_turn(&chat_address())
        .await
        .expect("local submit creates authority run snapshot");
    assert_eq!(snapshot.phase, ConversationActiveTurnPhase::Preparing);
    assert_eq!(snapshot.driver, ConversationDriverKind::Rua);
}

#[tokio::test]
async fn rejected_first_input_does_not_materialize_agent_chat() {
    let temp = TempDir::new().unwrap();
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    state.wait_platform_initialization().await.unwrap();
    state.conversation_authority.begin_draining("default").await;
    let outcome = intake(&state)
        .submit_delivery("default", request(CHAT_ID, "must be rejected"))
        .await;
    assert!(matches!(outcome, rua_server::services::conversation_input_intake::ConversationInputDeliveryOutcome::Rejected { .. }), "{outcome:?}");
    let workspace = temp.path().join("workspaces/default");
    assert!(
        WorkspaceSessionStore::new(&workspace)
            .load_chat_header(CHAT_ID)
            .await
            .unwrap()
            .is_none(),
        "rejected input must not persist a Chat header"
    );
    assert!(
        rua_server::services::chat_index::get(&workspace, CHAT_ID)
            .await
            .unwrap()
            .is_none(),
        "rejected input must not persist a Chat index entry"
    );
}

#[tokio::test]
async fn invalid_initial_acp_is_rejected_before_materialization() {
    let temp = TempDir::new().unwrap();
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    state.wait_platform_initialization().await.unwrap();
    let mut input = request(CHAT_ID, "first ACP input");
    input.extra.insert(
        "initial_acp".into(),
        serde_json::json!({"runner": "missing-runner"}),
    );
    let outcome = intake(&state).submit_delivery("default", input).await;
    assert!(
        matches!(outcome, ConversationInputDeliveryOutcome::Rejected { .. }),
        "{outcome:?}"
    );
    let store = WorkspaceSessionStore::new(temp.path().join("workspaces/default"));
    assert!(store.load_chat_header(CHAT_ID).await.unwrap().is_none());
    assert!(state
        .conversation_authority
        .active_turn(&chat_address())
        .await
        .is_none());
}

#[tokio::test]
async fn rejected_remote_first_agent_input_does_not_materialize_chat() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    state.wait_platform_initialization().await.unwrap();
    state.conversation_authority.begin_draining("default").await;
    let target_device_id = local_device_id(&state);
    let mut remote = remote_request_to(
        CHAT_ID,
        "remote-first-rejected",
        1,
        "must be rejected",
        &target_device_id,
    );
    remote.base_manifest_hash =
        rua_server::services::conversation_manifest::initial_conversation_manifest_hash(
            &chat_address(),
        )
        .unwrap();

    let error = intake(&state)
        .accept_remote_conversation_input(remote, chrono::Utc::now().to_rfc3339())
        .await
        .expect_err("draining Agent rejects remote first input");
    assert!(error.contains("shutting down"), "{error}");
    let workspace = temp.path().join("workspaces/default");
    assert!(WorkspaceSessionStore::new(&workspace)
        .load_chat_header(CHAT_ID)
        .await
        .unwrap()
        .is_none());
    assert!(rua_server::services::chat_index::get(&workspace, CHAT_ID)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn accepted_remote_first_agent_input_materializes_chat_and_turn() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    let target_device_id = local_device_id(&state);
    let mut remote = remote_request_to(
        CHAT_ID,
        "remote-first-accepted",
        1,
        "accepted remotely",
        &target_device_id,
    );
    remote.base_manifest_hash =
        rua_server::services::conversation_manifest::initial_conversation_manifest_hash(
            &chat_address(),
        )
        .unwrap();

    let ack = intake(&state)
        .accept_remote_conversation_input(remote, chrono::Utc::now().to_rfc3339())
        .await
        .expect("remote first input accepted");
    assert_eq!(ack.mode, ConversationInputAcceptanceMode::Started);
    let workspace = temp.path().join("workspaces/default");
    let store = WorkspaceSessionStore::new(&workspace);
    let header = store
        .load_chat_header(CHAT_ID)
        .await
        .unwrap()
        .expect("accepted remote input materializes Chat");
    assert_eq!(header.turn_index.latest_turn, 1);
    assert!(rua_server::services::chat_index::get(&workspace, CHAT_ID)
        .await
        .unwrap()
        .is_some());
    let turns = store
        .read_turn_range(CHAT_ID, 1, 1)
        .await
        .expect("read accepted Turn");
    assert_eq!(turns.len(), 1);
    let messages = store
        .read_turn_messages(CHAT_ID, &turns[0].turn_id)
        .await
        .expect("read accepted user message");
    assert_eq!(
        messages[0].metadata["submission_id"],
        "remote-first-accepted"
    );
}

#[tokio::test]
async fn corrupted_first_input_attachment_is_rejected_without_chat() {
    let temp = TempDir::new().unwrap();
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    state.wait_platform_initialization().await.unwrap();
    let target = local_device_id(&state);
    let mut remote = remote_request_to(CHAT_ID, "corrupt-first-media", 1, "inspect", &target);
    remote.base_manifest_hash =
        rua_server::services::conversation_manifest::initial_conversation_manifest_hash(
            &chat_address(),
        )
        .unwrap();
    remote
        .metadata
        .insert("first_submission".into(), serde_json::Value::Bool(true));
    let media_id = "00000000-0000-4000-8000-000000000112";
    let root = temp.path().join("workspaces/default");
    std::fs::create_dir_all(root.join("media")).unwrap();
    std::fs::write(root.join("media").join(media_id), b"abc").unwrap();
    remote.input = Some(vec![
        serde_json::json!({"role":"user","content":[{"type":"file","media_ref":format!("media/{media_id}")} ]}),
    ]);
    remote.media_requirements = vec![
        rua_server::services::device_command_bus::RemoteConversationInputMediaRequirement {
            agent_id: "default".into(),
            media_ref: format!("media/{media_id}"),
            filename: media_id.into(),
            byte_size: 3,
            sha256: "0".repeat(64),
            mime_type: "text/plain".into(),
        },
    ];
    let error = intake(&state)
        .accept_remote_conversation_input(remote, chrono::Utc::now().to_rfc3339())
        .await
        .unwrap_err();
    assert!(error.contains("integrity mismatch"), "{error}");
    assert!(WorkspaceSessionStore::new(&root)
        .load_chat_header(CHAT_ID)
        .await
        .unwrap()
        .is_none());
    assert!(state
        .conversation_authority
        .active_turn(&chat_address())
        .await
        .is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_first_agent_inputs_keep_the_winner_configuration() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    seed_model_switch_provider(&state, "fixture", &["model-a", "model-b"]).await;
    state.wait_platform_initialization().await.unwrap();
    let model_overrides = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let release_first = Arc::new(tokio::sync::Notify::new());
    state
        .registry
        .get("default")
        .await
        .unwrap()
        .set_react_runtime(Arc::new(FirstTurnHoldRuntime {
            calls: Arc::new(AtomicUsize::new(0)),
            model_overrides: Arc::clone(&model_overrides),
            first_started: Arc::new(tokio::sync::Notify::new()),
            release_first: Arc::clone(&release_first),
        }))
        .await;
    let input_intake = intake(&state);
    let barrier = Arc::new(tokio::sync::Barrier::new(3));

    let submit = |model: &'static str| {
        let input_intake = input_intake.clone();
        let barrier = Arc::clone(&barrier);
        tokio::spawn(async move {
            let mut request = request(CHAT_ID, model);
            request.extra.insert(
                "chat_model_slot".to_string(),
                serde_json::json!({"provider_id": "fixture", "model": model}),
            );
            barrier.wait().await;
            input_intake.submit_delivery("default", request).await
        })
    };
    let first = submit("model-a");
    let second = submit("model-b");
    barrier.wait().await;
    let first = first.await.expect("first submit task");
    let second = second.await.expect("second submit task");
    assert!(
        matches!(first, ConversationInputDeliveryOutcome::Delivered { .. }),
        "{first:?}"
    );
    assert!(
        matches!(second, ConversationInputDeliveryOutcome::Delivered { .. }),
        "{second:?}"
    );
    release_first.notify_one();

    let store = WorkspaceSessionStore::new(temp.path().join("workspaces/default"));
    let header = store
        .load_chat_header(CHAT_ID)
        .await
        .expect("load Chat header")
        .expect("accepted input materializes Chat");
    let winner_model = header.extra["hook_configuration"]["model"]["model"]
        .as_str()
        .expect("winner model");
    assert!(matches!(winner_model, "model-a" | "model-b"));
    assert_eq!(header.extra["model"]["model"], winner_model);
    let turns = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let turns = store
                .read_turn_range(CHAT_ID, 1, usize::MAX)
                .await
                .expect("read accepted turns");
            if turns.len() == 2 {
                break turns;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("both accepted inputs become durable");
    assert_eq!(turns.len(), 2);
    for turn in turns {
        assert_eq!(turn.metadata[TURN_MODEL_KEY]["model"], winner_model);
    }
    let executed_models = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let models = model_overrides.lock().await.clone();
            if models.len() == 2 {
                break models;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("both accepted Turns reach the controlled Runtime");
    assert_eq!(executed_models, vec![Some(winner_model.to_string()); 2]);
}

#[tokio::test]
async fn intake_accepts_session_authority_without_chat_index() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    let workspace = temp.path().join("workspaces/default");
    let device_id = local_device_id(&state);
    let now = "2026-07-20T00:00:00Z".to_string();
    let mut header = ChatSessionHeader::new(ChatMetadata {
        chat_id: CHAT_ID.to_string(),
        agent_id: "default".to_string(),
        title: CHAT_ID.to_string(),
        created_at: now.clone(),
        last_activity_at: now,
        pinned: false,
        archived: false,
        preview: None,
    });
    header.extra.insert(
        "authority_placement".to_string(),
        serde_json::json!({
            "device_id": device_id,
            "authority_epoch": "0000000000000001000000000000000000000000",
            "base_manifest_hash":
                "sha256:1111111111111111111111111111111111111111111111111111111111111111",
        }),
    );
    WorkspaceSessionStore::new(&workspace)
        .save_chat_header(&header)
        .await
        .expect("seed draft header");
    capture_fixture_configuration(&state, CHAT_ID).await;

    let outcome = intake(&state)
        .submit_delivery("default", request(CHAT_ID, "hello"))
        .await;

    assert_local_delivery(outcome, CHAT_ID);
    let indexed = rua_server::services::chat_index::get(&workspace, CHAT_ID)
        .await
        .expect("read chat index")
        .expect("indexed Chat");
    assert_eq!(indexed.name, "hello");
}

#[tokio::test]
async fn submit_delivery_reports_local_authority_transport() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);

    let outcome = intake(&state)
        .submit_delivery("default", request(CHAT_ID, "hello delivery"))
        .await;

    match outcome {
        rua_server::services::conversation_input_intake::ConversationInputDeliveryOutcome::Delivered {
            chat_id,
            transport,
            accepted,
            queued_to_active_authority,
            ..
        } => {
            assert_eq!(chat_id, CHAT_ID);
            assert_eq!(transport, ConversationInputDeliveryTransport::LocalAuthority);
            assert!(accepted);
            assert!(!queued_to_active_authority);
        }
        other => panic!("expected delivered local authority outcome, got {other:?}"),
    }
}

#[tokio::test]
async fn submit_delivery_reports_remote_authority_transport() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    let workspace = temp.path().join("workspaces/default");
    materialize_targeted_chat(
        &workspace,
        "default",
        CHAT_ID,
        "Remote authority",
        &ConversationAuthorityPlacement {
            device_id: "remote-device".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            base_manifest_hash:
                "sha256:1111111111111111111111111111111111111111111111111111111111111111"
                    .to_string(),
        },
    )
    .await
    .expect("seed remote authority");
    capture_fixture_configuration(&state, CHAT_ID).await;
    let store = WorkspaceSessionStore::new(&workspace);
    let original_header = store
        .load_chat_header(CHAT_ID)
        .await
        .expect("load header")
        .expect("header");
    let mut index = rua_server::services::chat_index::get(&workspace, CHAT_ID)
        .await
        .expect("load index")
        .expect("index");
    index.name = "Local stale title".to_string();
    index.updated_at = chrono::Utc::now() + chrono::Duration::hours(1);
    rua_server::services::chat_index::upsert(&workspace, index)
        .await
        .expect("write local read model");

    let outcome = intake(&state)
        .submit_delivery("default", request(CHAT_ID, "hello remote delivery"))
        .await;

    match outcome {
        rua_server::services::conversation_input_intake::ConversationInputDeliveryOutcome::Rejected {
            target_device_id,
            detail,
            failure_kind,
        } => {
            assert_eq!(target_device_id.as_deref(), Some("remote-device"));
            assert!(detail.contains("Device Network Runtime is suspended"));
            assert_eq!(failure_kind.as_deref(), Some("device_offline"));
        }
        other => panic!("expected rejected remote authority delivery, got {other:?}"),
    }
    let after_header = store
        .load_chat_header(CHAT_ID)
        .await
        .expect("reload header")
        .expect("header");
    assert_eq!(
        after_header, original_header,
        "source-side remote delivery must not rewrite authority-owned Chat metadata"
    );
}

#[tokio::test]
async fn local_submit_uses_authority_header_model_and_ignores_request_metadata() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    let model_override = Arc::new(tokio::sync::Mutex::new(None));
    let started = Arc::new(tokio::sync::Notify::new());
    state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime")
        .set_react_runtime(Arc::new(RecordingRuntime {
            model_override: Arc::clone(&model_override),
            started: Arc::clone(&started),
        }))
        .await;

    let workspace = temp.path().join("workspaces/default");
    materialize_targeted_chat(
        &workspace,
        "default",
        CHAT_ID,
        "Model slot",
        &ConversationAuthorityPlacement {
            device_id: local_device_id(&state),
            authority_epoch: epoch(1),
            base_manifest_hash:
                "sha256:1111111111111111111111111111111111111111111111111111111111111111"
                    .to_string(),
        },
    )
    .await
    .expect("materialize Chat Index placement");
    let store = WorkspaceSessionStore::new(&workspace);
    let mut header = store
        .load_chat_header(CHAT_ID)
        .await
        .expect("load header")
        .expect("materialized header");
    header.extra.insert(
        "model".to_string(),
        serde_json::json!({
            "provider_id": "opencode",
            "model": "deepseek-v4-flash-free"
        }),
    );
    store.save_chat_header(&header).await.expect("save header");

    let mut submit = request(CHAT_ID, "hello model slot");
    capture_fixture_configuration(&state, CHAT_ID).await;
    submit.extra.insert(
        "hook_configuration".to_string(),
        serde_json::json!({
            "epoch": "forged", "running_config": {"max_iters": 999999},
            "selected_agent_id": "other-agent", "allowed_tools": ["bash"],
        }),
    );
    submit.model = Some("source-model".to_string());
    submit.extra.insert(
        "chat_model_slot".to_string(),
        serde_json::json!({
            "provider_id": "source-provider",
            "model": "source-model"
        }),
    );
    let outcome = intake(&state).submit_delivery("default", submit).await;
    assert_local_delivery(outcome, CHAT_ID);
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .expect("local run started");
    assert_eq!(
        model_override.lock().await.as_deref(),
        Some("deepseek-v4-flash-free")
    );

    let snapshot = state
        .conversation_authority
        .active_turn(&chat_address())
        .await
        .expect("local submit creates authority run snapshot");
    assert_eq!(snapshot.driver, ConversationDriverKind::Rua);
    assert_eq!(
        snapshot
            .driver_detail
            .as_ref()
            .and_then(|detail| detail.get("metadata"))
            .and_then(|metadata| metadata.get("chat_model_slot"))
            .and_then(|slot| slot.get("provider_id"))
            .and_then(|provider| provider.as_str()),
        Some("opencode")
    );
    assert_eq!(
        snapshot
            .driver_detail
            .as_ref()
            .and_then(|detail| detail.get("metadata"))
            .and_then(|metadata| metadata.get("chat_model_slot"))
            .and_then(|slot| slot.get("model"))
            .and_then(|model| model.as_str()),
        Some("deepseek-v4-flash-free")
    );
}

#[tokio::test]
async fn queued_turn_reads_latest_authority_header_model_at_execution() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    seed_model_switch_provider(
        &state,
        "target-provider",
        &["target-model-a", "target-model-b"],
    )
    .await;
    seed_local_authority(&state, CHAT_ID, "Queued model switch").await;

    let address = ChatAddress::agent("default", CHAT_ID);
    let mut control_events = state
        .conversation_authority
        .subscribe_control_events(&address)
        .await;
    let calls = Arc::new(AtomicUsize::new(0));
    intake(&state)
        .update_authority_chat_model(
            "default",
            CHAT_ID,
            serde_json::json!({"provider_id": "target-provider", "model": "target-model-a"}),
        )
        .await
        .unwrap();
    let event: serde_json::Value =
        serde_json::from_str(&control_events.try_recv().unwrap()).unwrap();
    assert_eq!(event["reason"], "model_updated");
    assert_eq!(event["control"]["model"]["model"], "target-model-a");
    let model_overrides = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let first_started = Arc::new(tokio::sync::Notify::new());
    let release_first = Arc::new(tokio::sync::Notify::new());
    state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime")
        .set_react_runtime(Arc::new(FirstTurnHoldRuntime {
            calls: Arc::clone(&calls),
            model_overrides: Arc::clone(&model_overrides),
            first_started: Arc::clone(&first_started),
            release_first: Arc::clone(&release_first),
        }))
        .await;

    let input_intake = intake(&state);
    let first = input_intake
        .submit_delivery("default", request(CHAT_ID, "first"))
        .await;
    assert_local_delivery(first, CHAT_ID);
    tokio::time::timeout(Duration::from_secs(2), first_started.notified())
        .await
        .expect("first turn started");

    input_intake
        .update_authority_chat_model(
            "default",
            CHAT_ID,
            serde_json::json!({
                "provider_id": "target-provider",
                "model": "target-model-b"
            }),
        )
        .await
        .expect("save switched model");

    let mut second_request = request(CHAT_ID, "second");
    second_request.model = Some("source-model".to_string());
    second_request.extra.insert(
        "chat_model_slot".to_string(),
        serde_json::json!({
            "provider_id": "source-provider",
            "model": "source-model"
        }),
    );
    let second = input_intake
        .submit_delivery("default", second_request)
        .await;
    assert_authority_queue_delivery(second, CHAT_ID);

    release_first.notify_waiters();
    for _ in 0..80 {
        if calls.load(Ordering::SeqCst) >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        *model_overrides.lock().await,
        vec![
            Some("target-model-a".to_string()),
            Some("target-model-b".to_string())
        ]
    );
}

#[tokio::test]
async fn remote_conversation_input_uses_target_authority_chat_model_slot() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    let target_device_id = state
        .sync_device
        .read()
        .expect("sync device lock")
        .as_ref()
        .map(|device| hex::encode(device.device_id_short()))
        .expect("test sync device");
    seed_local_authority(&state, CHAT_ID, "Remote model slot").await;
    seed_model_switch_provider(&state, "target-provider", &["target-model"]).await;
    intake(&state).update_authority_chat_model("default", CHAT_ID, serde_json::json!({"provider_id": "target-provider", "model": "target-model", "thinking_strength": "medium"})).await.unwrap();
    let model_override = Arc::new(tokio::sync::Mutex::new(None));
    let started = Arc::new(tokio::sync::Notify::new());
    state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime")
        .set_react_runtime(Arc::new(RecordingRuntime {
            model_override: Arc::clone(&model_override),
            started: Arc::clone(&started),
        }))
        .await;

    let mut request = remote_request_to(
        CHAT_ID,
        "device-delivery-model",
        1,
        "remote hello",
        &target_device_id,
    );
    request.metadata.insert(
        "chat_model_slot".to_string(),
        serde_json::json!({
            "provider_id": "source-provider",
            "model": "source-model",
            "thinking_strength": "high"
        }),
    );

    let ack = intake(&state)
        .accept_remote_conversation_input(request, "2026-07-19T00:00:00Z".to_string())
        .await
        .expect("remote request accepted");
    assert_eq!(ack.device_delivery_id, "device-delivery-model");
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .expect("remote conversation input started");
    assert_eq!(model_override.lock().await.as_deref(), Some("target-model"));

    let snapshot = state
        .conversation_authority
        .active_turn(&chat_address())
        .await
        .expect("remote conversation input creates authority snapshot");
    assert_eq!(
        snapshot
            .driver_detail
            .as_ref()
            .and_then(|detail| detail.get("metadata"))
            .and_then(|metadata| metadata.get("chat_model_slot"))
            .and_then(|slot| slot.get("provider_id"))
            .and_then(|provider| provider.as_str()),
        Some("target-provider")
    );
    assert_eq!(
        snapshot
            .driver_detail
            .as_ref()
            .and_then(|detail| detail.get("metadata"))
            .and_then(|metadata| metadata.get("chat_model_slot"))
            .and_then(|slot| slot.get("thinking_strength"))
            .and_then(|strength| strength.as_str()),
        Some("medium")
    );
}

#[tokio::test]
async fn local_submit_binds_authority_snapshot_to_canonical_turn_identity() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    let release = Arc::new(tokio::sync::Notify::new());
    state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime")
        .set_react_runtime(Arc::new(HoldRuntime {
            release: Arc::clone(&release),
        }))
        .await;

    let outcome = intake(&state)
        .submit_delivery("default", request(CHAT_ID, "hello identity"))
        .await;
    assert_local_delivery(outcome, CHAT_ID);

    for _ in 0..80 {
        if let Some(snapshot) = state
            .conversation_authority
            .active_turn(&chat_address())
            .await
        {
            if snapshot.user_message_id.is_some() {
                assert!(snapshot.turn_id.starts_with("turn-"));
                assert!(snapshot
                    .user_message_id
                    .as_deref()
                    .is_some_and(|message_id| message_id.starts_with("msg-")));
                release.notify_one();
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    release.notify_one();
    panic!("authority snapshot did not bind canonical turn identity before run ended");
}

#[tokio::test]
async fn active_agent_turn_keeps_model_and_thinking_captured_at_acceptance() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    seed_model_switch_provider(&state, "fixture", &["model-a", "model-b"]).await;
    seed_local_authority(&state, CHAT_ID, "Frozen model").await;
    let scope_store = ChatScopeStore::resolve(temp.path(), chat_address());
    let chat_service = ChatService::for_scope_store(scope_store.clone());
    let model_a = serde_json::json!({
        "provider_id": "fixture",
        "model": "model-a",
        "thinking_strength": "high"
    });
    chat_service
        .update_setting(&chat_address(), ChatSetting::Model(model_a))
        .await
        .expect("set initial Chat model");

    let release = Arc::new(tokio::sync::Notify::new());
    state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime")
        .set_react_runtime(Arc::new(HoldRuntime {
            release: Arc::clone(&release),
        }))
        .await;
    let outcome = intake(&state)
        .submit_delivery("default", request(CHAT_ID, "hold this model"))
        .await;
    assert_local_delivery(outcome, CHAT_ID);

    let store = WorkspaceSessionStore::new(scope_store.root());
    let (turn_id, user_message_id) = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let turns = store
                .read_turn_range(CHAT_ID, 1, 1)
                .await
                .expect("read active Turn");
            if let Some(turn) = turns.into_iter().next() {
                let messages = store
                    .read_turn_messages(CHAT_ID, &turn.turn_id)
                    .await
                    .expect("read active Turn messages");
                if let Some(message) = messages.into_iter().find(|message| message.role == "user") {
                    break (turn.turn_id, message.message_id);
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("active Turn user message becomes durable");

    intake(&state)
        .update_authority_chat_model(
            "default",
            CHAT_ID,
            serde_json::json!({
                "provider_id": "fixture",
                "model": "model-b",
                "thinking_strength": "low"
            }),
        )
        .await
        .expect("update Chat model for later Turns");

    let messages = store
        .read_turn_messages(CHAT_ID, &turn_id)
        .await
        .expect("reload active Turn messages");
    let user = messages
        .iter()
        .find(|message| message.message_id == user_message_id)
        .expect("active Turn user message");
    assert_eq!(
        user.metadata[TURN_MODEL_KEY],
        serde_json::json!({"provider_id": "fixture", "model": "model-a"})
    );
    assert_eq!(
        user.metadata[TURN_THINKING_KEY],
        serde_json::json!({"thinking_strength": "high"})
    );
    release.notify_one();

    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if state
                .conversation_authority
                .active_turn(&chat_address())
                .await
                .is_none()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("first Turn finishes");
    let outcome = intake(&state)
        .submit_delivery("default", request(CHAT_ID, "use the updated model"))
        .await;
    assert_local_delivery(outcome, CHAT_ID);
    let second_user = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let turns = store
                .read_turn_range(CHAT_ID, 1, 2)
                .await
                .expect("read later Turn");
            if let Some(turn) = turns.get(1) {
                let messages = store
                    .read_turn_messages(CHAT_ID, &turn.turn_id)
                    .await
                    .expect("read later Turn messages");
                if let Some(message) = messages.into_iter().find(|message| message.role == "user") {
                    break message;
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("later Turn user message becomes durable");
    assert_eq!(
        second_user.metadata[TURN_MODEL_KEY],
        serde_json::json!({"provider_id": "fixture", "model": "model-b"})
    );
    assert_eq!(
        second_user.metadata[TURN_THINKING_KEY],
        serde_json::json!({"thinking_strength": "low", "thinking_param_style":"effort", "reasoning_effort_options":["low","medium","high"]})
    );
    release.notify_one();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_local_submits_do_not_leave_silent_user_only_turns() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    let chats = [
        "00000000-0000-0000-0000-000000009931",
        "00000000-0000-0000-0000-000000009932",
        "00000000-0000-0000-0000-000000009933",
    ];
    let input_intake = intake(&state);

    for chat_id in chats {
        let outcome = input_intake
            .submit_delivery("default", request(chat_id, "tell me a long story"))
            .await;
        assert_local_delivery(outcome, chat_id);
    }

    let store = WorkspaceSessionStore::new(temp.path().join("workspaces/default"));
    for chat_id in chats {
        let mut terminal = None;
        let mut saw_failed_note = false;
        for _ in 0..80 {
            let turns = store
                .read_turn_range(chat_id, 1, usize::MAX)
                .await
                .expect("read turns");
            if let Some(turn) = turns.first() {
                if turn.lifecycle.status != "in_progress" {
                    terminal = Some(turn.lifecycle.status.clone());
                }
            }
            if let Ok(messages) = store.read_message_range(chat_id, 1, usize::MAX).await {
                saw_failed_note = messages
                    .iter()
                    .filter_map(rua_session::RuntimeNote::from_message_log_entry)
                    .any(|note| {
                        matches!(
                            note.payload,
                            rua_session::RuntimeNotePayload::ConversationRuntimeTerminal(
                                ref terminal
                            ) if terminal.terminal_status == "failed"
                        )
                    });
            }
            if terminal.as_deref() == Some("failed") && saw_failed_note {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert_eq!(
            terminal.as_deref(),
            Some("failed"),
            "concurrent submit {chat_id} must terminalize visibly"
        );
        assert!(
            saw_failed_note,
            "concurrent submit {chat_id} must persist a failed runtime terminal note"
        );
    }
}

#[tokio::test]
async fn intake_marks_acp_bound_chat_as_acp_driver() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    let workspace = temp.path().join("workspaces/default");
    let device_id = local_device_id(&state);
    materialize_targeted_chat(
        &workspace,
        "default",
        CHAT_ID,
        "ACP driver",
        &ConversationAuthorityPlacement {
            device_id: device_id.clone(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            base_manifest_hash:
                "sha256:1111111111111111111111111111111111111111111111111111111111111111"
                    .to_string(),
        },
    )
    .await
    .expect("seed authority");
    capture_fixture_configuration(&state, CHAT_ID).await;
    persist_acp_binding(
        None,
        &workspace,
        &rua_core::ChatAddress::agent("default", CHAT_ID),
        "default",
        AcpBindingMetadata {
            target_device_id: Some(device_id),
            runner: "codex".to_string(),
            cwd: None,
            runner_session_id: Some("acp-session-1".to_string()),
        },
    )
    .await
    .expect("seed ACP binding");

    let outcome = intake(&state)
        .submit_delivery("default", request(CHAT_ID, "hello acp"))
        .await;
    assert_local_delivery(outcome, CHAT_ID);
    let snapshot = state
        .conversation_authority
        .active_turn(&chat_address())
        .await
        .expect("local submit creates authority run snapshot");
    assert_eq!(snapshot.driver, ConversationDriverKind::Acp);
    assert_eq!(
        snapshot
            .driver_detail
            .as_ref()
            .and_then(|detail| detail.get("runner"))
            .and_then(|runner| runner.as_str()),
        Some("codex")
    );
}

#[tokio::test]
async fn acp_passive_queue_uses_conversation_authority_not_stream_pending() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    seed_local_authority(&state, CHAT_ID, "ACP authority").await;
    let key = stream_key();
    let (_producer_id, _cancel_rx) = state.streams.register_active_keyed(&key).await;
    state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), CHAT_ID.to_string()),
            driver: ConversationDriverKind::Acp,
            authority_device_id: local_device_id(&state),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: Some(serde_json::json!({ "runner": "codex" })),
        })
        .await;

    let outcome = state
        .console_facade
        .submit_chat("default", request(CHAT_ID, "queued acp input"))
        .await;

    assert_eq!(outcome.kind, ConsoleSubmitKind::Queued);
    assert!(outcome.input_id.is_some());
    let control = state
        .conversation_authority
        .control_view(&chat_address(), &serde_json::json!({}))
        .await;
    assert_eq!(control.queue.entries.len(), 1);
    assert_eq!(control.queue.entries[0].text, "queued acp input");
}

#[tokio::test]
async fn acp_send_now_targets_queued_input_and_signals_active_producer() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    seed_local_authority(&state, CHAT_ID, "ACP Send Now authority").await;
    let key = stream_key();
    let (_producer_id, cancel_rx) = state.streams.register_active_keyed(&key).await;
    state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), CHAT_ID.to_string()),
            driver: ConversationDriverKind::Acp,
            authority_device_id: local_device_id(&state),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: Some(serde_json::json!({ "runner": "codex" })),
        })
        .await;

    let mut queued_request = request(CHAT_ID, "urgent acp input");
    queued_request.extra.insert(
        "submission_id".to_string(),
        serde_json::json!("submission-urgent-acp"),
    );
    let queued = state
        .console_facade
        .submit_chat("default", queued_request)
        .await;

    assert_eq!(queued.kind, ConsoleSubmitKind::Queued);
    assert_eq!(
        queued.submission_id.as_deref(),
        Some("submission-urgent-acp")
    );
    let input_id = queued.input_id.expect("queued input id");
    let promoted = state
        .conversation_input_intake
        .send_now(
            "default",
            CHAT_ID,
            &input_id,
            epoch(1),
            "send-now-urgent-acp",
        )
        .await
        .expect("send now");

    assert_eq!(promoted.input_id, input_id);
    assert!(
        cancel_rx.borrow().is_interject(),
        "active ACP Send Now must signal the active producer"
    );
    let control = state
        .conversation_authority
        .control_view(&chat_address(), &serde_json::json!({}))
        .await;
    assert_eq!(control.queue.entries.len(), 1);
    assert_eq!(control.queue.entries[0].input_id, input_id);
    assert_eq!(control.queue.entries[0].text, "urgent acp input");
}

#[tokio::test]
async fn local_producer_exit_clears_authority_snapshot() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);

    let outcome = intake(&state)
        .submit_delivery("default", request(CHAT_ID, "hello without model"))
        .await;
    assert_local_delivery(outcome, CHAT_ID);
    assert!(
        state
            .conversation_authority
            .active_turn(&chat_address())
            .await
            .is_some(),
        "submit should create an active authority snapshot before producer exits"
    );

    let mut cleared = false;
    for _ in 0..80 {
        if state
            .conversation_authority
            .active_turn(&chat_address())
            .await
            .is_none()
        {
            cleared = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    assert!(cleared, "producer exit did not clear authority snapshot");

    let store = WorkspaceSessionStore::new(temp.path().join("workspaces/default"));
    let turns = store
        .read_turn_range(CHAT_ID, 1, usize::MAX)
        .await
        .expect("read turns");
    assert_eq!(
        turns[0].lifecycle.status, "failed",
        "runtime/provider failure must persist a failed terminal lifecycle"
    );
    let messages = store
        .read_message_range(CHAT_ID, 1, usize::MAX)
        .await
        .expect("read messages");
    assert!(
        messages
            .iter()
            .filter_map(rua_session::RuntimeNote::from_message_log_entry)
            .any(|note| {
                matches!(
                    note.payload,
                    rua_session::RuntimeNotePayload::ConversationRuntimeTerminal(
                        ref terminal
                    ) if terminal.terminal_status == "failed"
                )
            }),
        "runtime/provider failure must persist a failed runtime terminal note"
    );
}

#[tokio::test]
async fn agent_scoped_submit_persists_compact_turn_agent_assignment() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);

    let outcome = intake(&state)
        .submit_delivery("default", request(CHAT_ID, "capture agent assignment"))
        .await;
    assert_local_delivery(outcome, CHAT_ID);

    let store = WorkspaceSessionStore::new(temp.path().join("workspaces/default"));
    let address = ChatAddress::agent("default", CHAT_ID);
    let chat_service =
        ChatService::for_resolved_scope(temp.path().join("workspaces/default"), address.clone());
    let assignment = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let turns = store
                .read_turn_range(CHAT_ID, 1, 1)
                .await
                .expect("read accepted Turn");
            if let Some(turn) = turns.into_iter().next() {
                if let Ok(assignment) = chat_service
                    .load_turn_agent_assignment(&address, &turn.turn_id)
                    .await
                {
                    break assignment;
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("accepted Turn Agent Assignment becomes durable");

    assert_eq!(assignment.agent_id, "default");
    assert!(!assignment.agent_name.trim().is_empty());
    let persisted = serde_json::to_value(assignment).expect("serialize assignment");
    assert!(persisted.get("configuration").is_none());
}

#[tokio::test]
async fn panicking_runtime_clears_authority_and_persists_failed_terminal() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime")
        .set_react_runtime(Arc::new(PanickingRuntime))
        .await;

    let outcome = intake(&state)
        .submit_delivery("default", request(CHAT_ID, "trigger panic"))
        .await;
    assert_local_delivery(outcome, CHAT_ID);

    let store = WorkspaceSessionStore::new(temp.path().join("workspaces/default"));
    let mut terminal = None;
    let mut saw_failed_note = false;
    let mut authority_cleared = false;
    for _ in 0..80 {
        let turns = store
            .read_turn_range(CHAT_ID, 1, usize::MAX)
            .await
            .expect("read turns");
        terminal = turns
            .first()
            .filter(|turn| turn.lifecycle.status != "in_progress")
            .map(|turn| turn.lifecycle.status.clone());
        if let Ok(messages) = store.read_message_range(CHAT_ID, 1, usize::MAX).await {
            saw_failed_note = messages
                .iter()
                .filter_map(rua_session::RuntimeNote::from_message_log_entry)
                .any(|note| {
                    matches!(
                        note.payload,
                        rua_session::RuntimeNotePayload::ConversationRuntimeTerminal(
                            ref terminal
                        ) if terminal.terminal_status == "failed"
                            && terminal.summary.contains("simulated provider panic")
                    )
                });
        }
        authority_cleared = state
            .conversation_authority
            .active_turn(&chat_address())
            .await
            .is_none();
        if terminal.as_deref() == Some("failed") && saw_failed_note && authority_cleared {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    assert_eq!(
        terminal.as_deref(),
        Some("failed"),
        "runtime panic must not leave an in-progress or successful turn"
    );
    assert!(
        saw_failed_note,
        "runtime panic must persist a visible failed terminal note"
    );
    assert!(
        authority_cleared,
        "runtime panic must clear the active authority snapshot"
    );
}

#[tokio::test]
async fn intake_queues_to_existing_active_authority_queue() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    seed_local_authority(&state, CHAT_ID, "Queued authority").await;
    let key = stream_key();
    let (_producer_id, _cancel_rx) = state.streams.register_active_keyed(&key).await;
    state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), CHAT_ID.to_string()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: local_device_id(&state),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: None,
        })
        .await;

    let mut second = request(CHAT_ID, "second");
    second.extra.insert(
        "submission_id".to_string(),
        serde_json::json!("submission-local-second"),
    );
    let outcome = intake(&state).submit_delivery("default", second).await;

    match outcome {
        ConversationInputDeliveryOutcome::Delivered {
            chat_id,
            transport: ConversationInputDeliveryTransport::LocalAuthority,
            accepted: true,
            queued_to_active_authority: true,
            submission_id,
            ..
        } => {
            assert_eq!(chat_id, CHAT_ID);
            assert_eq!(submission_id.as_deref(), Some("submission-local-second"));
        }
        other => panic!("expected queued authority delivery, got {other:?}"),
    }
    let snapshot = state
        .conversation_authority
        .control_view(&chat_address(), &serde_json::json!({}))
        .await
        .active_turn
        .expect("queued submit keeps active authority snapshot");
    assert_eq!(snapshot.queued_count, 1);
    let control = state
        .conversation_authority
        .control_view(&chat_address(), &serde_json::json!({}))
        .await;
    assert_eq!(control.queue.entries.len(), 1);
    assert_eq!(control.queue.entries[0].text, "second");
    assert_eq!(
        control.queue.entries[0].submission_id,
        "submission-local-second"
    );
    assert!(control.queue.entries[0].input_id.starts_with("input-"));

    assert!(
        state
            .streams
            .is_active_for_chat(CONSOLE_CHANNEL_ID, &chat_address())
            .await,
        "execution registry remains active without owning Conversation inputs"
    );
}

#[tokio::test]
async fn task_watch_output_continues_active_turn_at_boundary_without_user_interjection() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    seed_local_authority(&state, CHAT_ID, "Task Watch authority").await;
    let key = stream_key();
    let (_producer_id, mut cancel_rx) = state.streams.register_active_keyed(&key).await;
    state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: chat_address(),
            driver: ConversationDriverKind::Rua,
            authority_device_id: local_device_id(&state),
            authority_epoch: ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: None,
        })
        .await;

    let intake = state.conversation_input_intake.clone();
    let delivery = tokio::spawn(async move {
        intake
            .deliver_task_watch_output(rua_task::TaskWatchOutputNotification {
                notification_id: "task-watch-active-1".to_string(),
                owner: rua_task::TaskOwner::new("default", CHAT_ID),
                task_id: "task-active".to_string(),
                attempt_no: 1,
                origin_turn_id: Some("turn-active".to_string()),
                projection: rua_task::TaskLiveOutputProjection::ModelVisible,
                records: vec![rua_task::TaskLiveOutputRecord {
                    source: "stdout".to_string(),
                    text: "line".to_string(),
                }],
            })
            .await
    });

    tokio::time::timeout(Duration::from_secs(2), cancel_rx.changed())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(*cancel_rx.borrow(), RunControl::ContinueAtBoundary);
    let control = state
        .conversation_authority
        .control_view(&chat_address(), &serde_json::json!({}))
        .await;
    assert!(control.queue.entries.is_empty());
    assert_eq!(control.active_turn.unwrap().queued_count, 0);
    assert!(!delivery.is_finished());

    let stop_intake = state.conversation_input_intake.clone();
    let stop =
        tokio::spawn(async move { stop_intake.request_local_stop("default", CHAT_ID).await });
    tokio::time::sleep(Duration::from_millis(25)).await;
    state
        .conversation_authority
        .finish_active_turn(&chat_address())
        .await;
    assert!(stop.await.unwrap());
    assert!(
        !delivery.is_finished(),
        "Stop must not discard an unprocessed Task observation"
    );
    assert_eq!(
        state
            .conversation_authority
            .queue_snapshot(&chat_address())
            .await
            .len(),
        1
    );
    delivery.abort();
}

#[tokio::test]
async fn task_watch_delivery_waits_for_notification_turn_completion() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    seed_local_authority(&state, CHAT_ID, "Task Watch receipt").await;
    let first_started = Arc::new(tokio::sync::Notify::new());
    let release_first = Arc::new(tokio::sync::Notify::new());
    state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime")
        .set_react_runtime(Arc::new(FirstTurnHoldRuntime {
            calls: Arc::new(AtomicUsize::new(0)),
            model_overrides: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            first_started: Arc::clone(&first_started),
            release_first: Arc::clone(&release_first),
        }))
        .await;

    let intake = state.conversation_input_intake.clone();
    let delivery = tokio::spawn(async move {
        intake
            .deliver_task_watch_output(rua_task::TaskWatchOutputNotification {
                notification_id: "task-watch-receipt-1".to_string(),
                owner: rua_task::TaskOwner::new("default", CHAT_ID),
                task_id: "task-receipt".to_string(),
                attempt_no: 1,
                origin_turn_id: None,
                projection: rua_task::TaskLiveOutputProjection::ModelVisible,
                records: vec![rua_task::TaskLiveOutputRecord {
                    source: "stdout".to_string(),
                    text: "wake".to_string(),
                }],
            })
            .await
    });

    tokio::time::timeout(Duration::from_secs(2), first_started.notified())
        .await
        .expect("notification Turn started");
    assert!(
        !delivery.is_finished(),
        "queue acceptance must not acknowledge Task Watch delivery"
    );

    release_first.notify_one();
    tokio::time::timeout(Duration::from_secs(2), delivery)
        .await
        .expect("delivery acknowledgement")
        .expect("delivery task")
        .expect("watch delivery");
}

#[tokio::test]
async fn delegated_task_watch_output_does_not_interrupt_the_parent_turn() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    seed_local_authority(&state, CHAT_ID, "Delegated Task Watch authority").await;
    let key = stream_key();
    let (_producer_id, mut cancel_rx) = state.streams.register_active_keyed(&key).await;
    state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: chat_address(),
            driver: ConversationDriverKind::Rua,
            authority_device_id: local_device_id(&state),
            authority_epoch: ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: None,
        })
        .await;

    state
        .conversation_input_intake
        .deliver_task_watch_output(rua_task::TaskWatchOutputNotification {
            notification_id: "task-watch-delegated-1".to_string(),
            owner: rua_task::TaskOwner::new("default", CHAT_ID),
            task_id: "task-agent".to_string(),
            attempt_no: 1,
            origin_turn_id: Some("turn-active".to_string()),
            projection: rua_task::TaskLiveOutputProjection::ActivityOnly,
            records: vec![rua_task::TaskLiveOutputRecord {
                source: "subagent".to_string(),
                text: "internal progress".to_string(),
            }],
        })
        .await
        .unwrap();

    assert!(
        tokio::time::timeout(Duration::from_millis(50), cancel_rx.changed())
            .await
            .is_err(),
        "delegated execution activity must not schedule another parent model call"
    );
}

#[tokio::test]
async fn intake_queues_existing_active_authority() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    seed_local_authority(&state, CHAT_ID, "CLI busy authority").await;
    let key = stream_key();
    let (_producer_id, _cancel_rx) = state.streams.register_active_keyed(&key).await;
    state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: chat_address(),
            driver: ConversationDriverKind::Rua,
            authority_device_id: local_device_id(&state),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: None,
        })
        .await;

    let outcome = intake(&state)
        .submit_delivery("default", request(CHAT_ID, "second"))
        .await;

    match outcome {
        ConversationInputDeliveryOutcome::Delivered {
            transport: ConversationInputDeliveryTransport::LocalAuthority,
            accepted: true,
            queued_to_active_authority: true,
            ..
        } => {}
        other => panic!("expected queued authority delivery, got {other:?}"),
    }
    let control = state
        .conversation_authority
        .control_view(&chat_address(), &serde_json::json!({}))
        .await;
    assert_eq!(control.queue.entries.len(), 1);
    assert_eq!(control.queue.entries[0].text, "second");
    assert!(control.active_turn.is_some());
}

#[tokio::test]
async fn submit_delivery_rejects_orphan_active_turn_without_durable_placement() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), CHAT_ID.to_string()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "remote-device".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: None,
        })
        .await;

    let outcome = intake(&state)
        .submit_delivery("default", request(CHAT_ID, "remote active follow-up"))
        .await;

    match outcome {
        rua_server::services::conversation_input_intake::ConversationInputDeliveryOutcome::Rejected {
            target_device_id,
            detail,
            failure_kind,
        } => {
            assert!(target_device_id.is_none());
            assert!(detail.contains("without durable Conversation Authority Placement"));
            assert_eq!(failure_kind.as_deref(), Some("business"));
        }
        other => panic!("expected rejected orphan authority state, got {other:?}"),
    }

    let control = state
        .conversation_authority
        .control_view(&chat_address(), &serde_json::json!({}))
        .await;
    assert!(
        control.queue.entries.is_empty(),
        "source-side submit must not enter the local authority queue when active authority is remote"
    );
}

#[tokio::test]
async fn local_producer_drains_next_input_from_conversation_authority_queue() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    let store = WorkspaceSessionStore::new(temp.path().join("workspaces/default"));
    let mut live_subscription = state
        .active_stream_broadcast_hub
        .subscribe_chat(&chat_address(), &BTreeMap::new())
        .await;
    let live_rx = live_subscription
        .rx
        .as_mut()
        .expect("Chat Live Activity subscription");

    let mut first_request = request(CHAT_ID, "first without model");
    first_request.extra.insert(
        "submission_id".to_string(),
        serde_json::json!("submission-first"),
    );
    let first = intake(&state)
        .submit_delivery("default", first_request)
        .await;
    assert_local_delivery(first, CHAT_ID);
    let mut second_request = request(CHAT_ID, "second without model");
    second_request.extra.insert(
        "submission_id".to_string(),
        serde_json::json!("submission-second"),
    );
    let second = intake(&state)
        .submit_delivery("default", second_request)
        .await;
    assert_authority_queue_delivery(second, CHAT_ID);
    assert_eq!(
        state
            .conversation_authority
            .control_view(&chat_address(), &serde_json::json!({}))
            .await
            .queue
            .entries
            .len(),
        1
    );

    let mut saw_two_terminal_turns = false;
    for _ in 0..320 {
        let turns = store
            .read_turn_range(CHAT_ID, 1, usize::MAX)
            .await
            .expect("read turns");
        if turns.len() >= 2
            && turns
                .iter()
                .take(2)
                .all(|turn| turn.lifecycle.status != "in_progress")
        {
            saw_two_terminal_turns = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        saw_two_terminal_turns,
        "producer should drain authority-queued input through normal lifecycle"
    );
    let turns = store
        .read_turn_range(CHAT_ID, 1, 2)
        .await
        .expect("read terminal turns");
    for (turn, expected_submission_id) in
        turns.iter().zip(["submission-first", "submission-second"])
    {
        let messages = store
            .read_turn_messages(CHAT_ID, &turn.turn_id)
            .await
            .expect("read turn messages");
        let user = messages
            .iter()
            .find(|message| message.role == "user")
            .expect("user message");
        assert_eq!(
            user.metadata["submission_id"],
            serde_json::json!(expected_submission_id)
        );
    }

    let first_turn_id = &turns[0].turn_id;
    let second_turn_id = &turns[1].turn_id;
    let mut payloads = Vec::new();
    while payloads.len() < 256 {
        let frame = tokio::time::timeout(Duration::from_millis(100), live_rx.recv())
            .await
            .expect("expected ordered Live Activity frame")
            .expect("Live Activity stream");
        let ActiveStreamBroadcastFrame::Event { payload, .. } = frame else {
            continue;
        };
        let value: serde_json::Value = serde_json::from_str(&payload).expect("Live payload JSON");
        let reached_second_user = value.get("type").and_then(serde_json::Value::as_str)
            == Some("user_message")
            && value
                .pointer("/message/turn_id")
                .and_then(serde_json::Value::as_str)
                == Some(second_turn_id.as_str());
        payloads.push(value);
        if reached_second_user {
            break;
        }
    }
    let position = |predicate: &dyn Fn(&serde_json::Value) -> bool| {
        payloads
            .iter()
            .position(predicate)
            .expect("expected ordered Live Activity payload")
    };
    let terminal = position(&|value| {
        value.get("type").and_then(serde_json::Value::as_str) == Some("turn_terminal")
            && value.get("turn_id").and_then(serde_json::Value::as_str)
                == Some(first_turn_id.as_str())
    });
    let idle = position(&|value| {
        value.get("type").and_then(serde_json::Value::as_str) == Some("conversation_control")
            && value.get("reason").and_then(serde_json::Value::as_str) == Some("turn_completed")
            && value
                .pointer("/control/active_turn")
                .is_none_or(serde_json::Value::is_null)
    });
    let next_started = position(&|value| {
        value.get("type").and_then(serde_json::Value::as_str) == Some("conversation_control")
            && value.get("reason").and_then(serde_json::Value::as_str)
                == Some("active_turn_started")
            && value
                .pointer("/control/active_turn/turn_id")
                .and_then(serde_json::Value::as_str)
                == Some(second_turn_id.as_str())
    });
    let next_user = position(&|value| {
        value.get("type").and_then(serde_json::Value::as_str) == Some("user_message")
            && value
                .pointer("/message/turn_id")
                .and_then(serde_json::Value::as_str)
                == Some(second_turn_id.as_str())
    });
    assert!(
        terminal < idle && idle < next_started && next_started < next_user,
        "Turn transition must publish terminal -> idle -> next start -> next user"
    );

    assert!(
        state
            .conversation_authority
            .control_view(&chat_address(), &serde_json::json!({}))
            .await
            .queue
            .entries
            .is_empty(),
        "authority queue should be empty after queued input is consumed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_started_producer_drains_local_and_remote_followups_from_one_authority_queue() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    let store = WorkspaceSessionStore::new(temp.path().join("workspaces/default"));
    let target_device_id = state
        .sync_device
        .read()
        .expect("sync device lock")
        .as_ref()
        .map(|device| hex::encode(device.device_id_short()))
        .expect("test sync device");
    seed_local_authority(&state, CHAT_ID, "Remote queue").await;
    let first_started = Arc::new(tokio::sync::Notify::new());
    let release_first = Arc::new(tokio::sync::Notify::new());
    state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime")
        .set_react_runtime(Arc::new(FirstTurnHoldRuntime {
            calls: Arc::new(AtomicUsize::new(0)),
            model_overrides: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            first_started: Arc::clone(&first_started),
            release_first: Arc::clone(&release_first),
        }))
        .await;

    let input_intake = intake(&state);
    let ack = input_intake
        .accept_remote_conversation_input(
            remote_request_to(
                CHAT_ID,
                "device-delivery-1",
                1,
                "remote first",
                &target_device_id,
            ),
            "2026-06-27T00:00:01Z".to_string(),
        )
        .await
        .expect("remote first accepted");
    assert_eq!(ack.device_delivery_id, "device-delivery-1");
    tokio::time::timeout(Duration::from_secs(2), first_started.notified())
        .await
        .expect("first remote conversation input started");

    let local_followup = input_intake
        .submit_delivery("default", request(CHAT_ID, "target local follow-up"))
        .await;
    assert_authority_queue_delivery(local_followup, CHAT_ID);
    let remote_followup = input_intake
        .accept_remote_conversation_input(
            remote_request_to(
                CHAT_ID,
                "device-delivery-2",
                2,
                "source remote follow-up",
                &target_device_id,
            ),
            "2026-06-27T00:00:02Z".to_string(),
        )
        .await
        .expect("remote follow-up accepted");
    assert_eq!(remote_followup.device_delivery_id, "device-delivery-2");

    let control = state
        .conversation_authority
        .control_view(&chat_address(), &serde_json::json!({}))
        .await;
    let queued_texts = control
        .queue
        .entries
        .iter()
        .map(|entry| entry.text.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        queued_texts,
        vec!["target local follow-up", "source remote follow-up"]
    );
    assert_eq!(
        control.queue.entries[1].device_delivery_id.as_deref(),
        Some("device-delivery-2")
    );

    release_first.notify_waiters();
    let mut saw_three_turns = false;
    let mut last_statuses = Vec::new();
    for _ in 0..320 {
        let turns = store
            .read_turn_range(CHAT_ID, 1, usize::MAX)
            .await
            .expect("read turns");
        last_statuses = turns
            .iter()
            .map(|turn| format!("{}:{}", turn.turn_id, turn.lifecycle.status))
            .collect();
        if turns.len() >= 3 {
            saw_three_turns = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let final_control = state
        .conversation_authority
        .control_view(&chat_address(), &serde_json::json!({}))
        .await;
    assert!(
        saw_three_turns,
        "remote-started producer should execute local and remote follow-ups from the same authority queue; turns={last_statuses:?}; queue={:?}; run={:?}",
        final_control.queue.entries,
        final_control.active_turn
    );
    assert!(
        final_control.queue.entries.is_empty(),
        "authority queue should be empty after all follow-ups are consumed"
    );
}

#[tokio::test]
async fn local_stop_waits_for_terminal_handoff_before_success() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    seed_local_authority(&state, CHAT_ID, "Local stop handoff").await;
    let key = stream_key();
    let (producer_id, mut cancel_rx) = state.streams.register_active_keyed(&key).await;
    let active_turn = state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), CHAT_ID.to_string()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: local_device_id(&state),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: None,
        })
        .await;
    let authority = Arc::clone(&state.conversation_authority);
    let streams = Arc::clone(&state.streams);
    let handoff_key = key.clone();
    let turn_id = active_turn.turn_id.clone();
    let handoff = tokio::spawn(async move {
        cancel_rx.changed().await.expect("stop signal");
        tokio::time::sleep(Duration::from_millis(100)).await;
        authority
            .complete_active_turn_and_drain_queue(
                streams.as_ref(),
                &handoff_key,
                &chat_address(),
                &turn_id,
                "interrupted",
                Some(chrono::Utc::now().to_rfc3339()),
                None,
                false,
                |_| async {},
            )
            .await
            .expect("complete local handoff");
        streams.mark_completed(&handoff_key, producer_id).await;
    });

    let started = Instant::now();
    let outcome = state
        .console_facade
        .stop_chat_outcome("default", CHAT_ID)
        .await;

    assert!(outcome.stopped);
    assert!(started.elapsed() >= Duration::from_millis(100));
    handoff.await.expect("handoff task");
    assert!(state
        .conversation_authority
        .active_turn(&chat_address())
        .await
        .is_none());
    assert!(
        state.streams.subscribe_keyed(&key).await.is_none(),
        "Stop should close viewer subscriptions through the display adapter"
    );
}

#[tokio::test]
async fn local_stop_without_authority_snapshot_does_not_treat_stream_as_business_turn() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    let key = stream_key();
    let (_producer_id, _cancel_rx) = state.streams.register_active_keyed(&key).await;

    let outcome = state
        .console_facade
        .stop_chat_outcome("default", CHAT_ID)
        .await;

    assert!(
        !outcome.stopped,
        "stream registry presence alone must not define a business run"
    );
}

#[tokio::test]
async fn local_interject_requires_authority_snapshot() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    let key = stream_key();
    let (_producer_id, _cancel_rx) = state.streams.register_active_keyed(&key).await;

    let applied = intake(&state)
        .request_local_interjection("default", CHAT_ID)
        .await;

    assert!(
        applied.is_none(),
        "stream registry presence alone must not define an interjectable business run"
    );
}

#[tokio::test]
async fn local_submit_with_active_authority_queues_through_authority() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    seed_local_authority(&state, CHAT_ID, "Local authority").await;
    let key = stream_key();
    let (_producer_id, _cancel_rx) = state.streams.register_active_keyed(&key).await;
    state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), CHAT_ID.to_string()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: local_device_id(&state),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: None,
        })
        .await;

    let outcome = state
        .console_facade
        .submit_chat("default", request(CHAT_ID, "send now"))
        .await;

    assert_eq!(outcome.kind, ConsoleSubmitKind::Queued);
    let snapshot = state
        .conversation_authority
        .active_turn(&chat_address())
        .await
        .expect("active authority snapshot");
    assert_eq!(snapshot.queued_count, 1);
}

#[tokio::test]
async fn panicked_producer_persists_failed_terminal_fact_and_drains_stream() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    let key = stream_key();
    let turn_id = "turn-panic-finalizer";
    let store = WorkspaceSessionStore::new(temp.path().join("workspaces/default"));
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: CHAT_ID.to_string(),
            agent_id: "default".to_string(),
            title: "panic finalizer".to_string(),
            created_at: "2026-07-16T00:00:00Z".to_string(),
            last_activity_at: "2026-07-16T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .expect("save header");
    store
        .append_conversation_turn(
            CHAT_ID,
            NewConversationTurn {
                turn_id: turn_id.to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-07-16T00:00:00Z".to_string(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![NewConversationMessage {
                    message_id: "msg-panic-user".to_string(),
                    role: "user".to_string(),
                    created_at: "2026-07-16T00:00:00Z".to_string(),
                    status: "completed".to_string(),
                    content: serde_json::json!([{ "type": "text", "text": "hello" }]),
                    provider: None,
                    tool: None,
                    metadata: serde_json::Map::new(),
                }],
            },
        )
        .await
        .expect("append in-progress turn");
    let (producer_id, _cancel_rx) = state.streams.register_active_keyed(&key).await;
    assert!(
        state
            .streams
            .set_active_turn_id(
                CONSOLE_CHANNEL_ID,
                &chat_address(),
                Priority::Normal,
                turn_id
            )
            .await
    );
    state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), CHAT_ID.to_string()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: local_device_id(&state),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: None,
        })
        .await;

    finalize_panicked_console_producer(PanickedConsoleProducerContext {
        panic_key: key.clone(),
        producer_id,
        panic_streams: Arc::clone(&state.streams),
        panic_workspace_registry: Arc::clone(&state.registry),
        session_sync: session_sync(&state),
        panic_conversation_authority: Some(Arc::clone(&state.conversation_authority)),
        panic_agent_id: "default".to_string(),
        panic_chat_id: CHAT_ID.to_string(),
        panic_scope_root: temp.path().join("workspaces/default"),
        detail: "boom".to_string(),
    })
    .await;

    assert!(
        !state
            .streams
            .is_active_for_chat(CONSOLE_CHANNEL_ID, &chat_address())
            .await,
        "panic finalizer must drain the active stream entry"
    );
    assert!(
        state
            .conversation_authority
            .active_turn(&chat_address())
            .await
            .is_none(),
        "panic finalizer must clear the active authority snapshot"
    );
    let turns = store
        .read_turn_range(CHAT_ID, 1, usize::MAX)
        .await
        .expect("read turns");
    assert_eq!(turns[0].lifecycle.status, "failed");
    let messages = store
        .read_message_range(CHAT_ID, 1, usize::MAX)
        .await
        .expect("read messages");
    assert!(
        messages
            .iter()
            .filter_map(rua_session::RuntimeNote::from_message_log_entry)
            .any(|note| {
                matches!(
                    note.payload,
                    rua_session::RuntimeNotePayload::ConversationRuntimeTerminal(
                        ref terminal
                    ) if terminal.terminal_status == "failed"
                        && terminal.summary.contains("producer panicked")
                )
            }),
        "panic finalizer must persist a visible runtime terminal note"
    );
}

fn session_sync(state: &AppState) -> SessionSyncEmitter {
    SessionSyncEmitter::new(
        state.rua_root.clone(),
        Arc::clone(&state.sync_enabled),
        Arc::clone(&state.sync_node),
        Arc::clone(&state.sync_network_secret),
        Arc::clone(&state.sync_device),
        Arc::clone(&state.sync_hlc_clock),
        Arc::clone(&state.sync_trigger),
    )
}

#[tokio::test]
async fn local_stop_preserves_user_queue_and_background_observations() {
    let temp = TempDir::new().unwrap();
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    let key = stream_key();
    let (_, mut cancel) = state.streams.register_active_keyed(&key).await;
    state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: chat_address(),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "device".into(),
            authority_epoch: epoch(1),
            driver_detail: None,
        })
        .await;
    for runtime_only in [false, true] {
        let mut extra = serde_json::Map::new();
        if runtime_only {
            extra.insert("runtime_observation".into(), serde_json::json!({}));
            extra.insert("background_result".into(), serde_json::json!(true));
        }
        state
            .conversation_authority
            .enqueue_input(
                &chat_address(),
                rua_server::services::conversation_authority::ConversationInputPayload {
                    message: format!("queued-{runtime_only}"),
                    model_override: None,
                    input: None,
                    messages: None,
                    extra,
                    attachments: Vec::new(),
                    enqueued_at: std::time::Instant::now(),
                },
                None,
            )
            .await
            .unwrap();
    }
    let intake = state.conversation_input_intake.clone();
    let stop = tokio::spawn(async move { intake.request_local_stop("default", CHAT_ID).await });
    tokio::time::timeout(Duration::from_secs(2), cancel.changed())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(*cancel.borrow(), RunControl::Stop);
    state
        .conversation_authority
        .finish_active_turn(&chat_address())
        .await;
    assert!(stop.await.unwrap());
    assert_eq!(
        state
            .conversation_authority
            .queue_snapshot(&chat_address())
            .await
            .len(),
        2
    );
    let control = state
        .conversation_authority
        .control_view(&chat_address(), &serde_json::json!({}))
        .await;
    assert!(control.queue.paused);
    assert_eq!(control.queue.entries.len(), 1);
    assert!(!control.background_work.suppressed);
}
