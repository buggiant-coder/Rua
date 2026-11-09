use agent_types::AgentEvent;
use llm_multiplex::event_parser::parse_openai_chunk;
use llm_multiplex::tag_parser::{TagParseMode, TagParser, TagSegment};
use std::collections::HashMap;

#[test]
fn segments_to_events_produces_unified_tool_call_format() {
    let segments = vec![
        TagSegment::Text("hello ".into()),
        TagSegment::Thinking("reasoning".into()),
        TagSegment::ToolCall {
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "/tmp/x"}),
        },
        TagSegment::Text(" world".into()),
        TagSegment::ToolCall {
            name: "write_file".into(),
            arguments: serde_json::json!({"path": "/tmp/y", "content": "z"}),
        },
    ];

    let events = TagParser::segments_to_events(&segments, 0);
    assert_eq!(events.len(), 5);

    assert!(matches!(&events[0], AgentEvent::TextDelta { delta, .. } if delta == "hello "));
    assert!(matches!(
        &events[1],
        AgentEvent::ThinkingDelta { delta, .. } if delta == "reasoning"
    ));

    if let AgentEvent::ToolCall { call } = &events[2] {
        assert_eq!(call.id, "local-tc-1");
        assert_eq!(call.name, "read_file");
        assert_eq!(call.arguments, serde_json::json!({"path": "/tmp/x"}));
    } else {
        panic!("expected ToolCall at index 2");
    }

    assert!(matches!(&events[3], AgentEvent::TextDelta { delta, .. } if delta == " world"));

    if let AgentEvent::ToolCall { call } = &events[4] {
        assert_eq!(call.id, "local-tc-2");
        assert_eq!(call.name, "write_file");
    } else {
        panic!("expected ToolCall at index 4");
    }
}

#[test]
fn segments_to_events_respects_tool_call_offset() {
    let segments = vec![TagSegment::ToolCall {
        name: "exec".into(),
        arguments: serde_json::json!({}),
    }];

    let events = TagParser::segments_to_events(&segments, 5);
    assert_eq!(events.len(), 1);
    if let AgentEvent::ToolCall { call } = &events[0] {
        assert_eq!(call.id, "local-tc-6");
    } else {
        panic!("expected ToolCall");
    }
}

#[test]
fn strict_mode_errors_on_bad_json() {
    let mut parser = TagParser::with_mode(TagParseMode::Strict);
    parser.push("<tool_call>not valid json</tool_call>");
    let segments = parser.finish();
    assert_eq!(segments.len(), 1);
    assert!(matches!(&segments[0], TagSegment::Error(msg) if msg.contains("malformed")));
}

#[test]
fn lenient_mode_emits_text_on_bad_json() {
    let mut parser = TagParser::with_mode(TagParseMode::Lenient);
    parser.push("<tool_call>not valid json</tool_call>");
    let segments = parser.finish();
    assert_eq!(segments.len(), 1);
    assert!(matches!(&segments[0], TagSegment::Text(t) if t.contains("not valid json")));
}

#[test]
fn stream_parse_and_convert_roundtrip() {
    let input = r#"Hello <think>internal</think> text <tool_call>{"name":"bash","arguments":{"cmd":"ls"}}</tool_call> end"#;
    let mut parser = TagParser::new();
    parser.push(input);
    let segments = parser.finish();
    let events = TagParser::segments_to_events(&segments, 0);

    let text_events: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text_events, vec!["Hello ", " text ", " end"]);

    let think_events: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ThinkingDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(think_events, vec!["internal"]);

    let tc_events: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolCall { call } => Some(call),
            _ => None,
        })
        .collect();
    assert_eq!(tc_events.len(), 1);
    assert_eq!(tc_events[0].id, "local-tc-1");
    assert_eq!(tc_events[0].name, "bash");
    assert_eq!(tc_events[0].arguments, serde_json::json!({"cmd": "ls"}));
}

#[test]
fn openai_chunk_emits_reasoning_before_visible_content() {
    let chunk = serde_json::json!({
        "choices": [{
            "delta": {
                "reasoning_content": "think",
                "content": "answer"
            }
        }]
    })
    .to_string();
    let (events, _) = parse_openai_chunk(&chunk, &mut HashMap::new()).unwrap();

    assert_eq!(events.len(), 2);
    assert!(matches!(&events[0], AgentEvent::ThinkingDelta { delta, .. } if delta == "think"));
    assert!(matches!(&events[1], AgentEvent::TextDelta { delta, .. } if delta == "answer"));
}
