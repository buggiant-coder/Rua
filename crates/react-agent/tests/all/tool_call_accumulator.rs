use agent_types::AgentEvent;
use react_agent::ToolCallAccumulator;
use serde_json::json;

#[test]
fn tool_call_accumulator_merges_deltas_by_index_and_preserves_first_seen_order() {
    let mut accumulator = ToolCallAccumulator::new();

    accumulator.push_delta(1, "call_b", Some("read"), r#"{"path""#);
    accumulator.push_delta(0, "call_a", Some("bash"), r#"{"command":"ls"}"#);
    accumulator.push_delta(1, "", None, r#":"src/lib.rs"}"#);

    let calls: Vec<_> = accumulator
        .finalize()
        .into_iter()
        .map(Result::unwrap)
        .collect();

    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].id, "call_b");
    assert_eq!(calls[0].name, "read");
    assert_eq!(calls[0].arguments, json!({"path": "src/lib.rs"}));
    assert_eq!(
        calls[0].raw_arguments.as_deref(),
        Some(r#"{"path":"src/lib.rs"}"#)
    );

    assert_eq!(calls[1].id, "call_a");
    assert_eq!(calls[1].name, "bash");
    assert_eq!(calls[1].arguments, json!({"command": "ls"}));
}

#[test]
fn tool_call_accumulator_accepts_agent_events_and_ignores_non_tool_deltas() {
    let mut accumulator = ToolCallAccumulator::new();

    assert!(!accumulator.push_event(&AgentEvent::TextDelta {
        delta: "hello".into(),
        metadata: serde_json::Value::Null,
    }));
    assert!(accumulator.push_event(&AgentEvent::ToolCallDelta {
        id: "call_1".into(),
        index: 0,
        name: Some("Search".into()),
        arguments_delta: r#"{"query":"rua"}"#.into(),
        metadata: serde_json::Value::Null,
    }));

    let call = accumulator
        .finalize()
        .into_iter()
        .next()
        .expect("one tool call")
        .expect("valid call");

    assert_eq!(call.id, "call_1");
    assert_eq!(call.name, "Search");
    assert_eq!(call.arguments, json!({"query": "rua"}));
}

#[test]
fn tool_call_accumulator_parse_error_preserves_identity_and_raw_arguments() {
    let mut accumulator = ToolCallAccumulator::new();
    accumulator.push_delta(0, "call_bad", Some("bash"), "not json");

    let err = accumulator
        .finalize()
        .into_iter()
        .next()
        .expect("one result")
        .expect_err("unterminated JSON should fail strict finalization");

    assert_eq!(err.id, "call_bad");
    assert_eq!(err.tool_name, "bash");
    assert_eq!(err.raw_arguments, "not json");
    assert!(
        err.to_string()
            .contains("invalid tool arguments for `bash`"),
        "error should preserve tool name: {err}"
    );
}

#[test]
fn tool_call_accumulator_lenient_finalize_keeps_raw_arguments_on_bad_json() {
    let mut accumulator = ToolCallAccumulator::new();
    accumulator.push_delta(0, "call_bad", Some("bash"), "not json");

    let calls = accumulator.finalize_lenient();

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id, "call_bad");
    assert_eq!(calls[0].name, "bash");
    assert_eq!(calls[0].arguments, json!({}));
    assert_eq!(calls[0].raw_arguments.as_deref(), Some("not json"));
}

#[test]
fn tool_call_accumulator_uses_stable_default_id_when_provider_never_sends_one() {
    let mut accumulator = ToolCallAccumulator::new();
    accumulator.push_delta(2, "", Some("read"), r#"{"path":"src/lib.rs"}"#);

    let call = accumulator
        .finalize()
        .into_iter()
        .next()
        .expect("one tool call")
        .expect("valid call");

    assert_eq!(call.id, "tc-2");
    assert_eq!(call.name, "read");
    assert_eq!(call.arguments, json!({"path": "src/lib.rs"}));
}

#[test]
fn tool_call_accumulator_empty_followup_name_does_not_erase_first_name() {
    let mut accumulator = ToolCallAccumulator::new();
    accumulator.push_delta(0, "call_read", Some("read"), r#"{"path""#);
    accumulator.push_delta(0, "", Some(""), r#":"README.md"}"#);

    let call = accumulator
        .finalize()
        .into_iter()
        .next()
        .expect("one tool call")
        .expect("valid call");

    assert_eq!(call.id, "call_read");
    assert_eq!(call.name, "read");
    assert_eq!(call.arguments, json!({"path": "README.md"}));
}

#[test]
fn tool_call_accumulator_clear_resets_pending_state() {
    let mut accumulator = ToolCallAccumulator::new();
    accumulator.push_delta(0, "call_a", Some("bash"), r#"{"command":"pwd"}"#);
    accumulator.push_delta(1, "call_b", Some("read"), r#"{"path":"Cargo.toml"}"#);

    assert!(accumulator.has_pending());
    assert_eq!(accumulator.pending_count(), 2);

    accumulator.clear();

    assert!(!accumulator.has_pending());
    assert_eq!(accumulator.pending_count(), 0);
    assert!(accumulator.finalize().is_empty());
}
