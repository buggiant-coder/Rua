use agent_types::{AgentEvent, ToolCall};
use futures::StreamExt;
use llm_multiplex::{
    stream_sanitizer::{sanitize_tool_call, try_repair_json},
    SanitizedStream,
};

fn make_event_stream(
    events: Vec<Result<AgentEvent, llm_multiplex::LlmError>>,
) -> llm_multiplex::EventStream {
    Box::pin(futures::stream::iter(events))
}

#[tokio::test]
async fn sanitized_stream_repairs_malformed_tool_call() {
    let malformed_call = ToolCall {
        id: "tc-1".to_string(),
        name: String::new(),
        arguments: serde_json::Value::Null,
        raw_arguments: Some(r#"{"key": "value",}"#.to_string()),
        provider_call_id: None,
        metadata: serde_json::Map::new(),
    };

    let events = vec![
        Ok(AgentEvent::Start {
            run_id: "r1".into(),
            model: Some("test".into()),
            metadata: serde_json::Value::Null,
        }),
        Ok(AgentEvent::ToolCall {
            call: malformed_call,
        }),
        Ok(AgentEvent::End {
            finish_reason: agent_types::FinishReason::Stop,
            usage: None,
            metadata: serde_json::Value::Null,
        }),
    ];

    let inner = make_event_stream(events);
    let mut stream = SanitizedStream::wrap(inner);

    let ev = stream.next().await.unwrap().unwrap();
    assert!(matches!(ev, AgentEvent::Start { .. }));

    let ev = stream.next().await.unwrap().unwrap();
    if let AgentEvent::ToolCall { call } = ev {
        assert_eq!(call.arguments, serde_json::json!({"key": "value"}));
    } else {
        panic!("expected ToolCall, got {:?}", ev);
    }

    let ev = stream.next().await.unwrap().unwrap();
    assert!(matches!(ev, AgentEvent::End { .. }));
}

#[test]
fn tool_argument_repair_accepts_first_json_value_before_trailing_model_text() {
    assert_eq!(
        try_repair_json(r#"{"city":"Tokyo"} trailing text"#),
        Some(serde_json::json!({"city": "Tokyo"}))
    );
    assert_eq!(
        try_repair_json(r#"{} trailing text"#),
        Some(serde_json::json!({}))
    );
}

#[test]
fn sanitize_tool_call_allows_empty_name_without_crashing() {
    let call = ToolCall {
        id: "tc-1".to_string(),
        name: String::new(),
        arguments: serde_json::json!({"key": "value"}),
        raw_arguments: None,
        provider_call_id: None,
        metadata: serde_json::Map::new(),
    };

    let result = sanitize_tool_call(call);
    assert_eq!(result.name, "");
    assert_eq!(result.arguments, serde_json::json!({"key": "value"}));
}

#[test]
fn sanitize_tool_call_preserves_valid_name() {
    let call = ToolCall {
        id: "tc-2".to_string(),
        name: "get_weather".to_string(),
        arguments: serde_json::json!({}),
        raw_arguments: None,
        provider_call_id: None,
        metadata: serde_json::Map::new(),
    };

    let result = sanitize_tool_call(call);
    assert_eq!(result.name, "get_weather");
}

#[test]
fn sanitize_tool_call_repairs_arguments_with_empty_name() {
    let call = ToolCall {
        id: "tc-3".to_string(),
        name: String::new(),
        arguments: serde_json::Value::Null,
        raw_arguments: Some(r#"{"city": "Tokyo",}"#.to_string()),
        provider_call_id: None,
        metadata: serde_json::Map::new(),
    };

    let result = sanitize_tool_call(call);
    assert_eq!(result.name, "");
    assert_eq!(result.arguments, serde_json::json!({"city": "Tokyo"}));
}

#[test]
fn sanitize_tool_call_repairs_empty_object_before_trailing_model_text() {
    let call = ToolCall {
        id: "tc-4".to_string(),
        name: "list_models".to_string(),
        arguments: serde_json::Value::Null,
        raw_arguments: Some(r#"{} trailing model explanation"#.to_string()),
        provider_call_id: None,
        metadata: serde_json::Map::new(),
    };

    let result = sanitize_tool_call(call);

    assert_eq!(result.name, "list_models");
    assert_eq!(result.arguments, serde_json::json!({}));
}

#[tokio::test]
async fn sanitized_stream_extracts_xml_tool_call_from_text() {
    let events = vec![
        Ok(AgentEvent::Start {
            run_id: "r2".into(),
            model: Some("test".into()),
            metadata: serde_json::Value::Null,
        }),
        Ok(AgentEvent::TextDelta {
            delta: r#"<tool_call>{"name":"read_file","arguments":{"path":"/tmp"}}</tool_call>"#
                .to_string(),
            metadata: serde_json::Value::Null,
        }),
        Ok(AgentEvent::End {
            finish_reason: agent_types::FinishReason::Stop,
            usage: None,
            metadata: serde_json::Value::Null,
        }),
    ];

    let inner = make_event_stream(events);
    let mut stream = SanitizedStream::wrap(inner);

    let ev = stream.next().await.unwrap().unwrap();
    assert!(matches!(ev, AgentEvent::Start { .. }));

    let ev = stream.next().await.unwrap().unwrap();
    if let AgentEvent::ToolCall { call } = ev {
        assert_eq!(call.name, "read_file");
        assert_eq!(call.arguments, serde_json::json!({"path": "/tmp"}));
    } else {
        panic!("expected ToolCall from XML extraction, got {:?}", ev);
    }

    let ev = stream.next().await.unwrap().unwrap();
    assert!(matches!(ev, AgentEvent::End { .. }));
}

#[tokio::test]
async fn sanitized_stream_extracts_think_tags() {
    let events = vec![
        Ok(AgentEvent::Start {
            run_id: "r3".into(),
            model: Some("test".into()),
            metadata: serde_json::Value::Null,
        }),
        Ok(AgentEvent::TextDelta {
            delta: "prefix <think>reasoning here</think> suffix".to_string(),
            metadata: serde_json::Value::Null,
        }),
        Ok(AgentEvent::End {
            finish_reason: agent_types::FinishReason::Stop,
            usage: None,
            metadata: serde_json::Value::Null,
        }),
    ];

    let inner = make_event_stream(events);
    let mut stream = SanitizedStream::wrap(inner);

    stream.next().await.unwrap().unwrap();

    let mut found_thinking = false;
    let mut found_text_prefix = false;
    let mut found_text_suffix = false;

    loop {
        let ev = stream.next().await.unwrap().unwrap();
        match &ev {
            AgentEvent::TextDelta { delta, .. } => {
                if delta.contains("prefix") {
                    found_text_prefix = true;
                }
                if delta.contains("suffix") {
                    found_text_suffix = true;
                }
            }
            AgentEvent::ThinkingDelta { delta, .. } if delta.contains("reasoning here") => {
                found_thinking = true;
            }
            AgentEvent::End { .. } => break,
            _ => {}
        }
    }

    assert!(found_thinking, "should have extracted thinking content");
    assert!(found_text_prefix, "should have prefix text");
    assert!(found_text_suffix, "should have suffix text");
}

#[tokio::test]
async fn sanitized_stream_does_not_classify_unclosed_think_as_thinking() {
    let events = vec![
        Ok(AgentEvent::TextDelta {
            delta: "<think>reasoning starts".to_string(),
            metadata: serde_json::Value::Null,
        }),
        Ok(AgentEvent::TextDelta {
            delta: " but provider never closes".to_string(),
            metadata: serde_json::Value::Null,
        }),
        Ok(AgentEvent::End {
            finish_reason: agent_types::FinishReason::Stop,
            usage: None,
            metadata: serde_json::Value::Null,
        }),
    ];

    let inner = make_event_stream(events);
    let mut stream = SanitizedStream::wrap(inner);
    let mut text = String::new();
    let mut thinking = String::new();

    while let Some(Ok(event)) = stream.next().await {
        match event {
            AgentEvent::TextDelta { delta, .. } => text.push_str(&delta),
            AgentEvent::ThinkingDelta { delta, .. } => thinking.push_str(&delta),
            AgentEvent::End { .. } => break,
            _ => {}
        }
    }

    assert_eq!(thinking, "");
    assert_eq!(text, "reasoning starts but provider never closes");
}

#[tokio::test]
async fn sanitized_stream_preserves_stray_closing_think_as_text() {
    let events = vec![
        Ok(AgentEvent::ThinkingDelta {
            delta: "native reasoning".to_string(),
            signature: None,
            metadata: serde_json::Value::Null,
        }),
        Ok(AgentEvent::TextDelta {
            delta: "\n</think>\n\n".to_string(),
            metadata: serde_json::Value::Null,
        }),
        Ok(AgentEvent::TextDelta {
            delta: "visible answer".to_string(),
            metadata: serde_json::Value::Null,
        }),
        Ok(AgentEvent::End {
            finish_reason: agent_types::FinishReason::Stop,
            usage: None,
            metadata: serde_json::Value::Null,
        }),
    ];

    let inner = make_event_stream(events);
    let mut stream = SanitizedStream::wrap(inner);
    let mut text = String::new();
    let mut thinking = String::new();

    while let Some(Ok(event)) = stream.next().await {
        match event {
            AgentEvent::TextDelta { delta, .. } => text.push_str(&delta),
            AgentEvent::ThinkingDelta { delta, .. } => thinking.push_str(&delta),
            AgentEvent::End { .. } => break,
            _ => {}
        }
    }

    assert_eq!(thinking, "native reasoning");
    assert_eq!(text, "\n</think>\n\nvisible answer");
}

#[tokio::test]
async fn sanitized_stream_preserves_text_after_tag_before_later_plain_chunk() {
    let events = vec![
        Ok(AgentEvent::TextDelta {
            delta:
                r#"before<tool_call>{"name":"search","arguments":{"q":"rust"}}</tool_call>after"#
                    .to_string(),
            metadata: serde_json::Value::Null,
        }),
        Ok(AgentEvent::TextDelta {
            delta: " later".to_string(),
            metadata: serde_json::Value::Null,
        }),
        Ok(AgentEvent::End {
            finish_reason: agent_types::FinishReason::Stop,
            usage: None,
            metadata: serde_json::Value::Null,
        }),
    ];

    let inner = make_event_stream(events);
    let mut stream = SanitizedStream::wrap(inner);
    let mut text = String::new();
    let mut tool_names = Vec::new();

    while let Some(Ok(event)) = stream.next().await {
        match event {
            AgentEvent::TextDelta { delta, .. } => text.push_str(&delta),
            AgentEvent::ToolCall { call } => tool_names.push(call.name),
            AgentEvent::End { .. } => break,
            _ => {}
        }
    }

    assert_eq!(tool_names, vec!["search"]);
    assert_eq!(text, "beforeafter later");
}
