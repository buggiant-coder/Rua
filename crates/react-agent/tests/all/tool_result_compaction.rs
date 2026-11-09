use agent_types::{ContentPart, ModelDialogueItem, Role, ToolCall, ToolResult};

use react_agent::snip::snip_old_tool_results;
use react_agent::tool_result_compressor::{
    compact_projected_tool_results, preprocess_tool_results,
};

fn assistant_with_tool_use(
    call_id: &str,
    name: &str,
    args: serde_json::Value,
) -> ModelDialogueItem {
    ModelDialogueItem {
        id: None,
        role: Role::Assistant,
        content: vec![ContentPart::ToolUse {
            call: ToolCall {
                id: call_id.to_string(),
                name: name.to_string(),
                arguments: args,
                raw_arguments: None,
                provider_call_id: None,
                metadata: serde_json::Map::new(),
            },
        }],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    }
}

fn tool_result_msg(call_id: &str, name: &str, output: &str) -> ModelDialogueItem {
    let mut result = ToolResult::text(call_id, output);
    result.name = Some(name.to_string());
    ModelDialogueItem {
        id: None,
        role: Role::Tool,
        content: vec![ContentPart::ToolResult { result }],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    }
}

fn user_text(text: &str) -> ModelDialogueItem {
    ModelDialogueItem::text(Role::User, text.to_string())
}

fn build_round(
    tool_use_id: &str,
    tool_name: &str,
    args: serde_json::Value,
    output: &str,
) -> Vec<ModelDialogueItem> {
    vec![
        assistant_with_tool_use(tool_use_id, tool_name, args),
        tool_result_msg(tool_use_id, tool_name, output),
    ]
}

fn output_text(msg: &ModelDialogueItem) -> String {
    msg.content
        .iter()
        .filter_map(|p| match p {
            ContentPart::ToolResult { result } => Some(
                result
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentPart::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(""),
            ),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

#[test]
fn preprocess_keeps_recent_rounds_intact() {
    let mut messages = Vec::new();
    messages.push(user_text("ping"));

    let long = "line\n".repeat(2_000);
    messages.extend(build_round(
        "tu1",
        "bash",
        serde_json::json!({"command": "echo 1"}),
        &long,
    ));
    messages.extend(build_round(
        "tu2",
        "bash",
        serde_json::json!({"command": "echo 2"}),
        &long,
    ));
    messages.extend(build_round(
        "tu3",
        "bash",
        serde_json::json!({"command": "echo 3"}),
        &long,
    ));

    let compressed = preprocess_tool_results(&mut messages, 2, 3_000);
    assert_eq!(compressed, 1, "only the oldest round should be touched");

    let round1_result = output_text(&messages[2]);
    assert!(
        round1_result.len() < long.len(),
        "tu1 result should be compressed"
    );
    assert!(
        output_text(&messages[4]).len() == long.len(),
        "tu2 (recent) should be untouched"
    );
    assert!(
        output_text(&messages[6]).len() == long.len(),
        "tu3 (recent) should be untouched"
    );
}

#[test]
fn preprocess_read_strategy_keeps_head_and_tail() {
    let mut output = String::new();
    for i in 1..=200 {
        output.push_str(&format!("line {i}\n"));
    }
    let mut messages = vec![user_text("p")];
    messages.extend(build_round(
        "tu_old",
        "read",
        serde_json::json!({"path": "/x"}),
        &output,
    ));

    messages.extend(build_round(
        "tu_recent",
        "read",
        serde_json::json!({"path": "/y"}),
        "short",
    ));

    preprocess_tool_results(&mut messages, 1, 500);
    let compressed = output_text(&messages[2]);
    assert!(compressed.contains("line 1\n"), "head retained");
    assert!(compressed.contains("line 200"), "tail retained");
    assert!(
        compressed.contains("lines truncated"),
        "should mention truncation; got: {compressed}"
    );
}

#[test]
fn preprocess_bash_strategy_keeps_tail() {
    let mut output = String::new();
    for i in 1..=100 {
        output.push_str(&format!("step {i}\n"));
    }
    let mut messages = vec![user_text("p")];
    messages.extend(build_round(
        "tu_old",
        "bash",
        serde_json::json!({"command": "yes"}),
        &output,
    ));
    messages.extend(build_round(
        "tu_recent",
        "bash",
        serde_json::json!({"command": "ok"}),
        "short",
    ));

    preprocess_tool_results(&mut messages, 1, 500);
    let compressed = output_text(&messages[2]);
    assert!(compressed.contains("step 100"), "last line kept");
    assert!(
        !compressed.contains("step 1\n"),
        "head dropped; got: {compressed}"
    );
}

#[test]
fn preprocess_edit_strategy_keeps_head_5() {
    let mut output = String::new();
    for i in 1..=50 {
        output.push_str(&format!("L{i}\n"));
    }
    let mut messages = vec![user_text("p")];
    messages.extend(build_round(
        "tu_old",
        "edit",
        serde_json::json!({"file": "f"}),
        &output,
    ));
    messages.extend(build_round(
        "tu_recent",
        "edit",
        serde_json::json!({"file": "g"}),
        "ok",
    ));

    preprocess_tool_results(&mut messages, 1, 50);
    let compressed = output_text(&messages[2]);
    assert!(
        compressed.contains("L1\n") && compressed.contains("L5"),
        "head 5 retained"
    );
    assert!(compressed.contains("lines truncated"));
}

#[test]
fn preprocess_skipped_when_under_threshold() {
    let mut messages = vec![user_text("p")];
    messages.extend(build_round(
        "tu_old",
        "bash",
        serde_json::json!({}),
        "small output",
    ));
    messages.extend(build_round(
        "tu_recent",
        "bash",
        serde_json::json!({}),
        "tiny",
    ));

    let compressed = preprocess_tool_results(&mut messages, 1, 10_000);
    assert_eq!(compressed, 0, "nothing exceeds 10KB");
}

#[test]
fn projection_compaction_applies_second_truncation_to_all_older_rounds() {
    let mut messages = vec![user_text("inspect")];
    for index in 1..=6 {
        messages.extend(build_round(
            &format!("tu{index}"),
            "bash",
            serde_json::json!({"command": format!("echo {index}")}),
            &format!("round-{index}-{}", "x".repeat(200)),
        ));
    }

    let truncated = compact_projected_tool_results(&mut messages, 2, 32, 1);

    assert_eq!(truncated, 4);
    for result_index in [2, 4, 6, 8] {
        let result = messages[result_index].tool_results()[0];
        assert!(
            !result.text_content().is_empty(),
            "older results must retain truncated content"
        );
        assert!(
            result.text_content().len() <= 32,
            "older results must honor the configured byte cap"
        );
    }
    assert!(messages[10].tool_results()[0].text_content().len() > 32);
    assert!(messages[12].tool_results()[0].text_content().len() > 32);
}

#[test]
fn preprocess_handles_multi_tool_message_with_partial_pairing() {
    let long = "completed\n".repeat(1_000);
    let mut messages = vec![
        user_text("run two tools"),
        ModelDialogueItem {
            id: None,
            role: Role::Assistant,
            content: vec![
                ContentPart::ToolUse {
                    call: ToolCall {
                        id: "toolu_done".to_string(),
                        name: "bash".to_string(),
                        arguments: serde_json::json!({"command": "done"}),
                        raw_arguments: None,
                        provider_call_id: None,
                        metadata: serde_json::Map::new(),
                    },
                },
                ContentPart::ToolUse {
                    call: ToolCall {
                        id: "toolu_pending".to_string(),
                        name: "read".to_string(),
                        arguments: serde_json::json!({"path": "/tmp/pending"}),
                        raw_arguments: None,
                        provider_call_id: None,
                        metadata: serde_json::Map::new(),
                    },
                },
            ],
            name: None,
            created_at: None,
            metadata: serde_json::Map::new(),
        },
        tool_result_msg("toolu_done", "bash", &long),
    ];

    let compressed = preprocess_tool_results(&mut messages, 1, 120);

    assert_eq!(
        compressed, 1,
        "only the completed round should be compressed; pending tool_use remains in flight"
    );
    assert!(
        output_text(&messages[2]).contains("completed"),
        "completed result remains a ToolResult text payload"
    );
    assert!(
        output_text(&messages[2]).len() < long.len(),
        "completed result should be byte-trimmed"
    );

    let assistant = &messages[1];
    let ContentPart::ToolUse { call: done } = &assistant.content[0] else {
        panic!("first block should remain ToolUse");
    };
    assert_eq!(done.id, "toolu_done");
    assert_eq!(
        done.arguments,
        serde_json::json!({"command": "done"}),
        "tier-1 compression never rewrites tool_use arguments"
    );

    let ContentPart::ToolUse { call: pending } = &assistant.content[1] else {
        panic!("second block should remain ToolUse");
    };
    assert_eq!(pending.id, "toolu_pending");
    assert_eq!(
        pending.arguments,
        serde_json::json!({"path": "/tmp/pending"}),
        "pending tool_use must not be altered by completed-round preprocessing"
    );
}

#[test]
fn snip_collapses_old_rounds_to_summaries_with_recall_key() {
    let long = "x".repeat(5_000);
    let mut messages = vec![user_text("hi")];
    messages.extend(build_round(
        "toolu_a",
        "bash",
        serde_json::json!({"command": "ls"}),
        &long,
    ));
    messages.extend(build_round(
        "toolu_b",
        "bash",
        serde_json::json!({"command": "pwd"}),
        "ok",
    ));
    messages.extend(build_round(
        "toolu_c",
        "bash",
        serde_json::json!({"command": "id"}),
        "uid=0",
    ));

    let rewritten = snip_old_tool_results(&mut messages, 2, 200);
    assert_eq!(
        rewritten, 1,
        "only the oldest round is older than keep_recent_n=2"
    );

    let tu_a_result = output_text(&messages[2]);
    assert!(
        tu_a_result.len() < long.len(),
        "snipped result is smaller than the original"
    );
    let expected_key = rua_tool::result_store::recall_key("toolu_a");
    assert!(
        tu_a_result.contains(&format!("recall_tool_result(recall_key=\"{expected_key}\")")),
        "recall hint must be an exact recall_tool_result call with recall_key(toolu_a); got: {tu_a_result}"
    );

    let assistant_a = &messages[1];
    let snipped_args = match &assistant_a.content[0] {
        ContentPart::ToolUse { call } => call.arguments.clone(),
        _ => panic!("expected ToolUse"),
    };
    assert!(
        snipped_args.get("_snipped").is_some(),
        "tool_use args replaced by _snipped marker; got: {snipped_args}"
    );
}

#[test]
fn snip_idempotent_on_already_snipped() {
    let long = "y".repeat(2_000);
    let mut messages = vec![user_text("hi")];
    messages.extend(build_round(
        "tu_a",
        "bash",
        serde_json::json!({"command": "x"}),
        &long,
    ));
    messages.extend(build_round(
        "tu_b",
        "bash",
        serde_json::json!({"command": "y"}),
        "ok",
    ));
    messages.extend(build_round(
        "tu_c",
        "bash",
        serde_json::json!({"command": "z"}),
        "ok",
    ));

    let first = snip_old_tool_results(&mut messages, 2, 200);
    let second = snip_old_tool_results(&mut messages, 2, 200);
    assert_eq!(first, 1);
    assert_eq!(
        second, 0,
        "second pass should detect _snipped marker and skip"
    );
}

#[test]
fn snip_leaves_recent_rounds_intact() {
    let mut messages = vec![user_text("hi")];
    messages.extend(build_round(
        "tu_a",
        "bash",
        serde_json::json!({"command": "x"}),
        "out a",
    ));
    messages.extend(build_round(
        "tu_b",
        "bash",
        serde_json::json!({"command": "y"}),
        "out b",
    ));

    snip_old_tool_results(&mut messages, 5, 200);
    assert!(
        output_text(&messages[2]).contains("out a"),
        "recent rounds untouched"
    );
    assert!(output_text(&messages[4]).contains("out b"));
}

#[test]
fn snip_preserves_block_types() {
    let long = "z".repeat(2_000);
    let mut messages = vec![user_text("hi")];
    messages.extend(build_round(
        "tu_a",
        "bash",
        serde_json::json!({"command": "x"}),
        &long,
    ));
    messages.extend(build_round(
        "tu_b",
        "bash",
        serde_json::json!({"command": "y"}),
        "ok",
    ));
    messages.extend(build_round(
        "tu_c",
        "bash",
        serde_json::json!({"command": "z"}),
        "ok",
    ));

    snip_old_tool_results(&mut messages, 2, 200);

    assert!(
        matches!(messages[1].content[0], ContentPart::ToolUse { .. }),
        "tool_use block type preserved"
    );
    assert!(
        matches!(messages[2].content[0], ContentPart::ToolResult { .. }),
        "tool_result block type preserved"
    );
}

#[test]
fn snip_handles_multiple_tool_uses_in_one_assistant_message() {
    let long_a = "alpha\n".repeat(1_000);
    let long_b = "beta\n".repeat(1_000);
    let mut messages = vec![
        user_text("run both tools"),
        ModelDialogueItem {
            id: None,
            role: Role::Assistant,
            content: vec![
                ContentPart::ToolUse {
                    call: ToolCall {
                        id: "toolu_a".to_string(),
                        name: "bash".to_string(),
                        arguments: serde_json::json!({"command": "alpha"}),
                        raw_arguments: None,
                        provider_call_id: None,
                        metadata: serde_json::Map::new(),
                    },
                },
                ContentPart::ToolUse {
                    call: ToolCall {
                        id: "toolu_b".to_string(),
                        name: "read".to_string(),
                        arguments: serde_json::json!({"path": "/tmp/b"}),
                        raw_arguments: None,
                        provider_call_id: None,
                        metadata: serde_json::Map::new(),
                    },
                },
            ],
            name: None,
            created_at: None,
            metadata: serde_json::Map::new(),
        },
        tool_result_msg("toolu_a", "bash", &long_a),
        tool_result_msg("toolu_b", "read", &long_b),
    ];

    let rewritten = snip_old_tool_results(&mut messages, 0, 120);

    assert_eq!(rewritten, 2, "both tool rounds should be rewritten");

    let assistant = &messages[1];
    let mut tool_use_ids = Vec::new();
    for block in &assistant.content {
        let ContentPart::ToolUse { call } = block else {
            panic!("assistant block should remain a ToolUse");
        };
        assert!(
            call.arguments.get("_snipped").is_some(),
            "tool_use args should carry snipped marker for {}",
            call.id
        );
        tool_use_ids.push(call.id.as_str());
    }
    assert_eq!(tool_use_ids, vec!["toolu_a", "toolu_b"]);

    let mut result_ids = Vec::new();
    for msg in &messages[2..] {
        let ContentPart::ToolResult { result } = &msg.content[0] else {
            panic!("tool result block type should be preserved");
        };
        let text = output_text(msg);
        assert!(
            text.contains("recall_tool_result(recall_key=\""),
            "snipped result should include an exact recall call; got: {text}"
        );
        result_ids.push(result.call_id.as_str());
    }
    assert_eq!(result_ids, tool_use_ids);
}

#[test]
fn snip_skips_pending_tool_use_without_rewriting_arguments() {
    let mut messages = vec![
        user_text("start tool"),
        assistant_with_tool_use(
            "toolu_pending",
            "bash",
            serde_json::json!({"command": "long-running"}),
        ),
        user_text("next turn arrives without result"),
    ];

    let rewritten = snip_old_tool_results(&mut messages, 0, 120);

    assert_eq!(
        rewritten, 0,
        "pending tool_use without a matching ToolResult is not a completed round"
    );
    let ContentPart::ToolUse { call } = &messages[1].content[0] else {
        panic!("assistant block should remain ToolUse");
    };
    assert_eq!(
        call.arguments,
        serde_json::json!({"command": "long-running"})
    );
    assert!(
        call.arguments.get("_snipped").is_none(),
        "in-flight tool_use args must not be replaced by a snipped marker"
    );
}

#[test]
fn snip_ignores_orphan_tool_result_without_matching_tool_use() {
    let orphan = tool_result_msg("toolu_orphan", "bash", &"orphan\n".repeat(1_000));
    let mut messages = vec![user_text("orphan result"), orphan.clone()];

    let rewritten = snip_old_tool_results(&mut messages, 0, 120);

    assert_eq!(rewritten, 0);
    assert_eq!(
        output_text(&messages[1]),
        output_text(&orphan),
        "orphan ToolResult should pass through unchanged"
    );
}

#[test]
fn snip_handles_multi_tool_message_with_partial_pairing() {
    let mut messages = vec![
        user_text("run two tools"),
        ModelDialogueItem {
            id: None,
            role: Role::Assistant,
            content: vec![
                ContentPart::ToolUse {
                    call: ToolCall {
                        id: "toolu_done".to_string(),
                        name: "bash".to_string(),
                        arguments: serde_json::json!({"command": "done"}),
                        raw_arguments: None,
                        provider_call_id: None,
                        metadata: serde_json::Map::new(),
                    },
                },
                ContentPart::ToolUse {
                    call: ToolCall {
                        id: "toolu_pending".to_string(),
                        name: "read".to_string(),
                        arguments: serde_json::json!({"path": "/tmp/pending"}),
                        raw_arguments: None,
                        provider_call_id: None,
                        metadata: serde_json::Map::new(),
                    },
                },
            ],
            name: None,
            created_at: None,
            metadata: serde_json::Map::new(),
        },
        tool_result_msg("toolu_done", "bash", &"done\n".repeat(1_000)),
    ];

    let rewritten = snip_old_tool_results(&mut messages, 0, 120);

    assert_eq!(rewritten, 1, "only the completed tool round is snipped");
    let assistant = &messages[1];
    let ContentPart::ToolUse { call: done } = &assistant.content[0] else {
        panic!("first block should remain ToolUse");
    };
    assert!(
        done.arguments.get("_snipped").is_some(),
        "completed round should be snipped"
    );

    let ContentPart::ToolUse { call: pending } = &assistant.content[1] else {
        panic!("second block should remain ToolUse");
    };
    assert_eq!(
        pending.arguments,
        serde_json::json!({"path": "/tmp/pending"}),
        "pending tool_use keeps original args"
    );

    let result_text = output_text(&messages[2]);
    let expected_key = rua_tool::result_store::recall_key("toolu_done");
    assert!(result_text.contains(&format!(
        "recall_tool_result(recall_key=\"{expected_key}\")"
    )));
}
