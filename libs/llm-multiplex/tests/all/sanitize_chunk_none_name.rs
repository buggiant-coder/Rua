use std::collections::HashMap;

use agent_types::AgentEvent;
use llm_multiplex::event_parser::parse_openai_chunk;

#[test]
fn null_name_maps_to_none() {
    let chunk = r#"{
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_1",
                    "function": {
                        "name": null,
                        "arguments": "{}"
                    }
                }]
            }
        }]
    }"#;
    let mut map = HashMap::new();
    let (events, _) = parse_openai_chunk(chunk, &mut map).unwrap();
    assert_eq!(events.len(), 1);
    if let AgentEvent::ToolCallDelta { name, .. } = &events[0] {
        assert_eq!(name, &None, "null should map to None, not Some(\"\")");
    } else {
        panic!("expected ToolCallDelta");
    }
}

#[test]
fn absent_name_maps_to_none() {
    let chunk = r#"{
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "function": {
                        "arguments": "more"
                    }
                }]
            }
        }]
    }"#;
    let mut map = HashMap::new();
    map.insert(0, "call_prev".to_string());
    let (events, _) = parse_openai_chunk(chunk, &mut map).unwrap();
    assert_eq!(events.len(), 1);
    if let AgentEvent::ToolCallDelta { name, .. } = &events[0] {
        assert_eq!(name, &None, "absent field should map to None");
    } else {
        panic!("expected ToolCallDelta");
    }
}

#[test]
fn explicit_empty_string_maps_to_some_empty() {
    let chunk = r#"{
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_2",
                    "function": {
                        "name": "",
                        "arguments": "{}"
                    }
                }]
            }
        }]
    }"#;
    let mut map = HashMap::new();
    let (events, _) = parse_openai_chunk(chunk, &mut map).unwrap();
    assert_eq!(events.len(), 1);
    if let AgentEvent::ToolCallDelta { name, .. } = &events[0] {
        assert_eq!(name, &Some(String::new()), "\"\" should map to Some(\"\")");
    } else {
        panic!("expected ToolCallDelta");
    }
}

#[test]
fn object_arguments_are_stringified_for_stream_delta_reassembly() {
    let chunk = r#"{
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_object_args",
                    "function": {
                        "name": "lookup",
                        "arguments": {"city": "Tokyo", "days": 2}
                    }
                }]
            }
        }]
    }"#;
    let mut map = HashMap::new();
    let (events, _) = parse_openai_chunk(chunk, &mut map).unwrap();

    assert_eq!(events.len(), 1);
    if let AgentEvent::ToolCallDelta {
        name,
        arguments_delta,
        ..
    } = &events[0]
    {
        assert_eq!(name.as_deref(), Some("lookup"));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(arguments_delta).unwrap(),
            serde_json::json!({"city": "Tokyo", "days": 2})
        );
    } else {
        panic!("expected ToolCallDelta");
    }
}

#[test]
fn missing_function_emits_empty_delta_instead_of_panicking_or_dropping_index() {
    let chunk = r#"{
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 3,
                    "id": "call_missing_function"
                }]
            }
        }]
    }"#;
    let mut map = HashMap::new();
    let (events, _) = parse_openai_chunk(chunk, &mut map).unwrap();

    assert_eq!(events.len(), 1);
    if let AgentEvent::ToolCallDelta {
        id,
        index,
        name,
        arguments_delta,
        ..
    } = &events[0]
    {
        assert_eq!(id, "call_missing_function");
        assert_eq!(*index, 3);
        assert_eq!(name, &None);
        assert_eq!(arguments_delta, "");
        assert_eq!(
            map.get(&3).map(String::as_str),
            Some("call_missing_function"),
            "future chunks for the same index should reuse the provider call id"
        );
    } else {
        panic!("expected ToolCallDelta");
    }
}

#[test]
fn streaming_null_then_real_name() {
    let mut map = HashMap::new();

    let chunk1 = r#"{
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_stream",
                    "function": {
                        "name": null,
                        "arguments": ""
                    }
                }]
            }
        }]
    }"#;
    let (events1, _) = parse_openai_chunk(chunk1, &mut map).unwrap();
    assert_eq!(events1.len(), 1);
    if let AgentEvent::ToolCallDelta { name, .. } = &events1[0] {
        assert_eq!(name, &None, "first chunk null name → None (no update)");
    } else {
        panic!("expected ToolCallDelta");
    }

    let chunk2 = r#"{
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "function": {
                        "name": "real_fn",
                        "arguments": "{\"key\":\"val\"}"
                    }
                }]
            }
        }]
    }"#;
    let (events2, _) = parse_openai_chunk(chunk2, &mut map).unwrap();
    assert_eq!(events2.len(), 1);
    if let AgentEvent::ToolCallDelta { name, .. } = &events2[0] {
        assert_eq!(
            name,
            &Some("real_fn".to_string()),
            "second chunk should carry real name"
        );
    } else {
        panic!("expected ToolCallDelta");
    }
}
