use agent_types::{
    AgentEvent, ChatRequest, FinishReason, ModelDialogueItem, ReasoningConfig, ReasoningMode, Role,
    ToolDefinition,
};
use futures::StreamExt;
use llm_multiplex::providers::anthropic::AnthropicConfig;
use llm_multiplex::traits::Provider;
use rua_provider::idealab_anthropic::IdealabAnthropicProvider;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

fn test_config() -> AnthropicConfig {
    AnthropicConfig {
        provider_id: "idealab-anthropic-test".into(),
        provider_name: "Test".into(),
        base_url: "http://localhost:1".into(),
        api_key: Some("test-key".into()),
        ..Default::default()
    }
}

async fn fake_anthropic_sse_server(
    body: String,
) -> (
    String,
    oneshot::Sender<()>,
    oneshot::Receiver<serde_json::Value>,
) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake provider");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
    let (release_tx, release_rx) = oneshot::channel();
    let (request_tx, request_rx) = oneshot::channel();

    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept request");
        let mut bytes = Vec::new();
        let mut buf = [0_u8; 1024];
        let mut header_end = None;
        let mut content_length = 0_usize;

        loop {
            let read = socket.read(&mut buf).await.expect("read request");
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buf[..read]);
            if header_end.is_none() {
                header_end = bytes.windows(4).position(|window| window == b"\r\n\r\n");
                if let Some(end) = header_end {
                    let headers = String::from_utf8_lossy(&bytes[..end]);
                    content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                }
            }
            if let Some(end) = header_end {
                if bytes.len() >= end + 4 + content_length {
                    break;
                }
            }
        }

        let request = serde_json::from_slice(&bytes[header_end.expect("request headers") + 4..])
            .expect("request JSON");
        let _ = request_tx.send(request);
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\nconnection: keep-alive\r\n\r\n",
            )
            .await
            .expect("write response headers");
        socket
            .write_all(format!("{:X}\r\n", body.len()).as_bytes())
            .await
            .expect("write chunk length");
        socket
            .write_all(body.as_bytes())
            .await
            .expect("write SSE body");
        socket.write_all(b"\r\n").await.expect("finish chunk");
        socket.flush().await.expect("flush SSE body");
        let _ = release_rx.await;
    });

    (base_url, release_tx, request_rx)
}

#[tokio::test]
async fn streaming_effort_reasoning_sends_adaptive_thinking() {
    let sse = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
    let (base_url, release_server, captured_request) = fake_anthropic_sse_server(sse.into()).await;
    let provider = IdealabAnthropicProvider::new(AnthropicConfig {
        base_url,
        generate_kwargs: serde_json::Map::from_iter([
            ("thinking_enable".into(), serde_json::json!(true)),
            ("thinking_budget".into(), serde_json::json!(8192)),
        ]),
        ..test_config()
    })
    .expect("provider");
    let model = provider
        .create_chat_model("claude-opus-4-7")
        .expect("model");
    let request = ChatRequest::new(vec![ModelDialogueItem::text(Role::User, "hello")])
        .with_max_tokens(4096)
        .with_reasoning(ReasoningConfig {
            mode: ReasoningMode::Effort,
            enabled: true,
            effort: Some("high".into()),
            budget_tokens: None,
            summary: None,
        });
    let events = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        model.chat_stream(request).collect::<Vec<_>>(),
    )
    .await
    .expect("stream completes");
    let body = captured_request.await.expect("captured request");
    let _ = release_server.send(());
    assert!(events.iter().all(Result::is_ok));
    assert!(events
        .iter()
        .any(|event| matches!(event, Ok(AgentEvent::End { .. }))));
    assert_eq!(body["model"], "claude-opus-4-7");
    assert_eq!(body["stream"], true);
    assert_eq!(body["thinking"], serde_json::json!({"type": "adaptive"}));
    assert_eq!(body["output_config"]["effort"], "high");
    assert_eq!(body["max_tokens"], 4096);
    assert!(body.get("thinking_budget").is_none());
    assert!(body.get("thinking_enable").is_none());
}

#[test]
fn inner_provider_created_once_across_100_models() {
    let provider = IdealabAnthropicProvider::new(test_config()).unwrap();
    assert_eq!(provider.inner_ref_count(), 1);

    let mut models: Vec<Box<dyn llm_multiplex::traits::ChatModel>> = Vec::new();
    for _ in 0..100 {
        models.push(provider.create_chat_model("claude-test").unwrap());
    }

    assert_eq!(provider.inner_ref_count(), 101);

    drop(models);
    assert_eq!(provider.inner_ref_count(), 1);
}

#[test]
fn create_chat_model_returns_correct_model_info() {
    let provider = IdealabAnthropicProvider::new(test_config()).unwrap();
    let model = provider.create_chat_model("claude-opus-4-7").unwrap();
    assert_eq!(model.model_info().id, "claude-opus-4-7");
}

#[tokio::test]
async fn streaming_tool_use_emits_one_call_with_the_provider_identity() {
    let sse = [
        (
            "message_start",
            r#"{"type":"message_start","message":{"usage":{"input_tokens":10,"output_tokens":0}}}"#,
        ),
        (
            "content_block_start",
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"bash","input":{}}}"#,
        ),
        (
            "content_block_delta",
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"command\":\"pwd\"}"}}"#,
        ),
        (
            "content_block_stop",
            r#"{"type":"content_block_stop","index":1}"#,
        ),
        (
            "message_delta",
            r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":5}}"#,
        ),
        ("message_stop", r#"{"type":"message_stop"}"#),
    ]
    .into_iter()
    .map(|(event, data)| format!("event: {event}\ndata: {data}\n\n"))
    .collect::<String>();
    let (base_url, release_server, _) = fake_anthropic_sse_server(sse).await;
    let provider = IdealabAnthropicProvider::new(AnthropicConfig {
        provider_id: "idealab-anthropic-test".into(),
        provider_name: "Test".into(),
        base_url,
        api_key: Some("test-key".into()),
        ..Default::default()
    })
    .unwrap();
    let model = provider.create_chat_model("claude-test").unwrap();
    let request = ChatRequest::streaming(vec![ModelDialogueItem::text(Role::User, "run pwd")])
        .with_tools(vec![ToolDefinition {
            name: "bash".into(),
            description: "Run a shell command".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"command": {"type": "string"}},
                "required": ["command"]
            }),
            strict: None,
            annotations: Default::default(),
            metadata: Default::default(),
        }]);

    let events: Vec<_> = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        model.chat_stream(request).collect(),
    )
    .await
    .expect("message_stop must terminate the stream without waiting for HTTP EOF");
    let _ = release_server.send(());
    let events: Vec<_> = events.into_iter().collect::<Result<_, _>>().unwrap();
    let tool_events: Vec<_> = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                AgentEvent::ToolCallDelta { .. } | AgentEvent::ToolCall { .. }
            )
        })
        .collect();

    assert_eq!(tool_events.len(), 2, "start + arguments delta expected");
    assert!(matches!(
        tool_events[0],
        AgentEvent::ToolCallDelta {
            id,
            name: Some(name),
            arguments_delta,
            ..
        } if id == "toolu_1" && name == "bash" && arguments_delta.is_empty()
    ));
    assert!(matches!(
        tool_events[1],
        AgentEvent::ToolCallDelta {
            id,
            name: None,
            arguments_delta,
            ..
        } if id == "toolu_1" && arguments_delta == r#"{"command":"pwd"}"#
    ));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::End {
            finish_reason: FinishReason::ToolUse,
            ..
        }
    )));
}

#[tokio::test]
async fn streaming_error_terminates_without_waiting_for_http_eof() {
    let sse = concat!(
        "event: error\n",
        "data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"provider overloaded\"}}\n\n"
    )
    .to_string();
    let (base_url, release_server, _) = fake_anthropic_sse_server(sse).await;
    let provider = IdealabAnthropicProvider::new(AnthropicConfig {
        provider_id: "idealab-anthropic-test".into(),
        provider_name: "Test".into(),
        base_url,
        api_key: Some("test-key".into()),
        ..Default::default()
    })
    .unwrap();
    let model = provider.create_chat_model("claude-test").unwrap();
    let request = ChatRequest::streaming(vec![ModelDialogueItem::text(Role::User, "hi")]);

    let events = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        model.chat_stream(request).collect::<Vec<_>>(),
    )
    .await
    .expect("error event must terminate without waiting for HTTP EOF");
    let _ = release_server.send(());
    let events = events.into_iter().collect::<Result<Vec<_>, _>>().unwrap();

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::Error { code: Some(code), message, .. }
            if code == "overloaded_error" && message == "provider overloaded"
    )));
    assert!(!events
        .iter()
        .any(|event| matches!(event, AgentEvent::End { .. })));
}
