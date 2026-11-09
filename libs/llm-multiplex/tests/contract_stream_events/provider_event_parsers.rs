use std::collections::HashMap;

use agent_types::{AgentEvent, FinishReason};
use llm_multiplex::event_parser::{parse_anthropic_event, parse_gemini_chunk, parse_openai_chunk};
use serde_json::json;

#[test]
fn openai_upstream_connection_termination_is_retryable() {
    let chunk = r#"{
        "error": {
            "message": "upstream connect error or disconnect/reset before headers. reset reason: connection termination"
        }
    }"#;
    let mut id_map = HashMap::new();

    let (events, finish) = parse_openai_chunk(chunk, &mut id_map).unwrap();

    assert!(finish.is_none());
    assert!(matches!(
        events.as_slice(),
        [AgentEvent::Error {
            retryable: true,
            ..
        }]
    ));
}

#[test]
fn openai_authentication_error_is_not_retryable() {
    let chunk = r#"{
        "error": {
            "type": "authentication_error",
            "message": "invalid API key"
        }
    }"#;
    let mut id_map = HashMap::new();

    let (events, _) = parse_openai_chunk(chunk, &mut id_map).unwrap();

    assert!(matches!(
        events.as_slice(),
        [AgentEvent::Error {
            retryable: false,
            ..
        }]
    ));
}

#[test]
fn openai_tool_call_deltas_are_tolerant_and_normalize_argument_fragments() {
    let chunk = r#"{
        "choices": [{
            "delta": {
                "tool_calls": [
                    {
                        "index": 0,
                        "id": "call_missing_function"
                    },
                    {
                        "index": 1,
                        "id": "call_object_args",
                        "function": {
                            "name": "ping",
                            "arguments": {"x": 2}
                        }
                    },
                    {
                        "index": 2,
                        "id": "call_null_args",
                        "function": {
                            "name": "noop",
                            "arguments": null
                        }
                    }
                ]
            }
        }]
    }"#;
    let mut id_map = HashMap::new();

    let (events, finish) = parse_openai_chunk(chunk, &mut id_map).unwrap();

    assert!(finish.is_none());
    assert_eq!(events.len(), 3);

    match &events[0] {
        AgentEvent::ToolCallDelta {
            id,
            index,
            name,
            arguments_delta,
            ..
        } => {
            assert_eq!(id, "call_missing_function");
            assert_eq!(*index, 0);
            assert_eq!(name, &None);
            assert_eq!(arguments_delta, "");
        }
        other => panic!("expected tolerant ToolCallDelta, got {other:?}"),
    }

    match &events[1] {
        AgentEvent::ToolCallDelta {
            id,
            index,
            name,
            arguments_delta,
            ..
        } => {
            assert_eq!(id, "call_object_args");
            assert_eq!(*index, 1);
            assert_eq!(name.as_deref(), Some("ping"));
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(arguments_delta).unwrap(),
                json!({"x": 2})
            );
        }
        other => panic!("expected object arguments ToolCallDelta, got {other:?}"),
    }

    match &events[2] {
        AgentEvent::ToolCallDelta {
            id,
            index,
            name,
            arguments_delta,
            ..
        } => {
            assert_eq!(id, "call_null_args");
            assert_eq!(*index, 2);
            assert_eq!(name.as_deref(), Some("noop"));
            assert_eq!(arguments_delta, "");
        }
        other => panic!("expected null-arguments ToolCallDelta, got {other:?}"),
    }
}

#[test]
fn openai_tool_call_delta_keeps_arguments_when_function_name_is_missing() {
    let chunk = r#"{
        "choices": [{
            "delta": {
                "tool_calls": [
                    {
                        "index": 3,
                        "id": "call_missing_name",
                        "function": {
                            "arguments": {"x": 3}
                        }
                    },
                    {
                        "index": 4,
                        "id": "call_array_args",
                        "function": {
                            "name": "batch",
                            "arguments": [1, 2]
                        }
                    }
                ]
            }
        }]
    }"#;
    let mut id_map = HashMap::new();

    let (events, finish) = parse_openai_chunk(chunk, &mut id_map).unwrap();

    assert!(finish.is_none());
    assert_eq!(events.len(), 2);

    match &events[0] {
        AgentEvent::ToolCallDelta {
            id,
            index,
            name,
            arguments_delta,
            ..
        } => {
            assert_eq!(id, "call_missing_name");
            assert_eq!(*index, 3);
            assert_eq!(name, &None);
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(arguments_delta).unwrap(),
                json!({"x": 3})
            );
        }
        other => panic!("expected missing-name ToolCallDelta, got {other:?}"),
    }

    match &events[1] {
        AgentEvent::ToolCallDelta {
            id,
            index,
            name,
            arguments_delta,
            ..
        } => {
            assert_eq!(id, "call_array_args");
            assert_eq!(*index, 4);
            assert_eq!(name.as_deref(), Some("batch"));
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(arguments_delta).unwrap(),
                json!([1, 2])
            );
        }
        other => panic!("expected array-arguments ToolCallDelta, got {other:?}"),
    }
}

#[test]
fn openai_tool_call_delta_reuses_index_id_across_chunks() {
    let mut id_map = HashMap::new();
    let first = r#"{
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_streamed",
                    "function": {"name": "search", "arguments": "{\"q\""}
                }]
            }
        }]
    }"#;
    let second = r#"{
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "function": {"arguments": ":\"rua\"}"}
                }]
            },
            "finish_reason": "tool_calls"
        }]
    }"#;

    let (first_events, first_finish) = parse_openai_chunk(first, &mut id_map).unwrap();
    let (second_events, second_finish) = parse_openai_chunk(second, &mut id_map).unwrap();

    assert!(first_finish.is_none());
    assert_eq!(second_finish, Some(FinishReason::ToolUse));

    let AgentEvent::ToolCallDelta { id: first_id, .. } = &first_events[0] else {
        panic!("first chunk should be ToolCallDelta");
    };
    let AgentEvent::ToolCallDelta {
        id: second_id,
        name,
        arguments_delta,
        ..
    } = &second_events[0]
    else {
        panic!("second chunk should be ToolCallDelta");
    };

    assert_eq!(first_id, "call_streamed");
    assert_eq!(second_id, "call_streamed");
    assert_eq!(name, &None);
    assert_eq!(arguments_delta, ":\"rua\"}");
}

#[test]
fn openai_tool_call_extra_content_is_preserved_in_metadata() {
    let chunk = r#"{
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_xyz",
                    "function": {
                        "name": "search",
                        "arguments": ""
                    },
                    "extra_content": {
                        "thought_signature": "sig_abc123"
                    }
                }]
            }
        }]
    }"#;
    let mut id_map = HashMap::new();

    let (events, _) = parse_openai_chunk(chunk, &mut id_map).unwrap();

    assert_eq!(events.len(), 1);
    match &events[0] {
        AgentEvent::ToolCallDelta { metadata, name, .. } => {
            assert_eq!(name.as_deref(), Some("search"));
            let extra = metadata
                .get("extra_content")
                .expect("extra_content in metadata");
            assert_eq!(
                extra.get("thought_signature").and_then(|v| v.as_str()),
                Some("sig_abc123")
            );
        }
        other => panic!("expected ToolCallDelta, got {other:?}"),
    }
}

#[test]
fn openai_tool_call_without_extra_content_has_null_metadata() {
    let chunk = r#"{
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_no_extra",
                    "function": {
                        "name": "get_time",
                        "arguments": "{}"
                    }
                }]
            }
        }]
    }"#;
    let mut id_map = HashMap::new();

    let (events, _) = parse_openai_chunk(chunk, &mut id_map).unwrap();

    assert_eq!(events.len(), 1);
    match &events[0] {
        AgentEvent::ToolCallDelta { metadata, .. } => {
            assert!(metadata.is_null());
        }
        other => panic!("expected ToolCallDelta, got {other:?}"),
    }
}

#[test]
fn anthropic_input_json_delta_uses_stable_index_id_and_partial_arguments() {
    let data = r#"{
        "index": 3,
        "delta": {
            "type": "input_json_delta",
            "partial_json": "{\"path\":\"/tmp"
        }
    }"#;

    let (events, finish) = parse_anthropic_event("content_block_delta", data).unwrap();

    assert!(finish.is_none());
    assert_eq!(events.len(), 1);
    match &events[0] {
        AgentEvent::ToolCallDelta {
            id,
            index,
            name,
            arguments_delta,
            ..
        } => {
            assert_eq!(id, "anthropic-tc-3");
            assert_eq!(*index, 3);
            assert_eq!(name, &None);
            assert_eq!(arguments_delta, "{\"path\":\"/tmp");
        }
        other => panic!("expected Anthropic ToolCallDelta, got {other:?}"),
    }
}

#[test]
fn gemini_function_call_chunk_emits_complete_tool_call() {
    let chunk = r#"{
        "candidates": [{
            "content": {
                "parts": [{
                    "functionCall": {
                        "name": "get_weather",
                        "args": {"city": "Tokyo"}
                    },
                    "thoughtSignature": "gemini-call-1"
                }]
            },
            "finishReason": "STOP"
        }]
    }"#;

    let (events, finish) = parse_gemini_chunk(chunk).unwrap();

    assert_eq!(finish, Some(FinishReason::Stop));
    assert_eq!(events.len(), 1);
    match &events[0] {
        AgentEvent::ToolCall { call } => {
            assert!(!call.id.is_empty());
            assert_ne!(call.id, "gemini-call-1");
            assert_eq!(call.name, "get_weather");
            assert_eq!(call.arguments, json!({"city": "Tokyo"}));
            assert!(call.raw_arguments.is_none());
            assert_eq!(
                call.metadata
                    .get("extra_content")
                    .and_then(|value| value.get("thought_signature"))
                    .and_then(|value| value.as_str()),
                Some("gemini-call-1")
            );
        }
        other => panic!("expected Gemini ToolCall, got {other:?}"),
    }
}

#[test]
fn gemini_thought_chunk_preserves_thought_signature() {
    let chunk = r#"{
        "candidates": [{
            "content": {
                "parts": [{
                    "text": "private reasoning",
                    "thought": true,
                    "thoughtSignature": "gemini-thought-sig"
                }]
            }
        }]
    }"#;

    let (events, finish) = parse_gemini_chunk(chunk).unwrap();

    assert!(finish.is_none());
    assert_eq!(events.len(), 1);
    match &events[0] {
        AgentEvent::ThinkingDelta {
            delta, signature, ..
        } => {
            assert_eq!(delta, "private reasoning");
            assert_eq!(signature.as_deref(), Some("gemini-thought-sig"));
        }
        other => panic!("expected Gemini ThinkingDelta, got {other:?}"),
    }
}

#[test]
fn gemini_error_chunk_emits_terminal_error_event() {
    let chunk = r#"{
        "type": "error",
        "error": {
            "message": "invalid tool schema",
            "type": "invalid_request_error"
        }
    }"#;

    let (events, finish) = parse_gemini_chunk(chunk).unwrap();

    assert!(finish.is_none());
    assert!(matches!(
        &events[0],
        AgentEvent::Error {
            code,
            message,
            retryable,
            ..
        } if code.as_deref() == Some("invalid_request_error")
            && message == "invalid tool schema"
            && !retryable
    ));
}
