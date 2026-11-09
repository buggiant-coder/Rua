use std::future::pending;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use rua_agent::context_manager::{
    CompactContextRequest, CompactContextResult, ContextManager, ContextSummaryRequest,
    ContextSummaryResult, HistoryReplacementGenerationRequest, HistoryReplacementGenerationResult,
};
use rua_agent::error::AgentError;
use rua_channel::core::console::SessionPersistence;
use rua_contract::agent::AgentInputMessage;
use rua_core::{ContentPart, ToolCall, ToolResult};
use rua_server::services::console_chat::{materialize_targeted_chat, WorkspaceSessionPersistence};
use rua_server::services::conversation_authority::ConversationAuthority;
use rua_server::services::conversation_authority_placement::{
    ConversationAuthorityEpoch, ConversationAuthorityPlacement,
};
use rua_server::{AppState, ServerProfile};
use tokio::sync::{mpsc, Semaphore};

fn epoch(wall_ms: u64) -> ConversationAuthorityEpoch {
    ConversationAuthorityEpoch::from_parts(wall_ms, 0, [0; 8])
}

const AUTHORITY_DEVICE: &str = "authority-device";
const AUTHORITY_BASE_MANIFEST: &str =
    "sha256:1111111111111111111111111111111111111111111111111111111111111111";

// `materialize_targeted_chat` requires canonical UUID Chat ids.
const CHAT_ORDER: &str = "3f8f0a3e-1c2d-4e6a-9b1f-0a1b2c3d4e5f";
const CHAT_FOREGROUND: &str = "4a1b2c3d-5e6f-4a7b-8c9d-1e2f3a4b5c6d";
const CHAT_DISABLE: &str = "5b2c3d4e-6f7a-4b8c-9d0e-2f3a4b5c6d7e";
const CHAT_NO_AUTHORITY: &str = "6c3d4e5f-7a8b-4c9d-8e1f-3a4b5c6d7e8f";
const CHAT_TRANSFER: &str = "7d4e5f6a-8b9c-4d0e-9f2a-4b5c6d7e8f9a";

/// `save_chat_entry` is update-only and rejects unmaterialized Chats, so tests
/// must explicitly seed the Chat before persisting entries or messages for it.
async fn materialize_chat(workspace: &std::path::Path, agent_id: &str, chat_id: &str, name: &str) {
    materialize_targeted_chat(
        workspace,
        agent_id,
        chat_id,
        name,
        &ConversationAuthorityPlacement {
            device_id: AUTHORITY_DEVICE.to_string(),
            authority_epoch: epoch(1),
            base_manifest_hash: AUTHORITY_BASE_MANIFEST.to_string(),
        },
    )
    .await
    .unwrap();
}

fn running() -> Arc<rua_config::AgentsRunningConfig> {
    let mut running = rua_config::AgentsRunningConfig::default();
    running.context_compact.history_replacement_enabled = true;
    Arc::new(running)
}

fn model() -> rua_session::CheckpointModelIdentity {
    rua_session::CheckpointModelIdentity {
        provider: "provider-a".to_string(),
        model: "model-a".to_string(),
    }
}

fn text(role: &str, value: &str) -> AgentInputMessage {
    AgentInputMessage {
        role: role.to_string(),
        content: vec![ContentPart::Text {
            text: value.to_string(),
        }],
    }
}

fn tool_result(call_id: &str, marker: &str, size: usize) -> ContentPart {
    let mut result = ToolResult::text(call_id, format!("{marker}{}", "x".repeat(size)));
    result.name = Some("read".to_string());
    ContentPart::ToolResult { result }
}

fn two_block_messages() -> Vec<AgentInputMessage> {
    vec![
        text("user", "Inspect large and small files"),
        AgentInputMessage {
            role: "assistant".to_string(),
            content: vec![
                ContentPart::ToolUse {
                    call: ToolCall::new(
                        "call-large",
                        "read",
                        serde_json::json!({"path": "large.rs"}),
                    ),
                },
                tool_result("call-large", "largest-block-marker", 100_000),
                ContentPart::Text {
                    text: "Large file inspected.".to_string(),
                },
                ContentPart::ToolUse {
                    call: ToolCall::new(
                        "call-small",
                        "read",
                        serde_json::json!({"path": "small.rs"}),
                    ),
                },
                tool_result("call-small", "smaller-block-marker", 70_000),
                ContentPart::Text {
                    text: "Small file inspected.".to_string(),
                },
            ],
        },
    ]
}

fn one_block_messages(marker: &str) -> Vec<AgentInputMessage> {
    vec![
        text("user", "Inspect one file"),
        AgentInputMessage {
            role: "assistant".to_string(),
            content: vec![
                ContentPart::ToolUse {
                    call: ToolCall::new("call-1", "read", serde_json::json!({"path": "one.rs"})),
                },
                tool_result("call-1", marker, 70_000),
                ContentPart::Text {
                    text: "Inspection completed.".to_string(),
                },
            ],
        },
    ]
}

fn build_persistence(
    workspace: &std::path::Path,
    _chat_id: &str,
    loaded_message_count: usize,
    context_manager: Arc<dyn ContextManager>,
    authority: Arc<ConversationAuthority>,
) -> WorkspaceSessionPersistence {
    WorkspaceSessionPersistence::new(workspace.to_path_buf())
        .with_loaded_message_count(loaded_message_count)
        .with_authority(AUTHORITY_DEVICE.to_string(), epoch(1))
        .with_conversation_authority(authority)
        .with_context_manager(context_manager)
        .with_checkpoint_model(model())
        .with_running_config(running())
}

struct OrderedManager {
    calls: AtomicUsize,
    source_tx: mpsc::UnboundedSender<String>,
    permits: Arc<Semaphore>,
}

#[async_trait]
impl ContextManager for OrderedManager {
    async fn compact_context(
        &self,
        req: CompactContextRequest,
    ) -> Result<CompactContextResult, AgentError> {
        Ok(CompactContextResult {
            compacted_messages: req.messages,
            tokens_saved: 0,
            summary_generated: None,
            grade: None,
        })
    }

    async fn summarize_context(
        &self,
        _req: ContextSummaryRequest,
    ) -> Result<ContextSummaryResult, AgentError> {
        Ok(ContextSummaryResult {
            summary: "unused".to_string(),
        })
    }

    async fn generate_history_replacement(
        &self,
        req: HistoryReplacementGenerationRequest,
    ) -> Result<HistoryReplacementGenerationResult, AgentError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let _ = self.source_tx.send(req.source_content);
        self.permits.acquire().await.expect("test permit").forget();
        Ok(HistoryReplacementGenerationResult {
            digest: "Useful execution trace digest.".to_string(),
            finish_reason: "stop".to_string(),
            usage: None,
        })
    }
}

struct DropSignal(Arc<AtomicBool>);

impl Drop for DropSignal {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

struct PreemptibleManager {
    calls: AtomicUsize,
    first_started: Arc<tokio::sync::Notify>,
    first_dropped: Arc<AtomicBool>,
}

#[async_trait]
impl ContextManager for PreemptibleManager {
    async fn compact_context(
        &self,
        req: CompactContextRequest,
    ) -> Result<CompactContextResult, AgentError> {
        Ok(CompactContextResult {
            compacted_messages: req.messages,
            tokens_saved: 0,
            summary_generated: None,
            grade: None,
        })
    }

    async fn summarize_context(
        &self,
        _req: ContextSummaryRequest,
    ) -> Result<ContextSummaryResult, AgentError> {
        Ok(ContextSummaryResult {
            summary: "unused".to_string(),
        })
    }

    async fn generate_history_replacement(
        &self,
        _req: HistoryReplacementGenerationRequest,
    ) -> Result<HistoryReplacementGenerationResult, AgentError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            let _drop_signal = DropSignal(Arc::clone(&self.first_dropped));
            self.first_started.notify_one();
            pending::<()>().await;
            unreachable!("pending generation is cancelled by dropping its future");
        }
        Ok(HistoryReplacementGenerationResult {
            digest: "Replacement generated after retry.".to_string(),
            finish_reason: "stop".to_string(),
            usage: None,
        })
    }
}

async fn wait_for_ready_records(
    store: &rua_session::WorkspaceSessionStore,
    chat_id: &str,
    expected: usize,
) {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let blocks = store
                .detect_history_replacement_blocks(chat_id, 4.0)
                .await
                .unwrap();
            let mut ready = 0usize;
            for block in blocks {
                if store
                    .load_history_replacement(chat_id, &block.record_id)
                    .await
                    .unwrap()
                    .is_some_and(|record| record.ready_digest().is_some())
                {
                    ready += 1;
                }
            }
            if ready == expected {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("ready history replacement records");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authority_coordinator_runs_largest_first_with_one_in_flight_and_deduped_rescan() {
    let dir = tempfile::tempdir().unwrap();
    let authority = Arc::new(ConversationAuthority::new());
    let (source_tx, mut source_rx) = mpsc::unbounded_channel();
    let permits = Arc::new(Semaphore::new(0));
    let manager = Arc::new(OrderedManager {
        calls: AtomicUsize::new(0),
        source_tx,
        permits: Arc::clone(&permits),
    });
    let messages = two_block_messages();
    materialize_chat(dir.path(), "agent-1", CHAT_ORDER, "Ordering").await;
    let persistence = build_persistence(
        dir.path(),
        CHAT_ORDER,
        0,
        manager.clone(),
        Arc::clone(&authority),
    );
    persistence
        .save_chat_entry("agent-1", CHAT_ORDER, "Ordering")
        .await
        .unwrap();
    persistence
        .save_messages("agent-1", CHAT_ORDER, &messages)
        .await
        .unwrap();

    let first = tokio::time::timeout(std::time::Duration::from_secs(2), source_rx.recv())
        .await
        .expect("first candidate")
        .expect("source");
    assert!(first.contains("largest-block-marker"));

    build_persistence(
        dir.path(),
        CHAT_ORDER,
        messages.len(),
        manager.clone(),
        Arc::clone(&authority),
    )
    .save_messages("agent-1", CHAT_ORDER, &messages)
    .await
    .unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), source_rx.recv(),)
            .await
            .is_err(),
        "duplicate discovery must not create a second in-flight generation"
    );

    permits.add_permits(1);
    let second = tokio::time::timeout(std::time::Duration::from_secs(2), source_rx.recv())
        .await
        .expect("second candidate")
        .expect("source");
    assert!(second.contains("smaller-block-marker"));
    permits.add_permits(1);

    let store = rua_session::WorkspaceSessionStore::new(dir.path());
    wait_for_ready_records(&store, CHAT_ORDER, 2).await;
    assert_eq!(manager.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn foreground_cancels_in_flight_generation_without_rejection_then_retries() {
    let dir = tempfile::tempdir().unwrap();
    let authority = Arc::new(ConversationAuthority::new());
    let first_started = Arc::new(tokio::sync::Notify::new());
    let first_dropped = Arc::new(AtomicBool::new(false));
    let manager = Arc::new(PreemptibleManager {
        calls: AtomicUsize::new(0),
        first_started: Arc::clone(&first_started),
        first_dropped: Arc::clone(&first_dropped),
    });
    let messages = one_block_messages("foreground-marker");
    materialize_chat(dir.path(), "agent-1", CHAT_FOREGROUND, "Foreground").await;
    let persistence = build_persistence(
        dir.path(),
        CHAT_FOREGROUND,
        0,
        manager.clone(),
        Arc::clone(&authority),
    );
    persistence
        .save_chat_entry("agent-1", CHAT_FOREGROUND, "Foreground")
        .await
        .unwrap();
    persistence
        .save_messages("agent-1", CHAT_FOREGROUND, &messages)
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), first_started.notified())
        .await
        .expect("first generation");

    let foreground = authority.begin_history_replacement_foreground(&model());
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !first_dropped.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("generation future should be cancelled");
    let store = rua_session::WorkspaceSessionStore::new(dir.path());
    let block = store
        .detect_history_replacement_blocks(CHAT_FOREGROUND, 4.0)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    assert!(store
        .load_history_replacement(CHAT_FOREGROUND, &block.record_id)
        .await
        .unwrap()
        .is_none());
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(manager.calls.load(Ordering::SeqCst), 1);

    drop(foreground);
    wait_for_ready_records(&store, CHAT_FOREGROUND, 1).await;
    assert_eq!(manager.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabling_cancels_late_publish_and_reenable_backfills_same_gap() {
    let dir = tempfile::tempdir().unwrap();
    let authority = Arc::new(ConversationAuthority::new());
    let first_started = Arc::new(tokio::sync::Notify::new());
    let first_dropped = Arc::new(AtomicBool::new(false));
    let manager = Arc::new(PreemptibleManager {
        calls: AtomicUsize::new(0),
        first_started: Arc::clone(&first_started),
        first_dropped: Arc::clone(&first_dropped),
    });
    let messages = one_block_messages("disable-marker");
    materialize_chat(dir.path(), "agent-1", CHAT_DISABLE, "Disable").await;
    let persistence = build_persistence(
        dir.path(),
        CHAT_DISABLE,
        0,
        manager.clone(),
        Arc::clone(&authority),
    );
    persistence
        .save_chat_entry("agent-1", CHAT_DISABLE, "Disable")
        .await
        .unwrap();
    persistence
        .save_messages("agent-1", CHAT_DISABLE, &messages)
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), first_started.notified())
        .await
        .expect("first generation");

    authority.disable_history_replacement_for_agent("agent-1");
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !first_dropped.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("disabled generation future should be cancelled");
    let store = rua_session::WorkspaceSessionStore::new(dir.path());
    let block = store
        .detect_history_replacement_blocks(CHAT_DISABLE, 4.0)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(store
        .load_history_replacement(CHAT_DISABLE, &block.record_id)
        .await
        .unwrap()
        .is_none());

    build_persistence(
        dir.path(),
        CHAT_DISABLE,
        messages.len(),
        manager.clone(),
        authority,
    )
    .save_messages("agent-1", CHAT_DISABLE, &messages)
    .await
    .unwrap();
    wait_for_ready_records(&store, CHAT_DISABLE, 1).await;
    assert_eq!(manager.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn persistence_without_conversation_authority_cannot_schedule_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let (source_tx, mut source_rx) = mpsc::unbounded_channel();
    let manager = Arc::new(OrderedManager {
        calls: AtomicUsize::new(0),
        source_tx,
        permits: Arc::new(Semaphore::new(1)),
    });
    let messages = one_block_messages("no-authority-marker");
    materialize_chat(dir.path(), "agent-1", CHAT_NO_AUTHORITY, "No authority").await;
    let persistence = WorkspaceSessionPersistence::new(dir.path().to_path_buf())
        .with_authority(AUTHORITY_DEVICE.to_string(), epoch(1))
        .with_context_manager(manager.clone())
        .with_checkpoint_model(model())
        .with_running_config(running());
    persistence
        .save_chat_entry("agent-1", CHAT_NO_AUTHORITY, "No authority")
        .await
        .unwrap();
    persistence
        .save_messages("agent-1", CHAT_NO_AUTHORITY, &messages)
        .await
        .unwrap();

    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(150), source_rx.recv(),)
            .await
            .is_err()
    );
    assert_eq!(manager.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enabling_feature_backfills_existing_authority_owned_history() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::from_tempdir(dir.path().to_path_buf(), ServerProfile::Test);
    let runtime = state.registry.get_or_create("default").await.unwrap();
    let (source_tx, mut source_rx) = mpsc::unbounded_channel();
    let manager = Arc::new(OrderedManager {
        calls: AtomicUsize::new(0),
        source_tx,
        permits: Arc::new(Semaphore::new(1)),
    });
    runtime.set_context_manager(manager.clone()).await;

    let local_device_id = state
        .sync_device
        .read()
        .unwrap()
        .as_ref()
        .map(|device| hex::encode(device.device_id_short()))
        .expect("local device");
    let store = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir());
    let mut header = rua_session::ChatSessionHeader::new(rua_session::ChatMetadata {
        chat_id: "chat-backfill".to_string(),
        agent_id: "default".to_string(),
        title: "Backfill".to_string(),
        created_at: "2026-07-26T00:00:00Z".to_string(),
        last_activity_at: "2026-07-26T00:00:00Z".to_string(),
        pinned: false,
        archived: false,
        preview: None,
    });
    header.extra.insert(
        "authority_placement".to_string(),
        serde_json::json!({
            "device_id": local_device_id,
            "authority_epoch": "0000000000000001000000000000000000000000",
            "base_manifest_hash":
                "sha256:1111111111111111111111111111111111111111111111111111111111111111"
        }),
    );
    header.extra.insert(
        "model".to_string(),
        serde_json::json!({
            "provider_id": "provider-a",
            "model": "model-a"
        }),
    );
    let configuration = state
        .product_hooks
        .resolve_complete_chat_configuration(
            &rua_core::ChatAddress::agent("default", "chat-backfill"),
            rua_server::services::product_hooks::ChatConfigurationMaterialization {
                selected_agent_id: "default".into(),
                running_config: Default::default(),
                security_config: Default::default(),
                agent_assignment: None,
                prompt_file_content: None,
                tool_config_snapshot: Default::default(),
                capability_configuration: None,
                model: header.extra.get("model").cloned(),
                thinking: None,
                collaboration_mode: serde_json::json!("default"),
                harness_configuration: None,
                authority_device_id: local_device_id.clone(),
            },
        )
        .await
        .unwrap();
    header.extra.insert(
        "hook_configuration".into(),
        serde_json::to_value(configuration).unwrap(),
    );
    store.save_chat_header(&header).await.unwrap();
    store
        .append_conversation_turn(
            "chat-backfill",
            rua_session::NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: rua_session::TurnLifecycle {
                    user_submitted_at: "2026-07-26T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-07-26T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-07-26T00:00:03Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("variant-1".to_string()),
                messages: vec![
                    rua_session::NewConversationMessage {
                        message_id: "user-1".to_string(),
                        role: "user".to_string(),
                        created_at: "2026-07-26T00:00:00Z".to_string(),
                        status: "completed".to_string(),
                        content: serde_json::json!([
                            {"type": "text", "text": "Inspect old history"}
                        ]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                    rua_session::NewConversationMessage {
                        message_id: "assistant-process".to_string(),
                        role: "assistant".to_string(),
                        created_at: "2026-07-26T00:00:01Z".to_string(),
                        status: "completed".to_string(),
                        content: serde_json::json!([{
                            "type": "tool_use",
                            "id": "call-1",
                            "name": "read",
                            "input": {"path": "old.rs"}
                        }]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                    rua_session::NewConversationMessage {
                        message_id: "tool-result".to_string(),
                        role: "tool".to_string(),
                        created_at: "2026-07-26T00:00:02Z".to_string(),
                        status: "completed".to_string(),
                        content: serde_json::json!([{
                            "type": "tool_result",
                            "id": "call-1",
                            "name": "read",
                            "output": [{
                                "type": "text",
                                "text": format!(
                                    "backfill-marker{}",
                                    "x".repeat(70_000)
                                )
                            }]
                        }]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                    rua_session::NewConversationMessage {
                        message_id: "assistant-final".to_string(),
                        role: "assistant".to_string(),
                        created_at: "2026-07-26T00:00:03Z".to_string(),
                        status: "completed".to_string(),
                        content: serde_json::json!([
                            {"type": "text", "text": "Old history inspected."}
                        ]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                ],
            },
        )
        .await
        .unwrap();

    state
        .conversation_input_intake
        .update_authority_chat_running_config(
            &rua_core::ChatAddress::agent("default", "chat-backfill"),
            "default",
            &runtime.workspace_dir(),
            serde_json::to_value(rua_config::ChatRunningConfig::from_agent_defaults(
                &running(),
            ))
            .unwrap(),
        )
        .await
        .unwrap();
    let config = state
        .product_hooks
        .chat_configuration(&rua_core::ChatAddress::agent("default", "chat-backfill"))
        .await
        .unwrap();
    assert!(
        config
            .running_config
            .resolve()
            .unwrap()
            .context_compact
            .history_replacement_enabled
    );
    let current = store
        .load_chat_header("chat-backfill")
        .await
        .unwrap()
        .unwrap();
    assert!(
        rua_chat::model_slot_from_header(&current).is_some(),
        "backfill needs a model"
    );
    let active_manager: Arc<dyn ContextManager> = manager.clone();
    assert!(
        Arc::ptr_eq(&runtime.context_manager().await.unwrap(), &active_manager),
        "fixture manager replaced during initialization"
    );
    rua_server::services::history_replacement::reconcile_agent_config(&state, "default").await;
    let source = tokio::time::timeout(std::time::Duration::from_secs(2), source_rx.recv())
        .await
        .unwrap_or_else(|error| {
            panic!("backfill generation: {error}; verify initialized Chat configuration")
        })
        .expect("source");
    assert!(source.contains("backfill-marker"));
    wait_for_ready_records(&store, "chat-backfill", 1).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_authority_reuses_preexisting_record_without_regeneration() {
    let dir = tempfile::tempdir().unwrap();
    let chat_id = CHAT_TRANSFER;
    let messages = one_block_messages("authority-transfer-marker");
    materialize_chat(dir.path(), "agent-1", chat_id, "Authority transfer").await;
    WorkspaceSessionPersistence::new(dir.path().to_path_buf())
        .save_chat_entry("agent-1", chat_id, "Authority transfer")
        .await
        .unwrap();
    WorkspaceSessionPersistence::new(dir.path().to_path_buf())
        .save_messages("agent-1", chat_id, &messages)
        .await
        .unwrap();

    let store = rua_session::WorkspaceSessionStore::new(dir.path());
    let block = store
        .detect_history_replacement_blocks(chat_id, 4.0)
        .await
        .unwrap()
        .remove(0);
    store
        .publish_ready_history_replacement(
            &rua_core::ChatAddress::agent("default", chat_id),
            rua_session::HistoryReplacementReadyDraft {
                record_id: block.record_id,
                source: block.source,
                digest: "Record synchronized before Authority transfer.".to_string(),
                replacement_token_estimate: 32,
                model: model(),
                generated_at: "2026-07-26T00:01:00Z".to_string(),
            },
        )
        .await
        .unwrap();

    let authority = Arc::new(ConversationAuthority::new());
    let (source_tx, mut source_rx) = mpsc::unbounded_channel();
    let manager = Arc::new(OrderedManager {
        calls: AtomicUsize::new(0),
        source_tx,
        permits: Arc::new(Semaphore::new(1)),
    });
    build_persistence(
        dir.path(),
        chat_id,
        messages.len(),
        manager.clone(),
        authority,
    )
    .save_messages("agent-1", chat_id, &messages)
    .await
    .unwrap();

    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(200), source_rx.recv())
            .await
            .is_err(),
        "synchronized ready record must suppress generation on the new Authority"
    );
    assert_eq!(manager.calls.load(Ordering::SeqCst), 0);
}
