use agent_types::AgentEvent;
use llm_multiplex::event_parser::parse_anthropic_event;

#[test]
fn message_delta_captures_cache_tokens() {
    let data = r#"{
        "delta": { "stop_reason": "end_turn" },
        "usage": {
            "input_tokens": 10,
            "output_tokens": 50,
            "cache_read_input_tokens": 100,
            "cache_creation_input_tokens": 50
        }
    }"#;
    let (events, _) = parse_anthropic_event("message_delta", data).unwrap();
    let usage_event = events
        .iter()
        .find(|e| matches!(e, AgentEvent::Usage { .. }));
    assert!(usage_event.is_some(), "should emit Usage event");
    if let AgentEvent::Usage { usage } = usage_event.unwrap() {
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 50);
        assert_eq!(usage.cached_tokens, Some(100));
        assert_eq!(usage.cache_write_tokens, Some(50));
    }
}

#[test]
fn message_delta_no_cache_fields_returns_none() {
    let data = r#"{
        "delta": { "stop_reason": "end_turn" },
        "usage": {
            "input_tokens": 0,
            "output_tokens": 42
        }
    }"#;
    let (events, _) = parse_anthropic_event("message_delta", data).unwrap();
    let usage_event = events
        .iter()
        .find(|e| matches!(e, AgentEvent::Usage { .. }));
    assert!(usage_event.is_some());
    if let AgentEvent::Usage { usage } = usage_event.unwrap() {
        assert_eq!(usage.completion_tokens, 42);
        assert_eq!(usage.cached_tokens, None);
        assert_eq!(usage.cache_write_tokens, None);
    }
}

#[test]
fn message_start_captures_cache_tokens() {
    let data = r#"{
        "message": {
            "usage": {
                "input_tokens": 500,
                "output_tokens": 0,
                "cache_read_input_tokens": 200,
                "cache_creation_input_tokens": 100
            }
        }
    }"#;
    let (events, _) = parse_anthropic_event("message_start", data).unwrap();
    let usage_event = events
        .iter()
        .find(|e| matches!(e, AgentEvent::Usage { .. }));
    assert!(usage_event.is_some(), "should emit Usage event");
    if let AgentEvent::Usage { usage } = usage_event.unwrap() {
        assert_eq!(usage.prompt_tokens, 500);
        assert_eq!(usage.cached_tokens, Some(200));
        assert_eq!(usage.cache_write_tokens, Some(100));
    }
}

#[test]
fn message_start_no_cache_fields_returns_none() {
    let data = r#"{
        "message": {
            "usage": {
                "input_tokens": 300,
                "output_tokens": 0
            }
        }
    }"#;
    let (events, _) = parse_anthropic_event("message_start", data).unwrap();
    if let AgentEvent::Usage { usage } = &events[0] {
        assert_eq!(usage.prompt_tokens, 300);
        assert_eq!(usage.cached_tokens, None);
        assert_eq!(usage.cache_write_tokens, None);
    } else {
        panic!("expected Usage event");
    }
}

#[test]
fn message_start_captures_thinking_tokens_as_reasoning_tokens() {
    let data = r#"{
        "message": {
            "usage": {
                "input_tokens": 50,
                "output_tokens": 10,
                "thinking_tokens": 300
            }
        }
    }"#;
    let (events, _) = parse_anthropic_event("message_start", data).unwrap();
    let usage_event = events
        .iter()
        .find(|e| matches!(e, AgentEvent::Usage { .. }));
    assert!(usage_event.is_some());
    if let AgentEvent::Usage { usage } = usage_event.unwrap() {
        assert_eq!(usage.reasoning_tokens, Some(300));
    }
}

#[test]
fn message_delta_captures_thinking_tokens_as_reasoning_tokens() {
    let data = r#"{
        "delta": { "stop_reason": "end_turn" },
        "usage": {
            "input_tokens": 0,
            "output_tokens": 42,
            "thinking_tokens": 200
        }
    }"#;
    let (events, _) = parse_anthropic_event("message_delta", data).unwrap();
    let usage_event = events
        .iter()
        .find(|e| matches!(e, AgentEvent::Usage { .. }));
    assert!(usage_event.is_some());
    if let AgentEvent::Usage { usage } = usage_event.unwrap() {
        assert_eq!(usage.reasoning_tokens, Some(200));
        assert_eq!(usage.completion_tokens, 42);
    }
}

#[test]
fn thinking_tokens_take_priority_over_reasoning_tokens() {
    let data = r#"{
        "message": {
            "usage": {
                "input_tokens": 1,
                "output_tokens": 1,
                "thinking_tokens": 100,
                "reasoning_tokens": 200
            }
        }
    }"#;
    let (events, _) = parse_anthropic_event("message_start", data).unwrap();
    if let AgentEvent::Usage { usage } = &events[0] {
        assert_eq!(usage.reasoning_tokens, Some(100));
    } else {
        panic!("expected Usage event");
    }
}
