use agent_types::{ContentPart, ModelDialogueItem, Role, ToolCall, ToolResult};
use rua_agent::projection_preparation::{
    final_projection_token_estimate, finalize_projection_message_group_estimates,
    project_message_log_entries_for_model, project_tool_results_for_model,
    ToolResultProjectionHook,
};
use rua_session::{
    CollaborationResultRuntimeNote, ContextProjection, MessageLogEntry,
    ProjectionMessageGroupEstimate, RuntimeNote, RuntimeNotePayload,
    TaskAttemptTerminalRuntimeNote, TaskWatchOutputProjection, TaskWatchOutputRecord,
    TaskWatchOutputRuntimeNote,
};

fn tool_round(call_id: &str, output: &str) -> [ModelDialogueItem; 2] {
    let mut result = ToolResult::text(call_id, output);
    result.name = Some("bash".to_string());
    [
        ModelDialogueItem {
            id: None,
            role: Role::Assistant,
            content: vec![ContentPart::ToolUse {
                call: ToolCall::new(call_id, "bash", serde_json::json!({"command": "printf x"})),
            }],
            name: None,
            created_at: None,
            metadata: serde_json::Map::new(),
        },
        ModelDialogueItem {
            id: None,
            role: Role::Tool,
            content: vec![ContentPart::ToolResult { result }],
            name: None,
            created_at: None,
            metadata: serde_json::Map::new(),
        },
    ]
}

fn log_message(sequence: u64, message_id: &str, message: ModelDialogueItem) -> MessageLogEntry {
    MessageLogEntry {
        message_id: message_id.to_string(),
        sequence,
        turn_id: "turn-1".to_string(),
        role: match message.role {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
            _ => "system",
        }
        .to_string(),
        created_at: "2026-07-26T00:00:00Z".to_string(),
        status: "completed".to_string(),
        content: serde_json::to_value(message.content).unwrap(),
        provider: None,
        tool: None,
        metadata: message.metadata,
    }
}

#[test]
fn projection_truncation_batches_keep_history_stable_between_boundaries() {
    let running: rua_config::AgentsRunningConfig = serde_json::from_value(serde_json::json!({
        "tool_result_compact": {
            "projection_truncation_enabled": true,
            "projection_truncation_batch_size": 10,
            "recent_n": 4,
            "old_max_bytes": 32
        }
    }))
    .unwrap();
    let mut messages = vec![ModelDialogueItem::text(Role::User, "inspect")];
    let mut previous = Vec::<ModelDialogueItem>::new();
    for count in 1usize..=31 {
        messages.extend(tool_round(&format!("call-{count}"), &"x".repeat(200)));
        let raw = messages.clone();
        let projected = project_tool_results_for_model(&messages, &running);
        let shortened = (count / 10 * 10).saturating_sub(4);
        for index in 0..count {
            assert_eq!(
                projected[index * 2 + 2].tool_results()[0]
                    .text_content()
                    .len()
                    <= 32,
                index < shortened,
                "after {count} results, only the first {shortened} may be shortened"
            );
        }
        if count % 10 != 0 && !previous.is_empty() {
            assert_eq!(
                projected[..previous.len()],
                previous,
                "appending within a batch must not change the model-visible prefix"
            );
        }
        assert_eq!(messages, raw);
        assert_eq!(
            project_tool_results_for_model(&messages, &running),
            projected,
            "rebuilding a projection or retrying must not advance the batch"
        );
        previous = projected;
    }

    let selected_branch = &messages[..17]; // Eight completed tool results.
    let projected = project_tool_results_for_model(selected_branch, &running);
    assert_eq!(projected, selected_branch);
}

#[test]
fn projection_truncation_batches_count_completed_results_in_multi_tool_groups() {
    let running: rua_config::AgentsRunningConfig = serde_json::from_value(serde_json::json!({
        "tool_result_compact": {
            "projection_truncation_enabled": true,
            "projection_truncation_batch_size": 10,
            "recent_n": 4,
            "old_max_bytes": 32
        }
    }))
    .unwrap();
    let mut calls = ModelDialogueItem::text(Role::Assistant, "");
    calls.content.clear();
    let mut results = ModelDialogueItem::text(Role::Tool, "");
    results.content.clear();
    for index in 1..=10 {
        let [call, result] = tool_round(&format!("call-{index}"), &"x".repeat(200));
        calls.content.extend(call.content);
        if index < 10 {
            results.content.extend(result.content);
        }
    }
    let mut messages = vec![calls, results];
    let pending = project_tool_results_for_model(&messages, &running);
    assert!(pending[1]
        .tool_results()
        .iter()
        .all(|result| result.text_content().len() == 200));

    let [_, last_result] = tool_round("call-10", &"x".repeat(200));
    messages[1].content.extend(last_result.content);
    let complete = project_tool_results_for_model(&messages, &running);
    for (index, result) in complete[1].tool_results().iter().enumerate() {
        assert_eq!(result.text_content().len() <= 32, index < 6);
    }
    assert_eq!(complete[0].content, messages[0].content);
}

#[test]
fn projection_truncation_batches_share_boundary_with_checkpoint_sources_and_estimates() {
    let mut running = rua_config::AgentsRunningConfig::default();
    running.tool_result_compact.projection_truncation_enabled = true;
    running.tool_result_compact.projection_truncation_batch_size = 10;
    running.tool_result_compact.recent_n = 4;
    running.tool_result_compact.old_max_bytes = 32;
    let mut entries = vec![log_message(
        1,
        "user",
        ModelDialogueItem::text(Role::User, "inspect"),
    )];
    for count in 1..=21 {
        for message in tool_round(&format!("call-{count}"), &"x".repeat(200)) {
            let sequence = entries.len() as u64 + 1;
            entries.push(log_message(
                sequence,
                &format!("message-{sequence}"),
                message,
            ));
        }
    }
    let tail_messages = entries.split_off(9);
    let projection = ContextProjection {
        chat_id: "chat-batched".to_string(),
        agent_id: "agent-1".to_string(),
        active_checkpoint_id: None,
        summary: None,
        summary_model: None,
        tail_start_sequence: Some(10),
        tail_end_sequence: Some(43),
        retained_messages: entries,
        tail_messages,
        current_user_input: None,
        stable_token_estimate: 2_000,
        history_replacement_evidence: Vec::new(),
    };
    let source =
        rua_agent::summary_checkpoint::summary_projection_slice(&projection, &running, 37).unwrap();
    let results = source
        .messages
        .iter()
        .flat_map(ModelDialogueItem::tool_results)
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 18);
    for (index, result) in results.iter().enumerate() {
        assert_eq!(
            result.text_content().len() <= 32,
            index < 16,
            "checkpoint source must use the full history's batch, not its shorter prefix's batch"
        );
    }
    let full_source =
        rua_agent::summary_checkpoint::summary_projection_slice(&projection, &running, 43).unwrap();
    assert_eq!(
        final_projection_token_estimate(&projection, &running),
        react_agent::estimate_model_visible_conversation_tokens(&full_source.messages, 4.0)
    );
    assert!(projection
        .tail_messages
        .last()
        .unwrap()
        .content
        .to_string()
        .contains(&"x".repeat(200)));
}

#[test]
fn projection_truncation_batch_window_edges_and_execution_limit_are_independent() {
    let mut messages = Vec::new();
    for count in 1..=10 {
        messages.extend(tool_round(&format!("call-{count}"), &"x".repeat(200)));
    }
    for (batch_size, recent_n, shortened) in [
        (10, 0, 10),
        (10, 4, 6),
        (10, 20, 0),
        (1, 4, 6),
        (u32::MAX, 0, 0),
    ] {
        let mut running = rua_config::AgentsRunningConfig::default();
        running.tool_result_compact.projection_truncation_enabled = true;
        running.tool_result_compact.projection_truncation_batch_size = batch_size;
        running.tool_result_compact.recent_n = recent_n;
        running.tool_result_compact.old_max_bytes = 32;
        let projected = project_tool_results_for_model(&messages, &running);
        assert_eq!(
            projected
                .iter()
                .flat_map(ModelDialogueItem::tool_results)
                .filter(|result| result.text_content().len() <= 32)
                .count(),
            shortened
        );
        running.tool_result_compact.old_max_bytes = 0;
        assert_eq!(
            project_tool_results_for_model(&messages, &running),
            messages
        );
    }

    let mut running = rua_config::AgentsRunningConfig::default();
    running.tool_result_compact.projection_truncation_batch_size = 10;
    running.tool_result_compact.execution_output_max_tokens = 128;
    for enabled in [false, true] {
        running.tool_result_compact.projection_truncation_enabled = enabled;
        let projected =
            project_tool_results_for_model(&tool_round("large", &"x".repeat(10_000)), &running);
        let result = projected[1].tool_results()[0];
        assert!(result.truncation.is_some());
        assert!(rua_tool::result_store::estimate_text_tokens(&result.text_content(), 4.0) <= 128);
    }
}

#[tokio::test]
async fn projection_tool_result_hook_is_independently_gated() {
    let mut running = rua_config::AgentsRunningConfig::default();
    running.tool_result_compact.enabled = false;
    assert!(ToolResultProjectionHook::from_running_config(&running).is_none());

    running.tool_result_compact.projection_truncation_enabled = true;
    running.tool_result_compact.recent_n = 1;
    running.tool_result_compact.old_max_bytes = 16;
    running.context_compact.early_background_compaction_enabled = false;

    let hook = ToolResultProjectionHook::from_running_config(&running)
        .expect("projection truncation should have its own switch");
    let mut messages = vec![ModelDialogueItem::text(Role::User, "inspect")];
    for index in 1..=3 {
        messages.extend(tool_round(
            &format!("call-{index}"),
            &format!("result-{index}-{}", "x".repeat(100)),
        ));
    }

    let projected = hook.projected_messages(&messages);
    assert!(!projected[2].tool_results()[0].text_content().is_empty());
    assert!(projected[2].tool_results()[0].text_content().len() <= 16);
    assert!(projected[4].tool_results()[0].text_content().len() <= 16);
    assert!(projected[6].tool_results()[0].text_content().len() > 16);
    let evidence = projected[0].metadata["rua_llm_debug_tool_result_truncation"]
        .as_array()
        .expect("tool truncation evidence");
    assert_eq!(evidence.len(), 2);
    assert_eq!(evidence[0]["call_id"], "call-1");
    assert_eq!(evidence[0]["kind"], "truncated");
    assert_eq!(evidence[1]["call_id"], "call-2");
    assert_eq!(evidence[1]["kind"], "truncated");
    assert!(evidence.iter().all(|entry| {
        entry["before_bytes"].as_u64().unwrap() > entry["after_bytes"].as_u64().unwrap()
    }));
}

#[test]
fn planner_group_estimate_applies_tool_truncation_after_digest_substitution() {
    let mut running = rua_config::AgentsRunningConfig::default();
    running.tool_result_compact.projection_truncation_enabled = true;
    running.tool_result_compact.recent_n = 1;
    running.tool_result_compact.old_max_bytes = 16;
    let mut tail_messages = vec![log_message(
        1,
        "history-replacement",
        ModelDialogueItem::text(
            Role::Assistant,
            "[History Replacement; source: conversation_read_span(start_sequence=2, end_sequence=3)]\n\nDigest marker",
        ),
    )];
    let mut sequence = 2u64;
    for index in 1..=3 {
        for message in tool_round(
            &format!("call-{index}"),
            &format!("raw-{index}-{}", "x".repeat(1_000)),
        ) {
            tail_messages.push(log_message(
                sequence,
                &format!("message-{sequence}"),
                message,
            ));
            sequence += 1;
        }
    }
    let projection = ContextProjection {
        chat_id: "chat-1".to_string(),
        agent_id: "agent-1".to_string(),
        active_checkpoint_id: None,
        summary: None,
        summary_model: None,
        tail_start_sequence: Some(1),
        tail_end_sequence: Some(sequence - 1),
        retained_messages: Vec::new(),
        tail_messages,
        current_user_input: None,
        stable_token_estimate: 2_000,
        history_replacement_evidence: Vec::new(),
    };
    let mut groups = vec![ProjectionMessageGroupEstimate {
        turn_id: "turn-1".to_string(),
        turn_ordinal: 1,
        first_sequence: 1,
        last_sequence: sequence - 1,
        token_estimate: 2_000,
        foldable: true,
        retained_sequence_ranges: Vec::new(),
    }];

    finalize_projection_message_group_estimates(&projection, &mut groups, &running);

    assert!(groups[0].token_estimate < 2_000);
    assert!(final_projection_token_estimate(&projection, &running) < 2_000);
    assert!(projection.tail_messages[0]
        .content
        .to_string()
        .contains("Digest marker"));
}

#[test]
fn canonical_text_envelope_bytes_do_not_trigger_truncation() {
    let mut messages = vec![ModelDialogueItem::text(Role::User, "inspect")];
    messages.extend(tool_round("call-1", &"x".repeat(100)));
    let mut running = rua_config::AgentsRunningConfig::default();
    running.tool_result_compact.projection_truncation_enabled = true;
    running.tool_result_compact.recent_n = 0;
    running.tool_result_compact.old_max_bytes = 110;
    let hook = ToolResultProjectionHook::from_running_config(&running).unwrap();

    let projected = hook.projected_messages(&messages);

    assert_eq!(projected[2].tool_results()[0].text_content().len(), 100);
}

#[test]
fn non_text_tool_result_is_not_removed_as_empty() {
    let mut messages = vec![ModelDialogueItem::text(Role::User, "inspect")];
    messages.extend(tool_round("call-1", "placeholder"));
    let ContentPart::ToolResult { result } = &mut messages[2].content[0] else {
        panic!("expected tool result");
    };
    result.content = vec![ContentPart::Image {
        url: "data:image/png;base64,AA==".into(),
        media_type: Some("image/png".into()),
        alt: None,
    }];
    let mut running = rua_config::AgentsRunningConfig::default();
    running.tool_result_compact.projection_truncation_enabled = true;
    running.tool_result_compact.recent_n = 0;
    running.tool_result_compact.old_max_bytes = 4_096;
    let hook = ToolResultProjectionHook::from_running_config(&running).unwrap();

    let projected = hook.projected_messages(&messages);

    assert!(matches!(
        projected[2].tool_results()[0].content.as_slice(),
        [ContentPart::Image { .. }]
    ));
}

fn runtime_note_entry(sequence: u64, note: RuntimeNote) -> MessageLogEntry {
    let mut message = note.into_new_conversation_message().unwrap();
    MessageLogEntry {
        message_id: message.message_id,
        sequence,
        turn_id: "turn-1".to_string(),
        role: message.role,
        created_at: message.created_at,
        status: message.status,
        content: message.content,
        provider: message.provider,
        tool: message.tool,
        metadata: std::mem::take(&mut message.metadata),
    }
}

#[test]
fn task_runtime_notes_project_as_tool_protocol_instead_of_system_messages() {
    let watch = runtime_note_entry(
        1,
        RuntimeNote {
            runtime_note_id: "watch-1".to_string(),
            created_at: "2026-08-26T00:00:00Z".to_string(),
            payload: RuntimeNotePayload::TaskWatchOutput(TaskWatchOutputRuntimeNote {
                task_id: "task-1".to_string(),
                attempt_no: 1,
                projection: TaskWatchOutputProjection::ModelVisible,
                records: vec![TaskWatchOutputRecord {
                    source: "stdout".to_string(),
                    text: "line one".to_string(),
                }],
            }),
        },
    );
    let terminal = runtime_note_entry(
        2,
        RuntimeNote {
            runtime_note_id: "terminal-1".to_string(),
            created_at: "2026-08-26T00:00:01Z".to_string(),
            payload: RuntimeNotePayload::TaskAttemptTerminal(TaskAttemptTerminalRuntimeNote {
                task_id: "task-1".to_string(),
                attempt_no: 1,
                terminal_status: "completed".to_string(),
                output: "Outcome\nDone".to_string(),
                runner_name: "bash".to_string(),
                tool_call_id: None,
                full_result_ref: None,
                artifacts: serde_json::json!([]),
                device: serde_json::json!({}),
            }),
        },
    );

    let projected = project_message_log_entries_for_model(&[watch, terminal]);

    assert_eq!(
        projected
            .iter()
            .map(|message| message.role)
            .collect::<Vec<_>>(),
        [Role::Assistant, Role::Tool, Role::Assistant, Role::Tool]
    );
    assert!(projected.iter().all(|message| message.role != Role::System));
    assert!(projected[0].has_tool_calls());
    assert_eq!(
        projected[1].tool_results()[0].text_content(),
        "<task_watch_output>\ntask_id: task-1\nattempt_no: 1\nrecords:\n[stdout] line one\n</task_watch_output>"
    );
    assert!(projected[2].has_tool_calls());
    let terminal: serde_json::Value =
        serde_json::from_str(&projected[3].tool_results()[0].text_content()).unwrap();
    assert!(terminal.get("status").is_some());
    assert_eq!(terminal["read"]["task_id"], "task-1");
}

#[test]
fn collaboration_result_projects_as_stable_tool_protocol_without_user_message() {
    let entry = runtime_note_entry(
        1,
        RuntimeNote {
            runtime_note_id: "collaboration-result:result-1".to_string(),
            created_at: "2026-08-26T00:00:00Z".to_string(),
            payload: RuntimeNotePayload::CollaborationResult(CollaborationResultRuntimeNote {
                request_id: "request-1".to_string(),
                result_id: "result-1".to_string(),
                target: rua_core::ChatAddress::agent("reviewer", "child"),
                status: "completed".to_string(),
                output: format!("review complete{}", "中文结果".repeat(4000)),
                partial: false,
                results: Vec::new(),
            }),
        },
    );

    let first = project_message_log_entries_for_model(std::slice::from_ref(&entry));
    let second = project_message_log_entries_for_model(&[entry]);

    assert_eq!(
        first.iter().map(|message| message.role).collect::<Vec<_>>(),
        [Role::Assistant, Role::Tool]
    );
    let call = &first[0].tool_calls()[0];
    let result = &first[1].tool_results()[0];
    assert_eq!(call.name, "Chat_Notification");
    assert_eq!(call.arguments["request_id"], "request-1");
    assert_eq!(result.name.as_deref(), Some("Chat_Notification"));
    assert_eq!(result.call_id, call.id);
    assert!(!result.is_error);
    assert!(result.text_content().contains("review complete"));
    let body: serde_json::Value = serde_json::from_str(&result.text_content()).unwrap();
    assert_eq!(body["status"], "completed");
    assert_eq!(body["partial"], false);
    assert_eq!(body["has_more"], true);
    assert!(body["output"].as_str().unwrap().len() <= 8000);
    assert_eq!(body["next"]["request_id"], "request-1");
    assert!(
        react_agent::estimate_model_visible_conversation_tokens(&first, 4.0) <= 10_000,
        "the complete notification tool-use/result pair must fit the default model budget"
    );
    assert!(body.get("request_id").is_none());
    assert_eq!(call.arguments["result_id"], "result-1");
    assert_eq!(second[0].tool_calls()[0].id, call.id);
    assert!(first.iter().all(|message| message.role != Role::User));
}

#[test]
fn task_notification_does_not_repeat_previously_read_terminal_preview() {
    let mut result = ToolResult::text("read-task", "terminal output");
    result.name = Some("task_read".into());
    result.metadata.insert(
        "task_terminal_observation".into(),
        serde_json::json!({"task_id":"task-1","attempt_no":1}),
    );
    let read = log_message(
        1,
        "read-result",
        ModelDialogueItem {
            content: vec![ContentPart::ToolResult { result }],
            ..ModelDialogueItem::text(Role::Tool, "")
        },
    );
    let note = runtime_note_entry(
        2,
        RuntimeNote {
            runtime_note_id: "task:task-1:attempt:1".into(),
            created_at: "2026-08-26T00:00:01Z".into(),
            payload: RuntimeNotePayload::TaskAttemptTerminal(TaskAttemptTerminalRuntimeNote {
                task_id: "task-1".into(),
                attempt_no: 1,
                terminal_status: "completed".into(),
                output: "terminal output".into(),
                runner_name: "fixture".into(),
                tool_call_id: None,
                full_result_ref: None,
                artifacts: serde_json::json!([]),
                device: serde_json::json!({}),
            }),
        },
    );
    let projected =
        project_message_log_entries_for_model(&[read.clone(), note.clone(), note.clone()]);
    let notifications = projected
        .iter()
        .flat_map(|message| message.tool_results())
        .filter(|result| result.name.as_deref() == Some("Task_Notification"))
        .collect::<Vec<_>>();
    assert_eq!(notifications.len(), 1);
    let body: serde_json::Value = serde_json::from_str(&notifications[0].text_content()).unwrap();
    assert_eq!(body["output_already_observed"], true);
    assert!(body.get("preview").is_none());
    let restored: Vec<MessageLogEntry> =
        serde_json::from_slice(&serde_json::to_vec(&vec![read, note.clone()]).unwrap()).unwrap();
    let replay = project_message_log_entries_for_model(&restored);
    let result = replay
        .iter()
        .flat_map(|m| m.tool_results())
        .find(|r| r.name.as_deref() == Some("Task_Notification"))
        .unwrap();
    assert_eq!(result.text_content(), notifications[0].text_content());
    let fresh = project_message_log_entries_for_model(&[note]);
    assert!(fresh
        .iter()
        .flat_map(|m| m.tool_results())
        .any(|r| r.text_content().contains("terminal output")));
}

#[test]
fn collaboration_result_pages_validate_utf8_offsets() {
    use rua_session::collaboration::collaboration_result_page;
    let text = "中".repeat(6000);
    assert!(collaboration_result_page("r", "v", "completed", &text, false, 1).is_err());
    assert!(
        collaboration_result_page("r", "v", "completed", &text, false, text.len() + 1).is_err()
    );
    let mut offset = 0;
    let mut restored = String::new();
    loop {
        let page = collaboration_result_page("r", "v", "completed", &text, false, offset).unwrap();
        restored.push_str(page["output"].as_str().unwrap());
        if page["has_more"] == false {
            break;
        }
        offset = page["next"]["offset"].as_u64().unwrap() as usize;
    }
    assert_eq!(restored, text);
}

#[test]
fn legacy_turn_aborted_marker_projects_as_user_without_reordering() {
    let mut marker = log_message(
        2,
        "turn-aborted-turn-1",
        ModelDialogueItem::text(
            Role::System,
            "<turn_aborted>\nThe user interrupted the previous turn on purpose.\n</turn_aborted>",
        ),
    );
    marker.metadata.insert(
        "model_visible_marker".to_string(),
        serde_json::json!("turn_aborted"),
    );
    let entries = vec![
        log_message(
            1,
            "user-before-abort",
            ModelDialogueItem::text(Role::User, "start work"),
        ),
        marker,
        log_message(
            3,
            "user-after-abort",
            ModelDialogueItem::text(Role::User, "continue with the next request"),
        ),
    ];

    let projected = project_message_log_entries_for_model(&entries);

    assert_eq!(
        projected
            .iter()
            .map(|message| (message.id.as_deref(), message.role))
            .collect::<Vec<_>>(),
        vec![
            (Some("user-before-abort"), Role::User),
            (Some("turn-aborted-turn-1"), Role::User),
            (Some("user-after-abort"), Role::User),
        ]
    );
}

#[test]
fn task_runtime_note_projection_uses_provider_safe_call_ids() {
    let runtime_note_id = "task-watch:08fd38b0-75e0-4096-8f77-8128e0f8872d:attempt:1:batch:333";
    let entry = runtime_note_entry(
        1,
        RuntimeNote {
            runtime_note_id: runtime_note_id.to_string(),
            created_at: "2026-08-28T00:00:00Z".to_string(),
            payload: RuntimeNotePayload::TaskWatchOutput(TaskWatchOutputRuntimeNote {
                task_id: "08fd38b0-75e0-4096-8f77-8128e0f8872d".to_string(),
                attempt_no: 1,
                projection: TaskWatchOutputProjection::ModelVisible,
                records: vec![TaskWatchOutputRecord {
                    source: "stderr".to_string(),
                    text: "progress".to_string(),
                }],
            }),
        },
    );

    let first = project_message_log_entries_for_model(std::slice::from_ref(&entry));
    let second = project_message_log_entries_for_model(&[entry]);
    let ContentPart::ToolUse { call } = &first[0].content[0] else {
        panic!("expected synthetic tool use");
    };
    let result = &first[1].tool_results()[0];

    assert!(call.id.len() <= 64, "call id was {} bytes", call.id.len());
    assert_eq!(result.call_id, call.id);
    assert_eq!(
        second[0].tool_calls()[0].id,
        call.id,
        "projection-owned IDs must be stable"
    );
}

#[test]
fn task_watch_batch_uses_single_tool_output_token_limit_without_secondary_truncation() {
    let watch = runtime_note_entry(
        1,
        RuntimeNote {
            runtime_note_id: "watch-large".to_string(),
            created_at: "2026-08-26T00:00:00Z".to_string(),
            payload: RuntimeNotePayload::TaskWatchOutput(TaskWatchOutputRuntimeNote {
                task_id: "task-1".to_string(),
                attempt_no: 1,
                projection: TaskWatchOutputProjection::ModelVisible,
                records: vec![TaskWatchOutputRecord {
                    source: "stdout".to_string(),
                    text: "0123456789".repeat(1_000),
                }],
            }),
        },
    );
    let messages = project_message_log_entries_for_model(&[watch]);
    let mut running = rua_config::AgentsRunningConfig::default();
    running.tool_result_compact.projection_truncation_enabled = false;
    running.tool_result_compact.execution_output_max_tokens = 128;
    running.context_compact.token_count_estimate_divisor = 4.0;

    let projected = project_tool_results_for_model(&messages, &running);

    let result = &projected[1].tool_results()[0];
    assert_eq!(result.name.as_deref(), Some("Task_Notification"));
    assert!(result.truncation.is_some());
    assert!(rua_tool::result_store::estimate_text_tokens(&result.text_content(), 4.0) <= 128);
}

#[test]
fn delegated_task_watch_activity_is_not_model_visible() {
    let entry = runtime_note_entry(
        1,
        RuntimeNote {
            runtime_note_id: "watch-delegated".to_string(),
            created_at: "2026-08-26T00:00:00Z".to_string(),
            payload: RuntimeNotePayload::TaskWatchOutput(TaskWatchOutputRuntimeNote {
                task_id: "task-1".to_string(),
                attempt_no: 1,
                projection: TaskWatchOutputProjection::ActivityOnly,
                records: vec![TaskWatchOutputRecord {
                    source: "acp".to_string(),
                    text: "internal execution transcript".to_string(),
                }],
            }),
        },
    );

    assert!(project_message_log_entries_for_model(&[entry]).is_empty());
}

#[test]
fn runtime_notifications_follow_the_complete_parallel_tool_group() {
    let [mut call_a, mut result_a] = tool_round("await-1", "actual completed result");
    let [call_b, mut result_b] = tool_round("get-2", "actual failed result");
    call_a.content.extend(call_b.content);
    if let ContentPart::ToolResult { result } = &mut result_b.content[0] {
        result.is_error = true;
        result
            .metadata
            .insert("evidence".into(), serde_json::json!("keep"));
    }
    result_a
        .metadata
        .insert("result_origin".into(), serde_json::json!("durable"));
    let terminal = |sequence, id: &str| {
        runtime_note_entry(
            sequence,
            RuntimeNote {
                runtime_note_id: id.to_string(),
                created_at: "2026-09-21T00:00:00Z".into(),
                payload: RuntimeNotePayload::TaskAttemptTerminal(TaskAttemptTerminalRuntimeNote {
                    task_id: "task-1".into(),
                    attempt_no: 1,
                    terminal_status: "completed".into(),
                    output: "async outcome".into(),
                    runner_name: "bash".into(),
                    tool_call_id: None,
                    full_result_ref: None,
                    artifacts: serde_json::json!([]),
                    device: serde_json::json!({}),
                }),
            },
        )
    };
    let entries = vec![
        log_message(1, "calls", call_a),
        terminal(2, "note-1"),
        log_message(3, "result-b", result_b.clone()),
        terminal(4, "note-2"),
        log_message(5, "result-a", result_a.clone()),
    ];
    let before = serde_json::to_value(&entries).unwrap();
    let projected = project_message_log_entries_for_model(&entries);
    assert_eq!(projected.len(), 7);
    assert_eq!(projected[1].tool_results()[0], result_a.tool_results()[0]);
    assert_eq!(projected[2].tool_results()[0], result_b.tool_results()[0]);
    assert_eq!(projected[1].metadata["result_origin"], "durable");
    assert_eq!(projected[1].id.as_deref(), Some("result-a"));
    assert_eq!(projected[3].tool_calls()[0].name, "Task_Notification");
    assert_eq!(projected[5].tool_calls()[0].name, "Task_Notification");
    let provider =
        llm_multiplex::OpenAiResponsesProvider::new(llm_multiplex::OpenAiCompatibleConfig {
            provider_id: "responses".into(),
            provider_name: "Responses".into(),
            base_url: "http://127.0.0.1:9/v1".into(),
            api_key: None,
            api_keys: vec![],
            headers: Default::default(),
            models: vec![],
            timeout_secs: 1,
            generate_kwargs: Default::default(),
            support_model_discovery: false,
            support_connection_check: false,
            is_local: true,
        })
        .unwrap();
    let wire = provider.build_request_body(&agent_types::ChatRequest::new(projected));
    let outputs: Vec<_> = wire["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "function_call_output")
        .collect();
    assert_eq!(outputs.len(), 4);
    assert_eq!(outputs[0]["output"], "actual completed result");
    assert_eq!(outputs[1]["output"], "actual failed result");
    assert!(!wire.to_string().contains("interrupted"));
    assert_eq!(serde_json::to_value(entries).unwrap(), before);
}
