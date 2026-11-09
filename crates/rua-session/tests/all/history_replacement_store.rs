use rua_core::chat::ChatAddress;
use rua_session::{
    derive_history_replacement_record_id, history_replacement_locator, ChatMetadata,
    ChatSessionHeader, CheckpointModelIdentity, CheckpointSequenceRange, ContextProjectionRequest,
    ConversationReadSpanRequest, HistoryReplacementContentAnchor, HistoryReplacementReadyDraft,
    HistoryReplacementRecordState, HistoryReplacementRejectedDraft,
    HistoryReplacementRejectionReason, HistoryReplacementSource, HistoryReplacementToolReference,
    NewConversationMessage, NewConversationTurn, TurnLifecycle, WorkspaceSessionStore,
};
use serde_json::{json, Value};

fn header(chat_id: &str) -> ChatSessionHeader {
    ChatSessionHeader::new(ChatMetadata {
        chat_id: chat_id.to_string(),
        agent_id: "agent-1".to_string(),
        title: "History replacement".to_string(),
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

#[test]
fn history_replacement_record_id_is_independent_of_json_map_order_features() {
    let source = HistoryReplacementSource {
        turn_id: "turn-1".to_string(),
        message_ids: vec!["a".to_string(), "b".to_string()],
        start_anchor: HistoryReplacementContentAnchor {
            message_id: "a".to_string(),
            content_part_index: 0,
        },
        end_anchor: HistoryReplacementContentAnchor {
            message_id: "b".to_string(),
            content_part_index: 1,
        },
        sequence_range: CheckpointSequenceRange {
            start_sequence: 2,
            end_sequence: 3,
        },
        tool_references: vec![HistoryReplacementToolReference {
            tool_call_id: "call-1".to_string(),
            tool_name: "read".to_string(),
            call_sequence: 2,
            result_sequence: 3,
            recall_key: Some("recall-1".to_string()),
        }],
        projection_lineage: "ignored-by-identity".to_string(),
        token_estimate: 42,
    };

    assert_eq!(
        derive_history_replacement_record_id("chat-1", &source),
        "hr-d87b42404a7ff516cbfa5e6431848f4a2db893d5ead410d50434ca1a87ba78fc"
    );
}

async fn append_trace_turn(store: &WorkspaceSessionStore, chat_id: &str, tool_output: String) {
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
                        json!([{"type": "text", "text": "Inspect the repository"}]),
                    ),
                    message(
                        "assistant-opening",
                        "assistant",
                        json!([{"type": "text", "text": "I will inspect src/lib.rs."}]),
                    ),
                    message(
                        "assistant-process",
                        "assistant",
                        json!([
                            {"type": "thinking", "text": "I need to inspect the files", "signature": null},
                            {
                                "type": "tool_use",
                                "id": "call-1",
                                "name": "read",
                                "arguments": {"path": "src/lib.rs"}
                            }
                        ]),
                    ),
                    message(
                        "tool-result",
                        "system",
                        json!([{
                            "type": "tool_result",
                            "call_id": "call-1",
                            "name": "read",
                            "content": [{"type": "text", "text": tool_output}],
                            "truncation": {
                                "recall_key": "recall-call-1"
                            }
                        }]),
                    ),
                    message(
                        "assistant-final",
                        "assistant",
                        json!([{"type": "text", "text": "The repository uses a layered runtime."}]),
                    ),
                ],
            },
        )
        .await
        .unwrap();
}

fn projection_text(messages: &[rua_session::MessageLogEntry]) -> String {
    messages
        .iter()
        .map(|message| message.content.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn ready_history_replacement_is_durable_and_only_changes_model_projection() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store.save_chat_header(&header("chat-1")).await.unwrap();
    let raw_marker = "raw-tool-output-marker";
    append_trace_turn(
        &store,
        "chat-1",
        format!("{raw_marker}{}", "x".repeat(70_000)),
    )
    .await;

    let blocks = store
        .detect_history_replacement_blocks("chat-1", 4.0)
        .await
        .unwrap();
    assert_eq!(blocks.len(), 1);
    let block = blocks.into_iter().next().unwrap();
    assert_eq!(
        block.source.message_ids,
        vec!["assistant-opening", "assistant-process", "tool-result"]
    );
    assert_eq!(block.source.sequence_range.start_sequence, 2);
    assert_eq!(block.source.sequence_range.end_sequence, 4);
    assert_eq!(
        block.source.tool_references,
        vec![HistoryReplacementToolReference {
            tool_call_id: "call-1".to_string(),
            tool_name: "read".to_string(),
            call_sequence: 3,
            result_sequence: 4,
            recall_key: Some("recall-call-1".to_string()),
        }]
    );
    assert!(block.source_content.contains("I will inspect src/lib.rs."));
    assert!(!block
        .source_content
        .contains("The repository uses a layered runtime."));
    assert!(block.source.token_estimate >= 16_000);
    let locator = history_replacement_locator(&block.source);
    assert!(locator.contains("\"tool_call_id\":\"call-1\""));
    assert!(locator.contains("\"result_sequence\":4"));
    assert!(locator.contains("\"recall_key\":\"recall-call-1\""));

    let digest =
        "The agent inspected src/lib.rs and learned that the repository uses a layered runtime.";
    let record = store
        .publish_ready_history_replacement(
            &rua_core::ChatAddress::agent("agent-1", "chat-1"),
            HistoryReplacementReadyDraft {
                record_id: block.record_id.clone(),
                source: block.source.clone(),
                digest: digest.to_string(),
                replacement_token_estimate: 64,
                model: CheckpointModelIdentity {
                    provider: "provider-a".to_string(),
                    model: "model-a".to_string(),
                },
                generated_at: "2026-07-26T00:01:00Z".to_string(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        record.chat_address,
        rua_core::ChatAddress::agent("agent-1", "chat-1")
    );
    assert!(record.checksum.starts_with("sha256:"));
    assert_eq!(
        store
            .load_history_replacement("chat-1", &block.record_id)
            .await
            .unwrap(),
        Some(record)
    );
    assert!(store
        .history_replacement_path("chat-1", &block.record_id)
        .exists());

    let raw = store
        .build_context_projection(
            "chat-1",
            ContextProjectionRequest {
                current_user_input: None,
            },
        )
        .await
        .unwrap();
    assert!(projection_text(&raw.tail_messages).contains(raw_marker));

    let projected = store
        .build_context_projection_with_history_replacements(
            "chat-1",
            ContextProjectionRequest {
                current_user_input: None,
            },
            4.0,
        )
        .await
        .unwrap();
    let projected_text = projection_text(&projected.tail_messages);
    assert!(projected_text.contains(
        "[History Replacement; source: conversation_read_span(start_sequence=2, end_sequence=4);"
    ));
    assert!(projected_text.contains("call-1"));
    assert!(projected_text.contains("recall-call-1"));
    assert!(projected_text.contains(digest));
    assert!(projected_text.contains("The repository uses a layered runtime."));
    assert!(!projected_text.contains("I will inspect src/lib.rs."));
    assert!(!projected_text.contains(raw_marker));
    assert!(!projected_text.contains("\"type\":\"tool_use\""));
    assert!(!projected_text.contains("\"type\":\"tool_result\""));
    let manifest_path = store.history_replacement_manifest_path("chat-1");
    assert!(manifest_path.exists());
    std::fs::write(&manifest_path, b"not-json").unwrap();
    let rebuilt = store
        .build_context_projection_with_history_replacements(
            "chat-1",
            ContextProjectionRequest {
                current_user_input: None,
            },
            4.0,
        )
        .await
        .unwrap();
    assert!(!projection_text(&rebuilt.tail_messages).contains(raw_marker));
    let records = store
        .list_history_replacement_records_for_address(&ChatAddress::agent("agent-1", "chat-1"))
        .await
        .unwrap();
    assert_eq!(records.len(), 1);
    let rebuilt_manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    assert_eq!(rebuilt_manifest["version"], json!(1));
    let mut incomplete_manifest = rebuilt_manifest;
    incomplete_manifest["records"] = json!([]);
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&incomplete_manifest).unwrap(),
    )
    .unwrap();
    let records = store
        .list_history_replacement_records_for_address(&ChatAddress::agent("agent-1", "chat-1"))
        .await
        .unwrap();
    assert_eq!(
        records.len(),
        1,
        "a manifest missing records must rebuild from the authoritative files"
    );
    let raw_groups = store
        .projection_message_group_estimates_effective("chat-1", false, 4.0)
        .await
        .unwrap();
    let replaced_groups = store
        .projection_message_group_estimates_effective("chat-1", true, 4.0)
        .await
        .unwrap();
    assert!(
        replaced_groups
            .iter()
            .map(|group| group.token_estimate)
            .sum::<u64>()
            < raw_groups
                .iter()
                .map(|group| group.token_estimate)
                .sum::<u64>(),
        "planner groups must measure the digest-substituted copy"
    );
    assert!(
        projected.stable_token_estimate < raw.stable_token_estimate,
        "effective projection accounting must reflect replacement savings"
    );

    let recalled = store
        .conversation_read_span(
            "chat-1",
            ConversationReadSpanRequest {
                start_sequence: 2,
                end_sequence: 4,
                expand_before: 0,
                expand_after: 0,
            },
        )
        .await
        .unwrap();
    assert!(projection_text(&recalled.messages).contains(raw_marker));

    let record_path = store.history_replacement_path("chat-1", &block.record_id);
    let mut corrupt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
    corrupt["checksum"] = json!("sha256:corrupt");
    std::fs::write(&record_path, serde_json::to_vec_pretty(&corrupt).unwrap()).unwrap();
    let fallback = store
        .build_context_projection_with_history_replacements(
            "chat-1",
            ContextProjectionRequest {
                current_user_input: None,
            },
            4.0,
        )
        .await
        .unwrap();
    assert!(
        projection_text(&fallback.tail_messages).contains(raw_marker),
        "a changed invalid authority record must invalidate the manifest and fall back to raw history"
    );
}

#[tokio::test]
async fn project_history_replacement_loads_with_its_canonical_address() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    let chat_id = "project-chat";
    let chat_address = rua_core::ChatAddress::project("project-1", chat_id);
    store.save_chat_header(&header(chat_id)).await.unwrap();
    append_trace_turn(
        &store,
        chat_id,
        format!("{}{}", "project-marker", "x".repeat(70_000)),
    )
    .await;
    let block = store
        .detect_history_replacement_blocks(chat_id, 4.0)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let record = store
        .publish_ready_history_replacement(
            &chat_address,
            HistoryReplacementReadyDraft {
                record_id: block.record_id.clone(),
                source: block.source,
                digest: "Project-scoped replacement".to_string(),
                replacement_token_estimate: 32,
                model: CheckpointModelIdentity {
                    provider: "test".to_string(),
                    model: "test-model".to_string(),
                },
                generated_at: "2026-07-26T00:01:00Z".to_string(),
            },
        )
        .await
        .unwrap();

    assert_eq!(
        store
            .load_history_replacement_for_address(&chat_address, &record.record_id)
            .await
            .unwrap(),
        Some(record.clone())
    );
    assert_eq!(
        store
            .list_history_replacement_records_for_address(&chat_address)
            .await
            .unwrap(),
        vec![record.clone()]
    );

    let projection = store
        .build_effective_context_projection_for_address(
            &chat_address,
            ContextProjectionRequest {
                current_user_input: None,
            },
            true,
            4.0,
        )
        .await
        .unwrap();
    let projected_text = projection_text(&projection.tail_messages);
    assert!(projected_text.contains("Project-scoped replacement"));
    assert!(!projected_text.contains("project-marker"));

    let emergency_projection = store
        .build_emergency_context_projection(
            &chat_address,
            1,
            ContextProjectionRequest {
                current_user_input: None,
            },
            true,
            4.0,
        )
        .await
        .unwrap();
    let emergency_text = projection_text(&emergency_projection.tail_messages);
    assert!(emergency_text.contains("Project-scoped replacement"));
    assert!(!emergency_text.contains("project-marker"));
    assert_eq!(emergency_projection.history_replacement_evidence.len(), 1);

    let raw_groups = store
        .projection_message_group_estimates_effective_for_address(&chat_address, false, 4.0)
        .await
        .unwrap();
    let replaced_groups = store
        .projection_message_group_estimates_effective_for_address(&chat_address, true, 4.0)
        .await
        .unwrap();
    assert!(
        replaced_groups
            .iter()
            .map(|group| group.token_estimate)
            .sum::<u64>()
            < raw_groups
                .iter()
                .map(|group| group.token_estimate)
                .sum::<u64>()
    );

    store
        .append_conversation_turn_for_address(
            &chat_address,
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
                        json!([{"type": "text", "text": "Continue"}]),
                    ),
                    message(
                        "assistant-2",
                        "assistant",
                        json!([{"type": "text", "text": "Continued"}]),
                    ),
                ],
            },
        )
        .await
        .unwrap();
    assert!(store
        .load_history_replacement_for_address(&chat_address, &block.record_id)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn small_execution_trace_block_remains_ineligible() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store.save_chat_header(&header("chat-small")).await.unwrap();
    append_trace_turn(&store, "chat-small", "small output".to_string()).await;

    assert!(store
        .detect_history_replacement_blocks("chat-small", 4.0)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn superseded_history_replacement_versions_are_ignored_by_manifest_scan() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&header("chat-superseded"))
        .await
        .unwrap();
    append_trace_turn(
        &store,
        "chat-superseded",
        format!("superseded-marker{}", "x".repeat(70_000)),
    )
    .await;
    let block = store
        .detect_history_replacement_blocks("chat-superseded", 4.0)
        .await
        .unwrap()
        .remove(0);
    let record = store
        .publish_ready_history_replacement(
            &ChatAddress::agent("agent-1", "chat-superseded"),
            HistoryReplacementReadyDraft {
                record_id: block.record_id,
                source: block.source,
                digest: "Superseded digest.".to_string(),
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
    let path = store.history_replacement_path("chat-superseded", &record.record_id);
    let mut payload: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    payload["prompt_version"] = json!(1);
    std::fs::write(&path, serde_json::to_vec_pretty(&payload).unwrap()).unwrap();

    assert!(store
        .list_history_replacement_records("chat-superseded")
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn detector_only_uses_the_selected_assistant_response_variant() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&header("chat-variants"))
        .await
        .unwrap();
    let large_old = format!("old-variant-marker{}", "o".repeat(70_000));
    store
        .append_conversation_turn(
            "chat-variants",
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-07-26T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-07-26T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-07-26T00:00:03Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("variant-old".to_string()),
                messages: vec![
                    message(
                        "user-1",
                        "user",
                        json!([{"type": "text", "text": "Inspect the selected implementation"}]),
                    ),
                    message(
                        "old-process",
                        "assistant",
                        json!([{
                            "type": "tool_use",
                            "id": "old-call",
                            "name": "read",
                            "input": {"path": "old.rs"}
                        }]),
                    ),
                    message(
                        "old-result",
                        "tool",
                        json!([{
                            "type": "tool_result",
                            "id": "old-call",
                            "name": "read",
                            "output": [{"type": "text", "text": large_old}]
                        }]),
                    ),
                    message(
                        "old-final",
                        "assistant",
                        json!([{"type": "text", "text": "Old response"}]),
                    ),
                ],
            },
        )
        .await
        .unwrap();

    let large_new = format!("new-variant-marker{}", "n".repeat(70_000));
    store
        .append_messages_to_turn_as_new_variant(
            "chat-variants",
            "turn-1",
            TurnLifecycle {
                user_submitted_at: "2026-07-26T00:00:00Z".to_string(),
                assistant_started_at: Some("2026-07-26T00:01:01Z".to_string()),
                assistant_completed_at: Some("2026-07-26T00:01:03Z".to_string()),
                status: "completed".to_string(),
            },
            Some("variant-new".to_string()),
            vec![
                message(
                    "new-process",
                    "assistant",
                    json!([{
                        "type": "tool_use",
                        "id": "new-call",
                        "name": "read",
                        "input": {"path": "new.rs"}
                    }]),
                ),
                message(
                    "new-result",
                    "tool",
                    json!([{
                        "type": "tool_result",
                        "id": "new-call",
                        "name": "read",
                        "output": [{"type": "text", "text": large_new}]
                    }]),
                ),
                message(
                    "new-final",
                    "assistant",
                    json!([{"type": "text", "text": "New response"}]),
                ),
            ],
        )
        .await
        .unwrap();

    let selected_new = store
        .detect_history_replacement_blocks("chat-variants", 4.0)
        .await
        .unwrap();
    assert_eq!(selected_new.len(), 1);
    assert_eq!(
        selected_new[0].source.message_ids,
        vec!["new-process", "new-result"]
    );
    assert!(selected_new[0]
        .source_content
        .contains("new-variant-marker"));
    assert!(!selected_new[0]
        .source_content
        .contains("old-variant-marker"));

    assert!(store
        .set_turn_selected_variant("chat-variants", "turn-1", "variant-old".to_string(),)
        .await
        .unwrap());
    let selected_old = store
        .detect_history_replacement_blocks("chat-variants", 4.0)
        .await
        .unwrap();
    assert_eq!(selected_old.len(), 1);
    assert_eq!(
        selected_old[0].source.message_ids,
        vec!["old-process", "old-result"]
    );
    assert!(selected_old[0]
        .source_content
        .contains("old-variant-marker"));
    assert!(!selected_old[0]
        .source_content
        .contains("new-variant-marker"));
}

#[tokio::test]
async fn completed_tool_messages_do_not_close_active_block_before_visible_text() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&header("chat-active-boundary"))
        .await
        .unwrap();
    let active_lifecycle = TurnLifecycle {
        user_submitted_at: "2026-07-26T00:00:00Z".to_string(),
        assistant_started_at: Some("2026-07-26T00:00:01Z".to_string()),
        assistant_completed_at: None,
        status: "in_progress".to_string(),
    };
    store
        .append_conversation_turn(
            "chat-active-boundary",
            NewConversationTurn {
                turn_id: "turn-active".to_string(),
                lifecycle: active_lifecycle.clone(),
                selected_variant_id: None,
                messages: vec![message(
                    "user-active",
                    "user",
                    json!([{"type": "text", "text": "Inspect while the turn is active"}]),
                )],
            },
        )
        .await
        .unwrap();
    store
        .append_messages_to_turn(
            "chat-active-boundary",
            "turn-active",
            active_lifecycle.clone(),
            Some("variant-active".to_string()),
            vec![
                message(
                    "active-process",
                    "assistant",
                    json!([{
                        "type": "tool_use",
                        "id": "active-call",
                        "name": "read",
                        "input": {"path": "active.rs"}
                    }]),
                ),
                message(
                    "active-result",
                    "tool",
                    json!([{
                        "type": "tool_result",
                        "id": "active-call",
                        "name": "read",
                        "output": [{
                            "type": "text",
                            "text": format!("active-marker{}", "x".repeat(70_000))
                        }]
                    }]),
                ),
            ],
        )
        .await
        .unwrap();
    assert!(
        store
            .detect_history_replacement_blocks("chat-active-boundary", 4.0)
            .await
            .unwrap()
            .is_empty(),
        "completed tool Messages alone must not close an active block"
    );

    store
        .append_messages_to_turn(
            "chat-active-boundary",
            "turn-active",
            active_lifecycle,
            Some("variant-active".to_string()),
            vec![message(
                "active-visible",
                "assistant",
                json!([{"type": "text", "text": "The active read is complete."}]),
            )],
        )
        .await
        .unwrap();
    let closed = store
        .detect_history_replacement_blocks("chat-active-boundary", 4.0)
        .await
        .unwrap();
    assert_eq!(closed.len(), 1);
    assert!(!closed[0]
        .source_content
        .contains("The active read is complete."));
    let closed_block = closed.into_iter().next().unwrap();
    store
        .publish_ready_history_replacement(
            &rua_core::ChatAddress::agent("agent-1", "chat-active-boundary"),
            HistoryReplacementReadyDraft {
                record_id: closed_block.record_id.clone(),
                source: closed_block.source,
                digest: "The active read completed successfully.".to_string(),
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
    store
        .append_messages_to_turn(
            "chat-active-boundary",
            "turn-active",
            TurnLifecycle {
                user_submitted_at: "2026-07-26T00:00:00Z".to_string(),
                assistant_started_at: Some("2026-07-26T00:00:01Z".to_string()),
                assistant_completed_at: None,
                status: "in_progress".to_string(),
            },
            Some("variant-active".to_string()),
            vec![message(
                "later-visible",
                "assistant",
                json!([{"type": "text", "text": "The active turn continued later."}]),
            )],
        )
        .await
        .unwrap();
    let projected = store
        .build_context_projection_with_history_replacements(
            "chat-active-boundary",
            ContextProjectionRequest {
                current_user_input: None,
            },
            4.0,
        )
        .await
        .unwrap();
    let projected_text = projection_text(&projected.tail_messages);
    assert!(projected_text.contains("The active read completed successfully."));
    assert!(projected_text.contains("The active turn continued later."));
}

#[tokio::test]
async fn projection_replaces_two_blocks_that_share_one_message_id_by_part_anchor() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&header("chat-shared-message"))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            "chat-shared-message",
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
                        json!([{"type": "text", "text": "Inspect alpha and beta"}]),
                    ),
                    message(
                        "assistant-shared",
                        "assistant",
                        json!([
                            {"type": "text", "text": "Inspect alpha next."},
                            {
                                "type": "tool_use",
                                "id": "call-alpha",
                                "name": "read",
                                "input": {"path": "alpha.rs"}
                            },
                            {
                                "type": "tool_result",
                                "id": "call-alpha",
                                "name": "read",
                                "output": [{
                                    "type": "text",
                                    "text": format!("alpha-raw-marker{}", "a".repeat(70_000))
                                }]
                            },
                            {"type": "text", "text": "Alpha is configured."},
                            {
                                "type": "tool_use",
                                "id": "call-beta",
                                "name": "read",
                                "input": {"path": "beta.rs"}
                            },
                            {
                                "type": "tool_result",
                                "id": "call-beta",
                                "name": "read",
                                "output": [{
                                    "type": "text",
                                    "text": format!("beta-raw-marker{}", "b".repeat(70_000))
                                }]
                            },
                            {"type": "text", "text": "Beta is configured."}
                        ]),
                    ),
                ],
            },
        )
        .await
        .unwrap();

    let blocks = store
        .detect_history_replacement_blocks("chat-shared-message", 4.0)
        .await
        .unwrap();
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0].source.start_anchor.content_part_index, 0);
    assert_eq!(blocks[0].source.end_anchor.content_part_index, 2);
    assert_eq!(blocks[1].source.start_anchor.content_part_index, 3);
    assert_eq!(blocks[1].source.end_anchor.content_part_index, 5);
    for (index, block) in blocks.iter().enumerate() {
        store
            .publish_ready_history_replacement(
                &rua_core::ChatAddress::agent("agent-1", "chat-shared-message"),
                HistoryReplacementReadyDraft {
                    record_id: block.record_id.clone(),
                    source: block.source.clone(),
                    digest: format!("Digest for block {index}."),
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
    }

    let projected = store
        .build_context_projection_with_history_replacements(
            "chat-shared-message",
            ContextProjectionRequest {
                current_user_input: None,
            },
            4.0,
        )
        .await
        .unwrap();
    let text = projection_text(&projected.tail_messages);
    assert_eq!(text.matches("[History Replacement; source:").count(), 2);
    assert!(text.contains("Digest for block 0."));
    assert!(text.contains("Digest for block 1."));
    assert!(!text.contains("Inspect alpha next."));
    assert!(!text.contains("Alpha is configured."));
    assert!(text.contains("Beta is configured."));
    assert!(!text.contains("alpha-raw-marker"));
    assert!(!text.contains("beta-raw-marker"));
    assert_eq!(projected.history_replacement_evidence.len(), 2);
    for evidence in &projected.history_replacement_evidence {
        assert_eq!(evidence.status, "applied");
        assert!(blocks
            .iter()
            .any(|block| block.record_id == evidence.record_id));
        assert!(!evidence.mutations.is_empty());
        assert!(evidence.mutations.iter().all(|mutation| {
            mutation.message_id == "assistant-shared"
                && Some(mutation.before.clone()) != mutation.after
        }));
        let carrier = evidence.digest_carrier.as_ref().expect("digest carrier");
        assert_eq!(carrier.message_id, "assistant-shared");
        assert!(evidence
            .mutations
            .iter()
            .any(|mutation| mutation.content_part_index == carrier.content_part_index));
    }
}

#[tokio::test]
async fn rejected_record_deduplicates_same_model_and_allows_new_model_transition_to_ready() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&header("chat-rejected"))
        .await
        .unwrap();
    let raw_marker = "rejected-raw-marker";
    append_trace_turn(
        &store,
        "chat-rejected",
        format!("{raw_marker}{}", "x".repeat(70_000)),
    )
    .await;
    let block = store
        .detect_history_replacement_blocks("chat-rejected", 4.0)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let model_a = CheckpointModelIdentity {
        provider: "provider-a".to_string(),
        model: "model-a".to_string(),
    };
    let model_b = CheckpointModelIdentity {
        provider: "provider-b".to_string(),
        model: "model-b".to_string(),
    };

    let rejected = store
        .publish_rejected_history_replacement(
            &rua_core::ChatAddress::agent("agent-1", "chat-rejected"),
            HistoryReplacementRejectedDraft {
                record_id: block.record_id.clone(),
                source: block.source.clone(),
                model: model_a.clone(),
                reason: HistoryReplacementRejectionReason::InsufficientSavings,
                replacement_token_estimate: 10_000,
                saved_token_estimate: 7_000,
                rejected_at: "2026-07-26T00:01:00Z".to_string(),
            },
        )
        .await
        .unwrap();
    assert!(rejected.is_rejected_for_model(&model_a));
    assert!(!rejected.allows_generation_for_model(&model_a));
    assert!(rejected.allows_generation_for_model(&model_b));
    assert!(rejected.checksum.starts_with("sha256:"));
    assert!(matches!(
        rejected.state,
        HistoryReplacementRecordState::Rejected {
            ref generations
        } if generations.len() == 1
    ));

    let duplicate = store
        .publish_rejected_history_replacement(
            &rua_core::ChatAddress::agent("agent-1", "chat-rejected"),
            HistoryReplacementRejectedDraft {
                record_id: block.record_id.clone(),
                source: block.source.clone(),
                model: model_a.clone(),
                reason: HistoryReplacementRejectionReason::InsufficientReduction,
                replacement_token_estimate: 9_000,
                saved_token_estimate: 8_000,
                rejected_at: "2026-07-26T00:02:00Z".to_string(),
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        duplicate.state,
        HistoryReplacementRecordState::Rejected {
            ref generations
        } if generations.len() == 1
    ));

    let projected_rejected = store
        .build_context_projection_with_history_replacements(
            "chat-rejected",
            ContextProjectionRequest {
                current_user_input: None,
            },
            4.0,
        )
        .await
        .unwrap();
    assert!(projection_text(&projected_rejected.tail_messages).contains(raw_marker));
    let record_path = store.history_replacement_path("chat-rejected", &block.record_id);
    let mut corrupt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
    corrupt["checksum"] = json!("sha256:corrupt-rejected");
    std::fs::write(&record_path, serde_json::to_vec_pretty(&corrupt).unwrap()).unwrap();
    let corrupt_fallback = store
        .build_context_projection_with_history_replacements(
            "chat-rejected",
            ContextProjectionRequest {
                current_user_input: None,
            },
            4.0,
        )
        .await
        .unwrap();
    assert!(projection_text(&corrupt_fallback.tail_messages).contains(raw_marker));
    std::fs::write(&record_path, serde_json::to_vec_pretty(&duplicate).unwrap()).unwrap();
    assert!(store
        .publish_ready_history_replacement(
            &rua_core::ChatAddress::agent("agent-1", "chat-rejected"),
            HistoryReplacementReadyDraft {
                record_id: block.record_id.clone(),
                source: block.source.clone(),
                digest: "same model should not overwrite its rejection".to_string(),
                replacement_token_estimate: 32,
                model: model_a,
                generated_at: "2026-07-26T00:03:00Z".to_string(),
            },
        )
        .await
        .is_err());

    let ready = store
        .publish_ready_history_replacement(
            &rua_core::ChatAddress::agent("agent-1", "chat-rejected"),
            HistoryReplacementReadyDraft {
                record_id: block.record_id.clone(),
                source: block.source,
                digest: "A different active Chat model produced a useful digest.".to_string(),
                replacement_token_estimate: 32,
                model: model_b,
                generated_at: "2026-07-26T00:04:00Z".to_string(),
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        ready.state,
        HistoryReplacementRecordState::Ready { .. }
    ));
    assert_eq!(ready.created_at, "2026-07-26T00:01:00Z");
    let projected_ready = store
        .build_context_projection_with_history_replacements(
            "chat-rejected",
            ContextProjectionRequest {
                current_user_input: None,
            },
            4.0,
        )
        .await
        .unwrap();
    let projected_text = projection_text(&projected_ready.tail_messages);
    assert!(projected_text.contains("A different active Chat model"));
    assert!(!projected_text.contains(raw_marker));
}

#[tokio::test]
async fn variant_selection_and_rollback_reuse_records_until_source_is_permanently_unreachable() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    let chat_id = "chat-variant-lifecycle";
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
                selected_variant_id: Some("variant-old".to_string()),
                messages: vec![
                    message(
                        "lifecycle-user",
                        "user",
                        json!([{"type": "text", "text": "Inspect both implementations"}]),
                    ),
                    message(
                        "old-process",
                        "assistant",
                        json!([{
                            "type": "tool_use",
                            "id": "old-call",
                            "name": "read",
                            "input": {"path": "old.rs"}
                        }]),
                    ),
                    message(
                        "old-result",
                        "tool",
                        json!([{
                            "type": "tool_result",
                            "id": "old-call",
                            "name": "read",
                            "output": [{
                                "type": "text",
                                "text": format!("old-lifecycle-marker{}", "o".repeat(70_000))
                            }]
                        }]),
                    ),
                    message(
                        "old-final",
                        "assistant",
                        json!([{"type": "text", "text": "Old implementation inspected."}]),
                    ),
                ],
            },
        )
        .await
        .unwrap();
    let old_block = store
        .detect_history_replacement_blocks(chat_id, 4.0)
        .await
        .unwrap()
        .remove(0);
    store
        .publish_ready_history_replacement(
            &rua_core::ChatAddress::agent("agent-1", chat_id),
            HistoryReplacementReadyDraft {
                record_id: old_block.record_id.clone(),
                source: old_block.source.clone(),
                digest: "Digest for the old implementation.".to_string(),
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

    store
        .append_messages_to_turn_as_new_variant(
            chat_id,
            "turn-1",
            TurnLifecycle {
                user_submitted_at: "2026-07-26T00:00:00Z".to_string(),
                assistant_started_at: Some("2026-07-26T00:02:01Z".to_string()),
                assistant_completed_at: Some("2026-07-26T00:02:03Z".to_string()),
                status: "completed".to_string(),
            },
            Some("variant-new".to_string()),
            vec![
                message(
                    "new-process",
                    "assistant",
                    json!([{
                        "type": "tool_use",
                        "id": "new-call",
                        "name": "read",
                        "input": {"path": "new.rs"}
                    }]),
                ),
                message(
                    "new-result",
                    "tool",
                    json!([{
                        "type": "tool_result",
                        "id": "new-call",
                        "name": "read",
                        "output": [{
                            "type": "text",
                            "text": format!("new-lifecycle-marker{}", "n".repeat(70_000))
                        }]
                    }]),
                ),
                message(
                    "new-final",
                    "assistant",
                    json!([{"type": "text", "text": "New implementation inspected."}]),
                ),
            ],
        )
        .await
        .unwrap();
    let new_block = store
        .detect_history_replacement_blocks(chat_id, 4.0)
        .await
        .unwrap()
        .remove(0);
    store
        .publish_ready_history_replacement(
            &rua_core::ChatAddress::agent("agent-1", chat_id),
            HistoryReplacementReadyDraft {
                record_id: new_block.record_id.clone(),
                source: new_block.source.clone(),
                digest: "Digest for the new implementation.".to_string(),
                replacement_token_estimate: 32,
                model: CheckpointModelIdentity {
                    provider: "provider-a".to_string(),
                    model: "model-a".to_string(),
                },
                generated_at: "2026-07-26T00:03:00Z".to_string(),
            },
        )
        .await
        .unwrap();

    assert!(
        store
            .history_replacement_path(chat_id, &old_block.record_id)
            .is_file(),
        "temporarily unselected Variant records must remain durable"
    );
    assert!(store
        .set_turn_selected_variant(chat_id, "turn-1", "variant-old".to_string())
        .await
        .unwrap());
    let selected_old = store
        .build_context_projection_with_history_replacements(
            chat_id,
            ContextProjectionRequest {
                current_user_input: None,
            },
            4.0,
        )
        .await
        .unwrap();
    let selected_old_text = projection_text(&selected_old.tail_messages);
    assert!(selected_old_text.contains("Digest for the old implementation."));
    assert!(!selected_old_text.contains("old-lifecycle-marker"));

    assert!(store
        .set_turn_selected_variant(chat_id, "turn-1", "variant-new".to_string())
        .await
        .unwrap());
    store
        .rollback_active_conversation_to_turn(chat_id, "turn-1", "variant-old".to_string())
        .await
        .unwrap()
        .expect("rollback target");

    assert!(store
        .history_replacement_path(chat_id, &old_block.record_id)
        .is_file());
    assert!(
        !store
            .history_replacement_path(chat_id, &new_block.record_id)
            .exists(),
        "rollback permanently removes the unretained Variant lineage"
    );
    assert!(store
        .history_replacement_tombstone_path(chat_id, &new_block.record_id)
        .is_file());
    let rolled_back = store
        .build_context_projection_with_history_replacements(
            chat_id,
            ContextProjectionRequest {
                current_user_input: None,
            },
            4.0,
        )
        .await
        .unwrap();
    assert!(
        projection_text(&rolled_back.tail_messages).contains("Digest for the old implementation.")
    );
}

#[tokio::test]
async fn branch_rebases_compatible_ready_record_to_target_sequences_and_lineage() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    let source_chat_id = "chat-branch-source";
    let target_chat_id = "chat-branch-target";
    store
        .save_chat_header(&header(source_chat_id))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            source_chat_id,
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-07-26T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-07-26T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-07-26T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("variant-unused".to_string()),
                messages: vec![
                    message(
                        "branch-user",
                        "user",
                        json!([{"type": "text", "text": "Inspect the selected branch"}]),
                    ),
                    message(
                        "unused-final",
                        "assistant",
                        json!([{"type": "text", "text": "Unused response"}]),
                    ),
                ],
            },
        )
        .await
        .unwrap();
    let branch_messages = vec![
        message(
            "branch-process",
            "assistant",
            json!([{
                "type": "tool_use",
                "id": "branch-call",
                "name": "read",
                "input": {"path": "branch.rs"}
            }]),
        ),
        message(
            "branch-result",
            "tool",
            json!([{
                "type": "tool_result",
                "id": "branch-call",
                "name": "read",
                "output": [{
                    "type": "text",
                    "text": format!("branch-raw-marker{}", "b".repeat(70_000))
                }]
            }]),
        ),
        message(
            "branch-final",
            "assistant",
            json!([{"type": "text", "text": "Selected branch response"}]),
        ),
    ];
    store
        .append_messages_to_turn_as_new_variant(
            source_chat_id,
            "turn-1",
            TurnLifecycle {
                user_submitted_at: "2026-07-26T00:00:00Z".to_string(),
                assistant_started_at: Some("2026-07-26T00:01:01Z".to_string()),
                assistant_completed_at: Some("2026-07-26T00:01:03Z".to_string()),
                status: "completed".to_string(),
            },
            Some("variant-selected".to_string()),
            branch_messages.clone(),
        )
        .await
        .unwrap();
    let source_block = store
        .detect_history_replacement_blocks(source_chat_id, 4.0)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(source_block.source.sequence_range.start_sequence, 3);
    assert_eq!(source_block.source.tool_references[0].call_sequence, 3);
    assert_eq!(source_block.source.tool_references[0].result_sequence, 4);
    store
        .publish_ready_history_replacement(
            &rua_core::ChatAddress::agent("agent-1", source_chat_id),
            HistoryReplacementReadyDraft {
                record_id: source_block.record_id.clone(),
                source: source_block.source,
                digest: "Inherited branch digest.".to_string(),
                replacement_token_estimate: 32,
                model: CheckpointModelIdentity {
                    provider: "provider-a".to_string(),
                    model: "model-a".to_string(),
                },
                generated_at: "2026-07-26T00:02:00Z".to_string(),
            },
        )
        .await
        .unwrap();

    store
        .save_chat_header(&header(target_chat_id))
        .await
        .unwrap();
    let mut target_messages = vec![message(
        "branch-user",
        "user",
        json!([{"type": "text", "text": "Inspect the selected branch"}]),
    )];
    target_messages.extend(branch_messages);
    store
        .append_conversation_turn(
            target_chat_id,
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-07-26T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-07-26T00:01:01Z".to_string()),
                    assistant_completed_at: Some("2026-07-26T00:01:03Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("variant-selected".to_string()),
                messages: target_messages,
            },
        )
        .await
        .unwrap();

    assert_eq!(
        store
            .inherit_compatible_history_replacements(
                &rua_core::ChatAddress::agent("agent-1", source_chat_id),
                &rua_core::ChatAddress::agent("agent-1", target_chat_id),
            )
            .await
            .unwrap(),
        1
    );
    let target_block = store
        .detect_history_replacement_blocks(target_chat_id, 4.0)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(target_block.source.sequence_range.start_sequence, 2);
    let inherited = store
        .load_history_replacement(target_chat_id, &target_block.record_id)
        .await
        .unwrap()
        .expect("rebased target record");
    assert_eq!(inherited.ready_digest(), Some("Inherited branch digest."));
    assert_eq!(inherited.source.tool_references[0].call_sequence, 2);
    assert_eq!(inherited.source.tool_references[0].result_sequence, 3);
    assert_ne!(inherited.record_id, source_block.record_id);

    let projection = store
        .build_context_projection_with_history_replacements(
            target_chat_id,
            ContextProjectionRequest {
                current_user_input: None,
            },
            4.0,
        )
        .await
        .unwrap();
    let projection = projection_text(&projection.tail_messages);
    assert!(projection.contains("Inherited branch digest."));
    assert!(!projection.contains("branch-raw-marker"));
}

#[tokio::test]
async fn projection_exclusions_only_disable_history_replacements_covering_excluded_messages() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    let chat_id = "excluded-history-replacement";
    store.save_chat_header(&header(chat_id)).await.unwrap();
    append_trace_turn(&store, chat_id, "raw-history-evidence".repeat(4_000)).await;
    let block = store
        .detect_history_replacement_blocks(chat_id, 4.0)
        .await
        .unwrap()
        .remove(0);
    store
        .publish_ready_history_replacement(
            &ChatAddress::agent("agent-1", chat_id),
            HistoryReplacementReadyDraft {
                record_id: block.record_id,
                source: block.source,
                digest: "replacement-evidence".into(),
                replacement_token_estimate: 64,
                model: CheckpointModelIdentity {
                    provider: "fixture".into(),
                    model: "fixture".into(),
                },
                generated_at: "2026-09-21T00:00:00Z".into(),
            },
        )
        .await
        .unwrap();
    store
        .save_projection_exclusions(chat_id, vec!["assistant-final".into()])
        .await
        .unwrap();
    let projected = store
        .build_context_projection_with_history_replacements(
            chat_id,
            ContextProjectionRequest {
                current_user_input: None,
            },
            4.0,
        )
        .await
        .unwrap();
    assert!(projection_text(&projected.tail_messages).contains("replacement-evidence"));
    assert!(!projection_text(&projected.tail_messages).contains("raw-history-evidence"));

    store
        .save_projection_exclusions(
            chat_id,
            vec!["assistant-process".into(), "tool-result".into()],
        )
        .await
        .unwrap();
    let filtered = store
        .build_context_projection_with_history_replacements(
            chat_id,
            ContextProjectionRequest {
                current_user_input: None,
            },
            4.0,
        )
        .await
        .unwrap();
    assert!(!projection_text(&filtered.tail_messages).contains("replacement-evidence"));
    assert!(projection_text(&filtered.tail_messages).contains("I will inspect src/lib.rs."));
    assert_eq!(
        store
            .read_message_range(chat_id, 1, 10)
            .await
            .unwrap()
            .len(),
        5
    );
}
