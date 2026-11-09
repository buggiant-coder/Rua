use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use agent_types::{ContentPart, ModelDialogueItem, Role, ToolCall, ToolResult};
use async_trait::async_trait;
use react_agent::ModelCallProjector;
use rua_agent::context_manager::{
    CompactContextRequest, CompactContextResult, ContextManager, ContextSummaryRequest,
    ContextSummaryResult,
};
use rua_agent::error::AgentError;
use rua_agent::summary_checkpoint::{
    fit_summary_projection_slice_to_budget, prepare_summary_projection_for_budget,
    summary_projection_slice, summary_request_token_estimate, SummaryCheckpointPublisher,
    SummaryCheckpointPublisherInput, SummaryProjectionSlice,
};
use rua_contract::agent::AgentRequest;
use rua_session::{
    ChatMetadata, ChatSessionHeader, CheckpointModelIdentity, CheckpointSequenceRange,
    CheckpointTurnRange, ContextProjection, ContextProjectionRequest, HistoryReplacementReadyDraft,
    MessageLogEntry, NewConversationMessage, NewConversationTurn, ProjectionFoldFrontier,
    ProjectionPlanner, SummaryCheckpointDraft, TurnLifecycle, WorkspaceSessionStore,
};
use serde_json::{json, Value};

struct CapturingContextManager {
    content: Arc<Mutex<Option<String>>>,
}

struct OverflowOnceContextManager {
    content_sizes: Arc<Mutex<Vec<usize>>>,
}

#[async_trait]
impl ContextManager for CapturingContextManager {
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
        req: ContextSummaryRequest,
    ) -> Result<ContextSummaryResult, AgentError> {
        assert_eq!(req.max_tokens, Some(4_096));
        *self.content.lock().unwrap() = Some(format!(
            "{}\n{}",
            req.previous_summary.unwrap_or_default(),
            serde_json::to_string(&req.messages).unwrap()
        ));
        Ok(ContextSummaryResult {
            summary: "checkpoint summary".to_string(),
        })
    }
}

#[async_trait]
impl ContextManager for OverflowOnceContextManager {
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
        req: ContextSummaryRequest,
    ) -> Result<ContextSummaryResult, AgentError> {
        let mut content_sizes = self.content_sizes.lock().unwrap();
        content_sizes.push(serde_json::to_vec(&req.messages).unwrap().len());
        if content_sizes.len() == 1 {
            return Err(AgentError::provider("HTTP 400: context_length_exceeded"));
        }
        Ok(ContextSummaryResult {
            summary: "checkpoint summary".to_string(),
        })
    }
}

fn header(chat_id: &str) -> ChatSessionHeader {
    ChatSessionHeader::new(ChatMetadata {
        chat_id: chat_id.to_string(),
        agent_id: "agent-1".to_string(),
        title: "Checkpoint replacement".to_string(),
        created_at: "2026-07-26T00:00:00Z".to_string(),
        last_activity_at: "2026-07-26T00:00:00Z".to_string(),
        pinned: false,
        archived: false,
        preview: None,
    })
}

fn message(message_id: &str, role: &str, content: Value) -> NewConversationMessage {
    NewConversationMessage {
        message_id: message_id.to_string(),
        role: role.to_string(),
        created_at: "2026-07-26T00:00:00Z".to_string(),
        status: "completed".to_string(),
        content,
        provider: None,
        tool: None,
        metadata: serde_json::Map::new(),
    }
}

fn log_entry(sequence: u64, message_id: &str, content: &str) -> MessageLogEntry {
    MessageLogEntry {
        message_id: message_id.to_string(),
        sequence,
        turn_id: "turn-1".to_string(),
        role: "user".to_string(),
        created_at: "2026-07-26T00:00:00Z".to_string(),
        status: "completed".to_string(),
        content: json!([{ "type": "text", "text": content }]),
        provider: None,
        tool: None,
        metadata: serde_json::Map::new(),
    }
}

#[test]
fn summary_slice_projects_only_messages_before_the_frontier() {
    let projection = ContextProjection {
        chat_id: "chat-1".to_string(),
        agent_id: "agent-1".to_string(),
        active_checkpoint_id: Some("checkpoint-1".to_string()),
        summary: Some("previous checkpoint".to_string()),
        summary_model: None,
        tail_start_sequence: Some(1),
        tail_end_sequence: Some(3),
        retained_messages: Vec::new(),
        tail_messages: vec![
            log_entry(1, "message-1", "raw one"),
            log_entry(2, "message-1", "raw duplicate record"),
            log_entry(3, "message-2", "raw tail"),
        ],
        current_user_input: None,
        stable_token_estimate: 10,
        history_replacement_evidence: Vec::new(),
    };
    let slice =
        summary_projection_slice(&projection, &rua_config::AgentsRunningConfig::default(), 2)
            .unwrap();

    assert_eq!(
        slice.previous_summary.as_deref(),
        Some("previous checkpoint")
    );
    assert_eq!(slice.messages.len(), 2);
    assert_eq!(slice.messages[0].id.as_deref(), Some("message-1"));
    assert_eq!(slice.messages[1].id.as_deref(), Some("message-1"));
    assert!(slice
        .messages
        .iter()
        .all(|message| message.id.as_deref() != Some("message-2")));
}

fn tool_round(call_id: &str, tool_name: &str, output: &str) -> Vec<ModelDialogueItem> {
    vec![
        ModelDialogueItem {
            id: Some(format!("{call_id}-use")),
            role: Role::Assistant,
            content: vec![ContentPart::ToolUse {
                call: ToolCall::new(
                    call_id,
                    tool_name,
                    json!({"path": format!("{call_id}.txt")}),
                ),
            }],
            name: None,
            created_at: None,
            metadata: serde_json::Map::new(),
        },
        ModelDialogueItem {
            id: Some(format!("{call_id}-result")),
            role: Role::Tool,
            content: vec![ContentPart::ToolResult {
                result: ToolResult::text(call_id, output),
            }],
            name: Some(tool_name.to_string()),
            created_at: None,
            metadata: serde_json::Map::new(),
        },
    ]
}

#[test]
fn summary_input_fitting_drops_thinking_before_tool_results() {
    let mut running = rua_config::AgentsRunningConfig::default();
    running.context_compact.compact_with_thinking_block = true;
    running.context_compact.thinking_turns_to_keep = -1;
    running.context_compact.token_count_estimate_divisor = 4.0;

    let mut messages = vec![ModelDialogueItem {
        id: Some("assistant-thinking".to_string()),
        role: Role::Assistant,
        content: vec![
            ContentPart::Thinking {
                text: "private-thinking-marker".repeat(1_000),
                signature: None,
            },
            ContentPart::Text {
                text: "visible-assistant-marker".to_string(),
            },
        ],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    }];
    messages.extend(tool_round("call-kept", "read", "tool-output-must-remain"));
    let source = SummaryProjectionSlice {
        previous_summary: None,
        messages,
    };

    let fitted = fit_summary_projection_slice_to_budget(source, &running, 600)
        .unwrap()
        .unwrap();
    let rendered = serde_json::to_string(&fitted.messages).unwrap();

    assert!(!rendered.contains("private-thinking-marker"));
    assert!(rendered.contains("visible-assistant-marker"));
    assert!(rendered.contains("tool-output-must-remain"));
    assert!(!rendered.contains("recall_tool_result"));
}

#[test]
fn summary_input_fitting_compacts_oldest_tool_results_without_moving_frontier() {
    let mut running = rua_config::AgentsRunningConfig::default();
    running.context_compact.compact_with_thinking_block = false;
    running.context_compact.token_count_estimate_divisor = 4.0;

    let mut messages = tool_round("call-old", "read", &"old-output-".repeat(1_000));
    messages.extend(tool_round(
        "call-new",
        "read",
        "new-tool-output-must-remain",
    ));
    messages.push(ModelDialogueItem::text(
        Role::Assistant,
        "requested-frontier-end-marker",
    ));
    let source = SummaryProjectionSlice {
        previous_summary: None,
        messages,
    };

    let fitted = fit_summary_projection_slice_to_budget(source, &running, 800)
        .unwrap()
        .unwrap();
    let rendered = serde_json::to_string(&fitted.messages).unwrap();

    assert!(rendered.contains("requested-frontier-end-marker"));
    assert!(rendered.contains("call-old"));
    assert!(rendered.contains("recall_tool_result"));
    assert!(rendered.contains(&rua_tool::result_store::recall_key("call-old")));
    assert!(!rendered.contains("call-old.txt"));
    assert!(!rendered.contains(&"old-output-".repeat(100)));
    assert!(rendered.contains("call-new.txt"));
    assert!(rendered.contains("new-tool-output-must-remain"));
}

#[test]
fn summary_input_fitting_reports_when_fixed_prefix_cannot_fit() {
    let mut running = rua_config::AgentsRunningConfig::default();
    running.context_compact.compact_with_thinking_block = false;
    running.context_compact.token_count_estimate_divisor = 4.0;
    let source = SummaryProjectionSlice {
        previous_summary: None,
        messages: vec![ModelDialogueItem::text(
            Role::User,
            "non-tool-content-cannot-be-dropped".repeat(1_000),
        )],
    };

    let fitted = fit_summary_projection_slice_to_budget(source, &running, 400).unwrap();

    assert!(fitted.is_none());
}

#[test]
fn summary_input_fitting_moves_frontier_only_after_fixed_prefix_cannot_fit() {
    let running = rua_config::AgentsRunningConfig::default();
    let projection = ContextProjection {
        chat_id: "chat-1".to_string(),
        agent_id: "agent-1".to_string(),
        active_checkpoint_id: None,
        summary: None,
        summary_model: None,
        tail_start_sequence: Some(1),
        tail_end_sequence: Some(2),
        retained_messages: Vec::new(),
        tail_messages: vec![
            log_entry(1, "message-1", "small-prefix"),
            log_entry(2, "message-2", &"oversized-visible-text".repeat(1_000)),
        ],
        current_user_input: None,
        stable_token_estimate: 10_000,
        history_replacement_evidence: Vec::new(),
    };
    let groups = vec![
        rua_session::ProjectionMessageGroupEstimate {
            turn_id: "turn-1".to_string(),
            turn_ordinal: 1,
            first_sequence: 1,
            last_sequence: 1,
            token_estimate: 10,
            foldable: true,
            retained_sequence_ranges: Vec::new(),
        },
        rua_session::ProjectionMessageGroupEstimate {
            turn_id: "turn-2".to_string(),
            turn_ordinal: 2,
            first_sequence: 2,
            last_sequence: 2,
            token_estimate: 10_000,
            foldable: true,
            retained_sequence_ranges: Vec::new(),
        },
    ];
    let requested = ProjectionFoldFrontier {
        turn_id: "turn-2".to_string(),
        start_turn_ordinal: 1,
        end_turn_ordinal: 2,
        start_sequence: 1,
        end_sequence: 2,
        estimated_tokens_to_fold: 10_010,
        retained_sequence_ranges: Vec::new(),
    };

    let (selected, source) =
        prepare_summary_projection_for_budget(&projection, &groups, requested, &running, 600)
            .unwrap()
            .unwrap();
    let rendered = serde_json::to_string(&source.messages).unwrap();

    assert_eq!(selected.end_sequence, 1);
    assert!(rendered.contains("small-prefix"));
    assert!(!rendered.contains("oversized-visible-text"));
}

#[tokio::test]
async fn checkpoint_planner_keeps_split_assistant_response_with_its_tool_round() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    let chat_id = "split-assistant-checkpoint";
    store.save_chat_header(&header(chat_id)).await.unwrap();
    let mut text = message(
        "assistant-text",
        "assistant",
        json!([{"type": "text", "text": "response context ".repeat(1_000)}]),
    );
    let mut tool_use = message(
        "assistant-tool",
        "assistant",
        json!([{"type": "tool_use", "id": "call-1", "name": "read", "input": {}}]),
    );
    for part in [&mut text, &mut tool_use] {
        part.metadata.insert(
            agent_types::conversation_message_identity::MODEL_DIALOGUE_ITEM_ID_METADATA_KEY
                .to_string(),
            json!("model-call-1"),
        );
    }
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-09-20T07:05:07Z".to_string(),
                    assistant_started_at: Some("2026-09-20T07:05:08Z".to_string()),
                    assistant_completed_at: Some("2026-09-20T07:05:09Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("variant-1".to_string()),
                messages: vec![
                    message(
                        "user-1",
                        "user",
                        json!([{"type": "text", "text": "Continue"}]),
                    ),
                    text,
                    tool_use,
                    message(
                        "tool-result",
                        "tool",
                        json!([{"type": "tool_result", "id": "call-1", "name": "read",
                            "output": [{"type": "text", "text": "file contents"}]}]),
                    ),
                    message(
                        "assistant-final",
                        "assistant",
                        json!([{"type": "text", "text": "Done"}]),
                    ),
                ],
            },
        )
        .await
        .unwrap();
    let address = rua_core::ChatAddress::agent("agent-1", chat_id);
    let mut running = rua_config::AgentsRunningConfig::default();
    running.tool_result_compact.projection_truncation_enabled = false;
    let projection = store
        .build_effective_context_projection_for_address(
            &address,
            ContextProjectionRequest {
                current_user_input: None,
            },
            false,
            4.0,
        )
        .await
        .unwrap();
    let mut groups = store
        .projection_message_group_estimates_effective_for_address(&address, false, 4.0)
        .await
        .unwrap();
    rua_agent::projection_preparation::finalize_projection_message_group_estimates(
        &projection,
        &mut groups,
        &running,
    );
    let planner = ProjectionPlanner::new(rua_session::ContextTargetBand {
        upper_target_tokens: 2_000,
        emergency_limit_tokens: 5_000,
        tail_reserve_tokens: 500,
    });
    let plan = planner.plan(rua_session::ProjectionPlanningInput {
        entry_mode: rua_session::ProjectionEntryMode::PreModelCall,
        current_projection_tokens: 10_000,
        checkpoint_tokens: 0,
        system_context_tokens: 0,
        pending_user_input_tokens: 0,
        message_groups: groups.clone(),
    });
    let (frontier, source) = prepare_summary_projection_for_budget(
        &projection,
        &groups,
        plan.fold_frontier.unwrap(),
        &running,
        10_000,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        frontier.end_sequence, 4,
        "a response and all of its tool results are one safe group"
    );
    assert_eq!(source.messages.len(), 3);
    assert!(matches!(
        source.messages[1].content.last(),
        Some(ContentPart::ToolUse { .. })
    ));
    assert!(matches!(
        source.messages[2].content.first(),
        Some(ContentPart::ToolResult { .. })
    ));

    let captured = Arc::new(Mutex::new(None));
    let publisher = SummaryCheckpointPublisher::new(SummaryCheckpointPublisherInput {
        store: store.clone(),
        chat_id: chat_id.to_string(),
        chat_address: address,
        agent_id: "agent-1".to_string(),
        context_manager: Some(Arc::new(CapturingContextManager {
            content: Arc::clone(&captured),
        })),
        running_config: Arc::new(running),
        model: CheckpointModelIdentity {
            provider: "provider-a".to_string(),
            model: "model-a".to_string(),
        },
        workspace_dir: dir.path().to_path_buf(),
        replication_hook: None,
        model_call_identity: None,
    });
    publisher
        .publish(&frontier, source, || true)
        .await
        .unwrap()
        .unwrap();
    let rebuilt = store
        .build_context_projection(
            chat_id,
            ContextProjectionRequest {
                current_user_input: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(rebuilt.summary.as_deref(), Some("checkpoint summary"));
    assert_eq!(
        rebuilt
            .tail_messages
            .iter()
            .map(|m| m.message_id.as_str())
            .collect::<Vec<_>>(),
        vec!["assistant-final"]
    );
    assert!(captured
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .contains("file contents"));
    assert_eq!(
        store
            .read_turn_messages(chat_id, "turn-1")
            .await
            .unwrap()
            .len(),
        5
    );
}

#[test]
fn summary_input_budget_measures_the_whole_request_without_a_conversation_offset() {
    let running = rua_config::AgentsRunningConfig::default();
    let source = SummaryProjectionSlice {
        previous_summary: None,
        messages: vec![ModelDialogueItem::text(Role::User, "x".repeat(1_200))],
    };
    let measured = summary_request_token_estimate(&source, &running);
    assert!(
        measured > 400,
        "the summary instruction must be included in the budget"
    );
    assert!(
        fit_summary_projection_slice_to_budget(source, &running, 400)
            .unwrap()
            .is_none(),
        "a different conversation request cannot discount this summary input"
    );
}

#[tokio::test]
async fn checkpoint_projection_recovers_from_summary_provider_overflow() {
    assert_checkpoint_projection_recovers_from_summary_provider_overflow(false).await;
}

#[tokio::test]
async fn checkpoint_projection_publishes_and_rebuilds_with_excluded_history() {
    assert_checkpoint_projection_recovers_from_summary_provider_overflow(true).await;
}

async fn assert_checkpoint_projection_recovers_from_summary_provider_overflow(exclude: bool) {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    let chat_id = "summary-overflow-retry";
    prepare_chat(&store, chat_id).await;
    if exclude {
        store
            .save_projection_exclusions(chat_id, vec!["assistant-final".into()])
            .await
            .unwrap();
    }
    let content_sizes = Arc::new(Mutex::new(Vec::new()));
    let mut running = rua_config::AgentsRunningConfig {
        max_input_length: 4_000,
        ..Default::default()
    };
    running.context_compact.blocking_compact_ratio = 0.9;
    running.context_compact.context_reserve_ratio = 0.1;
    running.context_compact.history_replacement_enabled = false;
    running.tool_result_compact.projection_truncation_enabled = false;
    let request = AgentRequest {
        session_id: chat_id.into(),
        user_id: "user-1".to_string(),
        agent_id: "agent-1".into(),
        messages: Vec::new(),
        model_override: None,
        tool_mode: "auto".to_string(),
        attachments: Vec::new(),
        display_preferences: None,
        disabled_tools: Vec::new(),
        metadata: HashMap::from([
            ("turn_id".to_string(), json!("turn-1")),
            ("loaded_message_count".to_string(), json!(0)),
        ]),
        cancel: None,
        run_control: None,
        workspace_dir: Some(dir.path().to_path_buf()),
        conversation_store_dir: None,
        working_dir: None,
    };
    let projector = rua_agent::projection_preparation::ContextProjectionProjector::from_request(
        &request,
        Some(Arc::new(OverflowOnceContextManager {
            content_sizes: Arc::clone(&content_sizes),
        })),
        Arc::new(running),
        CheckpointModelIdentity {
            provider: "provider-a".to_string(),
            model: "model-a".to_string(),
        },
        Some(4_000),
        None,
    )
    .unwrap();
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);

    let projected = projector
        .project(agent_types::ChatRequest::streaming(Vec::new()), &event_tx)
        .await
        .unwrap();
    assert!(projected
        .messages
        .iter()
        .any(|message| message.text_content().contains("checkpoint summary")));
    if exclude {
        assert!(projected
            .messages
            .iter()
            .all(|message| !message.text_content().contains("Inspection completed.")));
    }

    let content_sizes = content_sizes.lock().unwrap().clone();
    assert_eq!(content_sizes.len(), 2);
    assert!(content_sizes[1] <= content_sizes[0]);
    assert!(store
        .load_active_summary_checkpoint(chat_id)
        .await
        .unwrap()
        .is_some());
}

async fn prepare_chat(
    store: &WorkspaceSessionStore,
    chat_id: &str,
) -> rua_session::ExecutionTraceBlock {
    store.save_chat_header(&header(chat_id)).await.unwrap();
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-07-26T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-07-26T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-07-26T00:00:03Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("variant-1".to_string()),
                messages: vec![
                    message(
                        "user-1",
                        "user",
                        json!([{"type": "text", "text": "Inspect source"}]),
                    ),
                    message(
                        "assistant-process",
                        "assistant",
                        json!([
                            {"type": "thinking", "thinking": "private-thinking-marker"},
                            {
                                "type": "tool_use",
                                "id": "call-1",
                                "name": "read",
                                "input": {"path": "src/lib.rs"}
                            }
                        ]),
                    ),
                    message(
                        "tool-result",
                        "tool",
                        json!([{
                            "type": "tool_result",
                            "id": "call-1",
                            "name": "read",
                            "output": [{
                                "type": "text",
                                "text": format!(
                                    "raw-checkpoint-marker{}",
                                    "x".repeat(70_000)
                                )
                            }]
                        }]),
                    ),
                    message(
                        "assistant-final",
                        "assistant",
                        json!([{
                            "type": "text",
                            "text": "Inspection completed."
                        }]),
                    ),
                ],
            },
        )
        .await
        .unwrap();
    store
        .detect_history_replacement_blocks(chat_id, 4.0)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap()
}

async fn publish_checkpoint(
    workspace: &std::path::Path,
    chat_id: &str,
    history_replacement_enabled: bool,
    configure: impl FnOnce(&mut rua_config::AgentsRunningConfig),
) -> (String, rua_session::ExecutionTraceBlock) {
    publish_checkpoint_for_address(
        workspace,
        rua_core::ChatAddress::agent("agent-1", chat_id),
        history_replacement_enabled,
        configure,
    )
    .await
}

async fn publish_checkpoint_for_address(
    workspace: &std::path::Path,
    chat_address: rua_core::ChatAddress,
    history_replacement_enabled: bool,
    configure: impl FnOnce(&mut rua_config::AgentsRunningConfig),
) -> (String, rua_session::ExecutionTraceBlock) {
    let chat_id = chat_address.chat_id().to_string();
    let store = WorkspaceSessionStore::new(workspace);
    let block = prepare_chat(&store, &chat_id).await;
    store
        .publish_ready_history_replacement(
            &chat_address,
            HistoryReplacementReadyDraft {
                record_id: block.record_id.clone(),
                source: block.source.clone(),
                digest: "ready-digest-marker".to_string(),
                replacement_token_estimate: 32,
                model: CheckpointModelIdentity {
                    provider: "provider-a".to_string(),
                    model: "model-a".to_string(),
                },
                generated_at: "2026-07-26T00:01:00Z".to_string(),
            },
        )
        .await
        .unwrap();
    let captured = Arc::new(Mutex::new(None));
    let context_manager = Arc::new(CapturingContextManager {
        content: Arc::clone(&captured),
    });
    let mut running = rua_config::AgentsRunningConfig::default();
    running.context_compact.history_replacement_enabled = history_replacement_enabled;
    running.context_compact.compact_with_thinking_block = false;
    configure(&mut running);
    let groups = store
        .projection_message_group_estimates_effective_for_address(
            &chat_address,
            history_replacement_enabled,
            4.0,
        )
        .await
        .unwrap();
    let frontier = ProjectionPlanner::widest_fold_frontier(&groups).unwrap();
    let summary_source = rua_agent::summary_checkpoint::build_summary_projection_slice(
        &store,
        &chat_address,
        &running,
        frontier.end_sequence,
    )
    .await
    .unwrap();
    let publisher = SummaryCheckpointPublisher::new(SummaryCheckpointPublisherInput {
        store,
        chat_id,
        chat_address: chat_address.clone(),
        agent_id: "agent-1".to_string(),
        context_manager: Some(context_manager),
        running_config: Arc::new(running),
        model: CheckpointModelIdentity {
            provider: "provider-a".to_string(),
            model: "model-a".to_string(),
        },
        workspace_dir: workspace.to_path_buf(),
        replication_hook: None,
        model_call_identity: Some(
            rua_agent::model_call_context::ModelCallIdentityContext::from_chat_address(
                &chat_address,
            ),
        ),
    });
    publisher
        .publish(&frontier, summary_source, || true)
        .await
        .unwrap()
        .unwrap();
    let content = captured.lock().unwrap().clone().unwrap();
    (content, block)
}

#[tokio::test]
async fn descendant_checkpoint_summary_includes_the_previous_retained_message() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&header("chat-retained-chain"))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            "chat-retained-chain",
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-07-26T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-07-26T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-07-26T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("assistant-1".to_string()),
                messages: vec![
                    message(
                        "user-1",
                        "user",
                        json!([{"type": "text", "text": "retained-user-marker"}]),
                    ),
                    message(
                        "assistant-1",
                        "assistant",
                        json!([{"type": "text", "text": "first-response-marker"}]),
                    ),
                ],
            },
        )
        .await
        .unwrap();
    let previous_state = store
        .checkpoint_projection_state("chat-retained-chain", 1)
        .await
        .unwrap();
    store
        .publish_summary_checkpoint(
            "chat-retained-chain",
            None,
            SummaryCheckpointDraft {
                checkpoint_id: "checkpoint-1".to_string(),
                previous_checkpoint_id: None,
                covered_turn_range: CheckpointTurnRange {
                    start_turn_ordinal: 1,
                    end_turn_ordinal: 1,
                },
                covered_sequence_range: CheckpointSequenceRange {
                    start_sequence: 1,
                    end_sequence: 1,
                },
                source_sequence_range: CheckpointSequenceRange {
                    start_sequence: 1,
                    end_sequence: 1,
                },
                frontier: previous_state.frontier,
                retained_sequence_ranges: previous_state.retained_sequence_ranges,
                projection_lineage: previous_state.projection_lineage,
                summary: "previous-summary-marker".to_string(),
                token_estimate: 8,
                model: CheckpointModelIdentity {
                    provider: "provider-a".to_string(),
                    model: "model-a".to_string(),
                },
                created_at: "2026-07-26T00:01:00Z".to_string(),
            },
        )
        .await
        .unwrap();
    store
        .append_conversation_turn(
            "chat-retained-chain",
            NewConversationTurn {
                turn_id: "turn-2".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-07-26T00:02:00Z".to_string(),
                    assistant_started_at: Some("2026-07-26T00:02:01Z".to_string()),
                    assistant_completed_at: Some("2026-07-26T00:02:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("assistant-2".to_string()),
                messages: vec![
                    message(
                        "user-2",
                        "user",
                        json!([{"type": "text", "text": "new-user-marker"}]),
                    ),
                    message(
                        "assistant-2",
                        "assistant",
                        json!([{"type": "text", "text": "new-response-marker"}]),
                    ),
                ],
            },
        )
        .await
        .unwrap();

    let captured = Arc::new(Mutex::new(None));
    let chat_address = rua_core::ChatAddress::agent("agent-1", "chat-retained-chain");
    let running_config = Arc::new(rua_config::AgentsRunningConfig::default());
    let frontier = ProjectionFoldFrontier {
        turn_id: "turn-2".to_string(),
        start_turn_ordinal: 1,
        end_turn_ordinal: 2,
        start_sequence: 2,
        end_sequence: 4,
        estimated_tokens_to_fold: 32,
        retained_sequence_ranges: Vec::new(),
    };
    let summary_source = rua_agent::summary_checkpoint::build_summary_projection_slice(
        &store,
        &chat_address,
        &running_config,
        frontier.end_sequence,
    )
    .await
    .unwrap();
    let publisher = SummaryCheckpointPublisher::new(SummaryCheckpointPublisherInput {
        store,
        chat_id: "chat-retained-chain".to_string(),
        chat_address,
        agent_id: "agent-1".to_string(),
        context_manager: Some(Arc::new(CapturingContextManager {
            content: Arc::clone(&captured),
        })),
        running_config,
        model: CheckpointModelIdentity {
            provider: "provider-a".to_string(),
            model: "model-a".to_string(),
        },
        workspace_dir: dir.path().to_path_buf(),
        replication_hook: None,
        model_call_identity: Some(
            rua_agent::model_call_context::ModelCallIdentityContext::from_chat_address(
                &rua_core::ChatAddress::agent("agent-1", "chat-1"),
            ),
        ),
    });
    publisher
        .publish(&frontier, summary_source, || true)
        .await
        .unwrap()
        .unwrap();

    let content = captured.lock().unwrap().clone().unwrap();
    assert!(content.contains("previous-summary-marker"));
    assert!(
        content.contains("retained-user-marker"),
        "the previous checkpoint's retained raw message must enter the next summary"
    );
    assert!(content.contains("first-response-marker"));
    assert!(content.contains("new-user-marker"));
    assert!(content.contains("new-response-marker"));
}

#[tokio::test]
async fn project_checkpoint_summary_consumes_ready_history_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let (content, _) = publish_checkpoint_for_address(
        dir.path(),
        rua_core::ChatAddress::project("project-1", "chat-project"),
        true,
        |_| {},
    )
    .await;

    assert!(content.contains("ready-digest-marker"));
    assert!(!content.contains("raw-checkpoint-marker"));
    assert!(!content.contains("private-thinking-marker"));
}

#[tokio::test]
async fn checkpoint_summary_consumes_ready_digest_and_record_reactivates_after_invalidation() {
    let dir = tempfile::tempdir().unwrap();
    let (content, block) = publish_checkpoint(dir.path(), "chat-enabled", true, |_| {}).await;
    assert!(content.contains("ready-digest-marker"));
    assert!(!content.contains("raw-checkpoint-marker"));
    assert!(!content.contains("private-thinking-marker"));

    let store = WorkspaceSessionStore::new(dir.path());
    let covered = store
        .build_effective_context_projection(
            "chat-enabled",
            ContextProjectionRequest {
                current_user_input: None,
            },
            true,
            4.0,
        )
        .await
        .unwrap();
    assert!(!format!("{:?}", covered.tail_messages).contains("ready-digest-marker"));
    assert!(store
        .load_history_replacement("chat-enabled", &block.record_id)
        .await
        .unwrap()
        .is_some());

    store
        .clear_active_summary_checkpoint("chat-enabled")
        .await
        .unwrap();
    let reactivated = store
        .build_effective_context_projection(
            "chat-enabled",
            ContextProjectionRequest {
                current_user_input: None,
            },
            true,
            4.0,
        )
        .await
        .unwrap();
    assert!(format!("{:?}", reactivated.tail_messages).contains("ready-digest-marker"));
}

#[tokio::test]
async fn disabled_history_replacement_keeps_raw_checkpoint_source_and_thinking_policy() {
    let dir = tempfile::tempdir().unwrap();
    let (content, _) = publish_checkpoint(dir.path(), "chat-disabled", false, |_| {}).await;
    assert!(content.contains("raw-checkpoint-marker"));
    assert!(!content.contains("ready-digest-marker"));
    assert!(!content.contains("private-thinking-marker"));
}

#[tokio::test]
async fn checkpoint_summary_uses_projection_tool_result_truncation_without_mutating_raw_log() {
    let dir = tempfile::tempdir().unwrap();
    let (content, _) = publish_checkpoint(dir.path(), "chat-tool-truncation", false, |running| {
        running.tool_result_compact.projection_truncation_enabled = true;
        running.tool_result_compact.recent_n = 0;
        running.tool_result_compact.old_max_bytes = 64;
        running.tool_result_compact.execution_output_max_tokens = 100_000;
    })
    .await;

    assert!(content.contains("raw-checkpoint-marker"));
    assert!(!content.contains(&"x".repeat(512)));

    let store = WorkspaceSessionStore::new(dir.path());
    let raw_messages = store
        .read_turn_messages("chat-tool-truncation", "turn-1")
        .await
        .unwrap();
    assert!(
        raw_messages
            .iter()
            .any(|message| message.content.to_string().contains(&"x".repeat(512))),
        "projection truncation must not rewrite the durable Message Log"
    );
}

#[tokio::test]
async fn context_projection_materializes_historical_media_absent_from_runtime_messages() {
    let workspace = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(project.path());
    let chat_id = "historical-media";
    store.save_chat_header(&header(chat_id)).await.unwrap();
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: "turn-image".into(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-09-21T00:00:00Z".into(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".into(),
                },
                selected_variant_id: None,
                messages: vec![message(
                    "user-image",
                    "user",
                    json!([
                        {"type":"text","text":"inspect the earlier image"},
                        {"type":"image","media_ref":"media/historical-image.png"},
                        {"type":"image","url":"https://example.com/external.png"}
                    ]),
                )],
            },
        )
        .await
        .unwrap();
    let mut running = rua_config::AgentsRunningConfig::default();
    running.context_compact.context_compact_enabled = false;
    let request = AgentRequest {
        session_id: chat_id.into(),
        user_id: "user".into(),
        agent_id: "agent-1".into(),
        messages: Vec::new(),
        model_override: None,
        tool_mode: "auto".into(),
        attachments: Vec::new(),
        display_preferences: None,
        disabled_tools: Vec::new(),
        metadata: HashMap::from([
            ("turn_id".into(), json!("turn-image")),
            ("loaded_message_count".into(), json!(0)),
        ]),
        cancel: None,
        run_control: None,
        workspace_dir: Some(workspace.path().to_path_buf()),
        conversation_store_dir: Some(project.path().to_path_buf()),
        working_dir: None,
    };
    let projector = rua_agent::projection_preparation::ContextProjectionProjector::from_request(
        &request,
        None,
        Arc::new(running),
        CheckpointModelIdentity {
            provider: "fixture".into(),
            model: "fixture".into(),
        },
        Some(1_000_000),
        None,
    )
    .unwrap();
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let projected = projector
        .project(agent_types::ChatRequest::streaming(Vec::new()), &tx)
        .await
        .unwrap();
    let urls = projected
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|p| match p {
            ContentPart::Image { url, .. } => Some(url.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let expected = project.path().join("media/historical-image.png");
    assert_eq!(
        urls,
        vec![
            expected.to_str().unwrap(),
            "https://example.com/external.png"
        ]
    );
    assert!(store.read_message_range(chat_id, 1, 1).await.unwrap()[0]
        .content
        .to_string()
        .contains("media/historical-image.png"));
}
