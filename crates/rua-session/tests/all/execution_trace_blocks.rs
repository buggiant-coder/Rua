use rua_session::{detect_execution_trace_blocks, MessageLogEntry};
use serde_json::{json, Value};

fn entry(sequence: u64, message_id: &str, role: &str, content: Value) -> MessageLogEntry {
    MessageLogEntry {
        message_id: message_id.to_string(),
        sequence,
        turn_id: "turn-1".to_string(),
        role: role.to_string(),
        created_at: "2026-07-26T00:00:00Z".to_string(),
        status: "completed".to_string(),
        content,
        provider: None,
        tool: None,
        metadata: serde_json::Map::new(),
    }
}

fn tool_use(call_id: &str) -> Value {
    json!({
        "type": "tool_use",
        "id": call_id,
        "name": "read",
        "input": {"path": format!("{call_id}.txt")}
    })
}

fn tool_result(call_id: &str, output: &str) -> Value {
    json!({
        "type": "tool_result",
        "id": call_id,
        "name": "read",
        "output": [{"type": "text", "text": output}],
        "metadata": {
            "tool_output": {
                "archive_key": format!("recall-{call_id}")
            }
        }
    })
}

#[test]
fn detector_emits_multiple_blocks_with_distinct_anchors_in_one_message() {
    let messages = vec![
        entry(
            1,
            "user-1",
            "user",
            json!([{"type": "text", "text": "Inspect both files"}]),
        ),
        entry(
            2,
            "assistant-1",
            "assistant",
            json!([
                {"type": "thinking", "thinking": "inspect alpha"},
                tool_use("call-a"),
                tool_result("call-a", "alpha result"),
                {"type": "text", "text": "Alpha is configured."},
                {"type": "thinking", "thinking": "inspect beta"},
                tool_use("call-b"),
                tool_result("call-b", "beta result"),
                {"type": "text", "text": "Beta is configured."}
            ]),
        ),
    ];

    let blocks = detect_execution_trace_blocks(
        "chat-1",
        "turn-1",
        &messages,
        "lineage-1".to_string(),
        4.0,
        false,
    );
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0].source.start_anchor.content_part_index, 0);
    assert_eq!(blocks[0].source.end_anchor.content_part_index, 2);
    assert_eq!(blocks[1].source.start_anchor.content_part_index, 3);
    assert_eq!(blocks[1].source.end_anchor.content_part_index, 6);
    assert_ne!(blocks[0].record_id, blocks[1].record_id);
    assert!(!blocks[0].source_content.contains("Alpha is configured."));
    assert!(blocks[1].source_content.contains("Alpha is configured."));
    assert!(!blocks[1].source_content.contains("Beta is configured."));
    assert_eq!(blocks[0].source.tool_references.len(), 1);
    assert_eq!(blocks[0].source.tool_references[0].tool_call_id, "call-a");
    assert_eq!(
        blocks[0].source.tool_references[0].recall_key.as_deref(),
        Some("recall-call-a")
    );
    assert_eq!(blocks[1].source.tool_references.len(), 1);
    assert_eq!(blocks[1].source.tool_references[0].tool_call_id, "call-b");

    let changed_content = vec![
        messages[0].clone(),
        entry(
            2,
            "assistant-1",
            "assistant",
            json!([
                {"type": "thinking", "thinking": "different reasoning"},
                tool_use("call-a"),
                tool_result("call-a", "different output"),
                {"type": "text", "text": "Different visible text."},
                {"type": "thinking", "thinking": "more different reasoning"},
                tool_use("call-b"),
                tool_result("call-b", "more different output"),
                {"type": "text", "text": "Another visible text."}
            ]),
        ),
    ];
    let same_identity = detect_execution_trace_blocks(
        "chat-1",
        "turn-1",
        &changed_content,
        "lineage-1".to_string(),
        4.0,
        false,
    );
    assert_eq!(
        blocks
            .iter()
            .map(|block| block.record_id.as_str())
            .collect::<Vec<_>>(),
        same_identity
            .iter()
            .map(|block| block.record_id.as_str())
            .collect::<Vec<_>>(),
        "source content must not participate in deterministic record identity"
    );
}

#[test]
fn detector_keeps_parallel_tool_calls_with_out_of_order_results_together() {
    let messages = vec![
        entry(
            1,
            "user-1",
            "user",
            json!([{"type": "text", "text": "Inspect in parallel"}]),
        ),
        entry(
            2,
            "assistant-tools",
            "assistant",
            json!([
                {"type": "thinking", "thinking": "read both"},
                tool_use("call-a"),
                tool_use("call-b")
            ]),
        ),
        entry(
            3,
            "result-b",
            "tool",
            json!([tool_result("call-b", "beta")]),
        ),
        entry(
            4,
            "result-a",
            "tool",
            json!([tool_result("call-a", "alpha")]),
        ),
        entry(
            5,
            "assistant-final",
            "assistant",
            json!([{"type": "text", "text": "Both reads completed."}]),
        ),
    ];

    let blocks = detect_execution_trace_blocks(
        "chat-1",
        "turn-1",
        &messages,
        "lineage-1".to_string(),
        4.0,
        false,
    );
    assert_eq!(blocks.len(), 1);
    assert_eq!(
        blocks[0].source.message_ids,
        vec!["assistant-tools", "result-b", "result-a"]
    );
    assert_eq!(blocks[0].source.sequence_range.start_sequence, 2);
    assert_eq!(blocks[0].source.sequence_range.end_sequence, 4);
}

#[test]
fn detector_requires_visible_text_or_terminal_variant_and_complete_results() {
    let complete_without_visible = vec![
        entry(
            1,
            "assistant-tools",
            "assistant",
            json!([tool_use("call-a")]),
        ),
        entry(
            2,
            "result-a",
            "tool",
            json!([tool_result("call-a", "done")]),
        ),
    ];
    assert!(detect_execution_trace_blocks(
        "chat-1",
        "turn-1",
        &complete_without_visible,
        "lineage-1".to_string(),
        4.0,
        false,
    )
    .is_empty());
    assert_eq!(
        detect_execution_trace_blocks(
            "chat-1",
            "turn-1",
            &complete_without_visible,
            "lineage-1".to_string(),
            4.0,
            true,
        )
        .len(),
        1,
        "terminal selected Variant closes its final complete process block"
    );

    let incomplete = vec![entry(
        1,
        "assistant-tools",
        "assistant",
        json!([tool_use("call-a")]),
    )];
    assert!(detect_execution_trace_blocks(
        "chat-1",
        "turn-1",
        &incomplete,
        "lineage-1".to_string(),
        4.0,
        true,
    )
    .is_empty());

    let mismatched = vec![
        entry(
            1,
            "assistant-tools",
            "assistant",
            json!([tool_use("call-a")]),
        ),
        entry(
            2,
            "result-b",
            "tool",
            json!([tool_result("call-b", "wrong result")]),
        ),
        entry(
            3,
            "assistant-final",
            "assistant",
            json!([{"type": "text", "text": "Done"}]),
        ),
    ];
    assert!(detect_execution_trace_blocks(
        "chat-1",
        "turn-1",
        &mismatched,
        "lineage-1".to_string(),
        4.0,
        true,
    )
    .is_empty());
}
