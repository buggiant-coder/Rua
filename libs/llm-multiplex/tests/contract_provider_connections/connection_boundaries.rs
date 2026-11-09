use std::collections::HashMap;

use agent_types::{AgentEvent, ChatRequest, ModelDialogueItem, Role};
use futures::StreamExt;
use llm_multiplex::debug_capture::{
    self, ActiveCapture, CHAT_ADDRESS_METADATA_KEY, MODEL_CALL_ID_METADATA_KEY,
};
use llm_multiplex::{
    AnthropicConfig, AnthropicProvider, ConnectionStatus, GeminiConfig, GeminiProvider, LlmError,
    ModelInfo, OllamaConfig, OllamaProvider, OpenAiCompatibleConfig, OpenAiCompatibleProvider,
    OpenAiResponsesProvider, Provider,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

#[derive(Debug)]
struct CapturedRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: String,
}

fn model(id: &str) -> ModelInfo {
    ModelInfo {
        id: id.to_string(),
        name: id.to_string(),
        context_window: None,
        max_output_tokens: None,
        supports_multimodal: None,
        supports_image: None,
        supports_video: None,
        probe_source: None,
        generate_kwargs: serde_json::Map::new(),
    }
}

async fn fake_provider_server(
    status: u16,
    body: &'static str,
) -> (String, oneshot::Receiver<CapturedRequest>) {
    let (base_url, mut requests) = fake_provider_pages(vec![(status, body)]).await;
    (base_url, requests.remove(0))
}

async fn fake_provider_pages(
    responses: Vec<(u16, &'static str)>,
) -> (String, Vec<oneshot::Receiver<CapturedRequest>>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake provider");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
    let (senders, receivers): (Vec<_>, Vec<_>) =
        responses.iter().map(|_| oneshot::channel()).unzip();

    tokio::spawn(async move {
        for ((status, body), tx) in responses.into_iter().zip(senders) {
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
                    header_end = find_header_end(&bytes);
                    if let Some(end) = header_end {
                        let header_text = String::from_utf8_lossy(&bytes[..end]);
                        content_length = parse_content_length(&header_text);
                    }
                }

                if let Some(end) = header_end {
                    if bytes.len() >= end + 4 + content_length {
                        break;
                    }
                }
            }

            let captured = parse_request(&bytes);
            let response = format!(
            "HTTP/1.1 {status} OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write response");
            let _ = tx.send(captured);
        }
    });

    (base_url, receivers)
}

async fn fake_chunked_sse_server(chunks: Vec<Vec<u8>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake provider");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));

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
                header_end = find_header_end(&bytes);
                if let Some(end) = header_end {
                    let header_text = String::from_utf8_lossy(&bytes[..end]);
                    content_length = parse_content_length(&header_text);
                }
            }

            if let Some(end) = header_end {
                if bytes.len() >= end + 4 + content_length {
                    break;
                }
            }
        }

        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n",
            )
            .await
            .expect("write response headers");

        for chunk in chunks {
            socket
                .write_all(format!("{:X}\r\n", chunk.len()).as_bytes())
                .await
                .expect("write chunk length");
            socket.write_all(&chunk).await.expect("write chunk");
            socket.write_all(b"\r\n").await.expect("finish chunk");
            socket.flush().await.expect("flush chunk");
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        socket
            .write_all(b"0\r\n\r\n")
            .await
            .expect("finish response");
    });

    base_url
}

async fn fake_disconnect_before_headers_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake provider");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));

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
                header_end = find_header_end(&bytes);
                if let Some(end) = header_end {
                    let header_text = String::from_utf8_lossy(&bytes[..end]);
                    content_length = parse_content_length(&header_text);
                }
            }

            if let Some(end) = header_end {
                if bytes.len() >= end + 4 + content_length {
                    break;
                }
            }
        }
    });

    base_url
}

async fn fake_open_sse_server(body: String) -> (String, oneshot::Sender<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake provider");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
    let (release_tx, release_rx) = oneshot::channel();

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
                header_end = find_header_end(&bytes);
                if let Some(end) = header_end {
                    let header_text = String::from_utf8_lossy(&bytes[..end]);
                    content_length = parse_content_length(&header_text);
                }
            }
            if let Some(end) = header_end {
                if bytes.len() >= end + 4 + content_length {
                    break;
                }
            }
        }

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

    (base_url, release_tx)
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn parse_content_length(header_text: &str) -> usize {
    header_text
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())
                .flatten()
        })
        .unwrap_or_default()
}

fn parse_request(bytes: &[u8]) -> CapturedRequest {
    let header_end = find_header_end(bytes).expect("request headers");
    let header_text = String::from_utf8_lossy(&bytes[..header_end]);
    let mut lines = header_text.lines();
    let request_line = lines.next().expect("request line");
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().expect("method").to_string();
    let path = request_parts.next().expect("path").to_string();
    let headers = lines
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.to_ascii_lowercase(), value.trim().to_string()))
        })
        .collect();
    let body = String::from_utf8_lossy(&bytes[header_end + 4..]).to_string();

    CapturedRequest {
        method,
        path,
        headers,
        body,
    }
}

fn openai_config(
    base_url: String,
    discovery: bool,
    connection_check: bool,
) -> OpenAiCompatibleConfig {
    OpenAiCompatibleConfig {
        provider_id: "openai-compatible".into(),
        provider_name: "OpenAI Compatible".into(),
        base_url,
        api_key: Some("sk-test-provider-key".into()),
        api_keys: Vec::new(),
        headers: serde_json::Map::new(),
        models: vec![model("configured-model")],
        timeout_secs: 1,
        generate_kwargs: serde_json::Map::new(),
        support_model_discovery: discovery,
        support_connection_check: connection_check,
        is_local: false,
    }
}

fn siliconflow_config(base_url: String) -> OpenAiCompatibleConfig {
    OpenAiCompatibleConfig {
        provider_id: "siliconflow-cn".into(),
        provider_name: "SiliconFlow (China)".into(),
        base_url,
        api_key: Some("sk-siliconflow-test-key".into()),
        api_keys: Vec::new(),
        headers: serde_json::Map::new(),
        models: vec![],
        timeout_secs: 1,
        generate_kwargs: serde_json::Map::new(),
        support_model_discovery: false,
        support_connection_check: true,
        is_local: false,
    }
}

fn ollama_config(base_url: String) -> OllamaConfig {
    OllamaConfig {
        provider_id: "ollama".into(),
        provider_name: "Ollama".into(),
        base_url,
        models: vec![model("qwen2.5")],
        timeout_secs: 1,
        generate_kwargs: serde_json::Map::new(),
    }
}

#[tokio::test]
async fn openai_compatible_static_modes_do_not_require_network() {
    let provider =
        OpenAiCompatibleProvider::new(openai_config("http://127.0.0.1:9".into(), false, false))
            .expect("provider");

    assert!(provider
        .check_connection()
        .await
        .expect("connection status")
        .is_connected());

    let models = provider.fetch_models().await.expect("static models");
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "configured-model");
    assert_eq!(models[0].probe_source, None);
}

#[tokio::test]
async fn openai_compatible_fetch_models_uses_models_endpoint_when_discovery_enabled() {
    let (base_url, request) = fake_provider_server(
        200,
        r#"{"data":[{"id":"gpt-test","name":"GPT Test","context_window":128000}]}"#,
    )
    .await;
    let provider =
        OpenAiCompatibleProvider::new(openai_config(base_url, true, true)).expect("provider");

    let models = provider.fetch_models().await.expect("discovered models");

    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "gpt-test");
    assert_eq!(models[0].name, "GPT Test");
    assert_eq!(models[0].context_window, Some(128000));
    assert_eq!(models[0].probe_source.as_deref(), Some("api"));

    let request = request.await.expect("captured request");
    assert_eq!(request.method, "GET");
    assert_eq!(request.path, "/models");
    assert_eq!(
        request.headers.get("authorization").map(String::as_str),
        Some("Bearer sk-test-provider-key")
    );
}

#[tokio::test]
async fn openai_compatible_stream_preserves_utf8_split_across_http_chunks() {
    let sse = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"架构\"},",
        "\"finish_reason\":null}]}\n\n",
        "data: [DONE]\n\n"
    );
    let split = sse.find('架').expect("Chinese content") + 1;
    let base_url = fake_chunked_sse_server(vec![
        sse.as_bytes()[..split].to_vec(),
        sse.as_bytes()[split..].to_vec(),
    ])
    .await;
    let provider =
        OpenAiCompatibleProvider::new(openai_config(base_url, false, false)).expect("provider");
    let model = provider
        .create_chat_model("configured-model")
        .expect("chat model");
    let mut stream = model.chat_stream(ChatRequest::streaming(vec![ModelDialogueItem::text(
        Role::User,
        "explain",
    )]));
    let mut text = String::new();

    while let Some(event) = stream.next().await {
        if let AgentEvent::TextDelta { delta, .. } = event.expect("stream event") {
            text.push_str(&delta);
        }
    }

    assert_eq!(text, "架构");
    assert!(!text.contains('\u{FFFD}'));
}

#[tokio::test]
async fn openai_compatible_disconnect_before_headers_is_retryable_connection_error() {
    let base_url = fake_disconnect_before_headers_server().await;
    let provider =
        OpenAiCompatibleProvider::new(openai_config(base_url, false, false)).expect("provider");
    let model = provider
        .create_chat_model("configured-model")
        .expect("chat model");
    let events = model
        .chat_stream(ChatRequest::streaming(vec![ModelDialogueItem::text(
            Role::User,
            "hello",
        )]))
        .collect::<Vec<_>>()
        .await;
    let error = events
        .into_iter()
        .find_map(Result::err)
        .expect("disconnect must surface an error");

    assert!(
        matches!(error, LlmError::Connection { .. }),
        "disconnect before response headers must be retryable, got {error:?}"
    );
    assert!(error.is_retryable());
}

#[tokio::test]
async fn openai_compatible_fetch_models_normalizes_and_deduplicates_api_rows() {
    let (base_url, request) = fake_provider_server(
        200,
        r#"{"data":[
            {"id":"gpt-4o-mini","name":"GPT-4o Mini"},
            {"id":"gpt-4o-mini","name":"duplicate"},
            {"id":"gpt-4.1","name":""},
            {"id":"    ","name":"invalid"}
        ]}"#,
    )
    .await;
    let provider =
        OpenAiCompatibleProvider::new(openai_config(base_url, true, true)).expect("provider");

    let models = provider.fetch_models().await.expect("discovered models");

    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id, "gpt-4o-mini");
    assert_eq!(models[0].name, "GPT-4o Mini");
    assert_eq!(models[1].id, "gpt-4.1");
    assert_eq!(models[1].name, "gpt-4.1");
    assert!(models
        .iter()
        .all(|model| model.probe_source.as_deref() == Some("api")));

    let request = request.await.expect("captured request");
    assert_eq!(request.method, "GET");
    assert_eq!(request.path, "/models");
}

#[tokio::test]
async fn openai_compatible_connection_check_allows_empty_model_catalogs() {
    let (base_url, request) = fake_provider_server(200, r#"{"data":[]}"#).await;
    let provider = OpenAiCompatibleProvider::new(siliconflow_config(base_url)).expect("provider");

    let status = provider
        .check_connection()
        .await
        .expect("connection status");
    assert!(
        status.is_connected(),
        "SiliconFlow-style providers may have no preset/discovered models but a successful /models probe still means connected"
    );
    assert!(
        provider
            .fetch_models()
            .await
            .expect("static models")
            .is_empty(),
        "support_model_discovery=false keeps the public model list empty"
    );

    let request = request.await.expect("captured request");
    assert_eq!(request.method, "GET");
    assert_eq!(request.path, "/models");
    assert_eq!(
        request.headers.get("authorization").map(String::as_str),
        Some("Bearer sk-siliconflow-test-key")
    );
}

#[tokio::test]
async fn openai_compatible_auth_failure_is_connection_status_not_thrown_error() {
    let (base_url, _request) =
        fake_provider_server(401, r#"{"error":{"message":"bad key sk-test-secret"}}"#).await;
    let provider =
        OpenAiCompatibleProvider::new(openai_config(base_url, true, true)).expect("provider");

    let status = provider.check_connection().await.expect("status result");
    match status {
        ConnectionStatus::Failed(message) => {
            assert_eq!(
                message,
                "auth failed for provider openai-compatible (HTTP 401): bad key ***"
            )
        }
        ConnectionStatus::Connected => panic!("expected auth failure"),
    }
}

#[tokio::test]
async fn openai_chat_done_terminates_model_call_without_http_eof() {
    let sse = [
        serde_json::json!({
            "id": "chatcmpl_1",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "bash",
                            "arguments": "{\"command\":\"pwd\"}"
                        }
                    }]
                },
                "finish_reason": null
            }]
        })
        .to_string(),
        serde_json::json!({
            "id": "chatcmpl_1",
            "choices": [{
                "index": 0,
                "delta": {},
                "finish_reason": "tool_calls"
            }]
        })
        .to_string(),
        "[DONE]".to_string(),
    ]
    .into_iter()
    .map(|event| format!("data: {event}\n\n"))
    .collect::<String>();
    let (base_url, release_server) = fake_open_sse_server(sse).await;
    let provider =
        OpenAiCompatibleProvider::new(openai_config(base_url, false, false)).expect("provider");
    let model = provider.create_chat_model("gpt-test").expect("chat model");
    let request = ChatRequest::streaming(vec![ModelDialogueItem::text(Role::User, "run pwd")]);

    let events: Vec<_> = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        model.chat_stream(request).collect(),
    )
    .await
    .expect("[DONE] must end the model call without waiting for HTTP EOF");
    let _ = release_server.send(());
    let events: Vec<_> = events.into_iter().collect::<Result<_, _>>().unwrap();

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::End {
            finish_reason: agent_types::FinishReason::ToolUse,
            ..
        }
    )));
}

#[tokio::test]
async fn openai_chat_error_event_terminates_stream_without_http_eof() {
    let sse = format!(
        "data: {}\n\n",
        serde_json::json!({
            "error": {
                "code": "stream_error",
                "message": "provider stream failed"
            }
        })
    );
    let (base_url, release_server) = fake_open_sse_server(sse).await;
    let provider =
        OpenAiCompatibleProvider::new(openai_config(base_url, false, false)).expect("provider");
    let model = provider.create_chat_model("gpt-test").expect("chat model");
    let request = ChatRequest::streaming(vec![ModelDialogueItem::text(Role::User, "hi")]);

    let events = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        model.chat_stream(request).collect::<Vec<_>>(),
    )
    .await
    .expect("stream error must terminate without waiting for HTTP EOF");
    let _ = release_server.send(());
    let events = events.into_iter().collect::<Result<Vec<_>, _>>().unwrap();

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::Error { code: Some(code), message, .. }
            if code == "stream_error" && message == "provider stream failed"
    )));
    assert!(!events
        .iter()
        .any(|event| matches!(event, AgentEvent::End { .. })));
}

#[tokio::test]
async fn openai_responses_connection_check_uses_provider_accepted_output_limit() {
    let (base_url, request) = fake_provider_server(
        200,
        r#"{
            "id":"resp-test",
            "model":"gpt-test",
            "output":[{"type":"message","content":[{"type":"output_text","text":"ok"}]}],
            "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}
        }"#,
    )
    .await;
    let provider = OpenAiResponsesProvider::new(openai_config(base_url, false, true))
        .expect("responses provider");

    assert!(provider
        .check_connection()
        .await
        .expect("connection status")
        .is_connected());

    let request = request.await.expect("captured request");
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/responses");
    let body: serde_json::Value = serde_json::from_str(&request.body).expect("json body");
    assert_eq!(body["model"], "configured-model");
    assert_eq!(body["max_output_tokens"], 16);
}

#[tokio::test]
async fn openai_responses_completed_event_terminates_stream_without_http_eof() {
    let capture_root = std::env::temp_dir().join(format!(
        "rua-openai-response-capture-{}",
        uuid::Uuid::new_v4()
    ));
    let address = rua_core::ChatAddress::agent("agent-capture", "chat-capture");
    debug_capture::register_capture(
        address.clone(),
        ActiveCapture {
            capture_id: "capture-1".to_string(),
            capture_path: capture_root.clone(),
        },
    );
    let sse = [
        serde_json::json!({
            "type": "response.output_text.delta",
            "delta": "captured text"
        }),
        serde_json::json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {
                "type": "function_call",
                "id": "fc_1",
                "call_id": "call_1",
                "name": "bash"
            }
        }),
        serde_json::json!({
            "type": "response.function_call_arguments.delta",
            "output_index": 0,
            "item_id": "fc_1",
            "delta": "{\"command\":\"pwd\"}"
        }),
        serde_json::json!({
            "type": "response.completed",
            "response": {
                "id": "response-session-1",
                "usage": {
                    "input_tokens": 10,
                    "output_tokens": 5,
                    "total_tokens": 15
                }
            }
        }),
    ]
    .into_iter()
    .map(|event| format!("data: {event}\n\n"))
    .collect::<String>();
    let (base_url, release_server) = fake_open_sse_server(sse).await;
    let mut config = openai_config(base_url, false, false);
    config.provider_id = "dashscope".into();
    config.provider_name = "DashScope".into();
    let provider = OpenAiResponsesProvider::new(config).expect("provider");
    let model = provider.create_chat_model("gpt-test").expect("chat model");
    let mut request = ChatRequest::streaming(vec![ModelDialogueItem::text(Role::User, "run pwd")]);
    request.metadata.insert(
        CHAT_ADDRESS_METADATA_KEY.to_string(),
        serde_json::to_value(&address).unwrap(),
    );
    request.metadata.insert(
        MODEL_CALL_ID_METADATA_KEY.to_string(),
        serde_json::json!("model-call-1"),
    );

    let events: Vec<_> = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        model.chat_stream(request).collect(),
    )
    .await
    .expect("response.completed must terminate the stream without waiting for HTTP EOF");
    let _ = release_server.send(());
    let events: Vec<_> = events.into_iter().collect::<Result<_, _>>().unwrap();

    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::End { .. })));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::End { metadata, .. }
            if metadata["rua_responses_session"]["response_id"] == "response-session-1"
    )));
    debug_capture::flush_capture(&capture_root, std::time::Duration::from_secs(5)).unwrap();
    let response_path = std::fs::read_dir(capture_root.join("calls/model-call-1"))
        .expect("capture call directory")
        .map(|entry| entry.expect("capture entry").path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("provider-response-"))
        })
        .expect("provider response capture");
    let response: serde_json::Value =
        serde_json::from_slice(&std::fs::read(response_path).unwrap()).unwrap();
    assert_eq!(response["body"]["completed"], true);
    assert_eq!(
        response["body"]["events"][0]["delta"],
        serde_json::json!("captured text")
    );
    debug_capture::unregister_capture(&address);
    debug_capture::flush_capture(&capture_root, std::time::Duration::from_secs(5)).unwrap();
    std::fs::remove_dir_all(capture_root).expect("cleanup capture");
}

#[tokio::test]
async fn openai_responses_incomplete_event_terminates_stream_without_http_eof() {
    let sse = format!(
        "data: {}\n\n",
        serde_json::json!({
            "type": "response.incomplete",
            "response": {
                "incomplete_details": {
                    "reason": "max_output_tokens"
                }
            }
        })
    );
    let (base_url, release_server) = fake_open_sse_server(sse).await;
    let provider =
        OpenAiResponsesProvider::new(openai_config(base_url, false, false)).expect("provider");
    let model = provider.create_chat_model("gpt-test").expect("chat model");
    let request = ChatRequest::streaming(vec![ModelDialogueItem::text(Role::User, "hi")]);

    let events = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        model.chat_stream(request).collect::<Vec<_>>(),
    )
    .await
    .expect("response.incomplete must terminate the stream without waiting for HTTP EOF");
    let _ = release_server.send(());
    let events = events.into_iter().collect::<Result<Vec<_>, _>>().unwrap();

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::Error { code: Some(code), message, .. }
            if code == "response_incomplete" && message.contains("max_output_tokens")
    )));
    assert!(!events
        .iter()
        .any(|event| matches!(event, AgentEvent::End { .. })));
}

#[tokio::test]
async fn gemini_check_connection_uses_models_endpoint() {
    let (base_url, request) = fake_provider_server(200, r#"{"models":[]}"#).await;
    let provider = GeminiProvider::new(GeminiConfig {
        provider_id: "gemini".into(),
        provider_name: "Gemini".into(),
        base_url,
        api_key: Some("gemini-key".into()),
        headers: serde_json::Map::new(),
        models: vec![model("gemini-test")],
        timeout_secs: 1,
        generate_kwargs: serde_json::Map::new(),
    })
    .expect("provider");

    assert!(provider
        .check_connection()
        .await
        .expect("connection status")
        .is_connected());

    let request = request.await.expect("captured request");
    assert_eq!(request.method, "GET");
    assert_eq!(request.path, "/v1beta/models");
    assert_eq!(
        request.headers.get("authorization").map(String::as_str),
        Some("Bearer gemini-key")
    );
}

#[tokio::test]
async fn gemini_auth_failure_is_connection_status_not_thrown_error() {
    let (base_url, _request) =
        fake_provider_server(401, r#"{"error":{"message":"invalid gemini key"}}"#).await;
    let provider = GeminiProvider::new(GeminiConfig {
        provider_id: "gemini".into(),
        provider_name: "Gemini".into(),
        base_url,
        api_key: Some("gemini-key".into()),
        headers: serde_json::Map::new(),
        models: vec![model("gemini-test")],
        timeout_secs: 1,
        generate_kwargs: serde_json::Map::new(),
    })
    .expect("provider");

    let status = provider.check_connection().await.expect("status result");
    match status {
        ConnectionStatus::Failed(message) => assert_eq!(
            message,
            "auth failed for provider gemini (HTTP 401): invalid gemini key"
        ),
        ConnectionStatus::Connected => panic!("expected auth failure"),
    }
}

#[tokio::test]
async fn gemini_fetch_models_normalizes_discovered_models() {
    let (base_url, request) = fake_provider_server(
        200,
        r#"{"models":[
            {
                "name":"models/gemini-2.5-flash",
                "displayName":"Gemini 2.5 Flash",
                "inputTokenLimit":1048576,
                "outputTokenLimit":8192
            },
            {"name":"models/gemini-2.5-flash","displayName":"duplicate"},
            {"name":"models/gemini-2.5-pro","displayName":""},
            {"name":"models/gemini-2.0-flash","displayName":"models/gemini-2.0-flash"},
            {"name":"   ","displayName":"invalid"}
        ]}"#,
    )
    .await;
    let provider = GeminiProvider::new(GeminiConfig {
        provider_id: "gemini".into(),
        provider_name: "Gemini".into(),
        base_url,
        api_key: Some("gemini-key".into()),
        headers: serde_json::Map::new(),
        models: vec![],
        timeout_secs: 1,
        generate_kwargs: serde_json::Map::new(),
    })
    .expect("provider");

    let models = provider.fetch_models().await.expect("discovered models");

    assert_eq!(models.len(), 3);
    assert_eq!(models[0].id, "gemini-2.5-flash");
    assert_eq!(models[0].name, "Gemini 2.5 Flash");
    assert_eq!(models[0].context_window, Some(1_048_576));
    assert_eq!(models[0].max_output_tokens, Some(8192));
    assert_eq!(models[0].probe_source.as_deref(), Some("api"));
    assert_eq!(models[1].id, "gemini-2.5-pro");
    assert_eq!(models[1].name, "gemini-2.5-pro");
    assert_eq!(models[2].id, "gemini-2.0-flash");
    assert_eq!(models[2].name, "gemini-2.0-flash");
    assert!(models
        .iter()
        .all(|model| model.probe_source.as_deref() == Some("api")));

    let request = request.await.expect("captured request");
    assert_eq!(request.method, "GET");
    assert_eq!(request.path, "/v1beta/models");
    assert_eq!(
        request.headers.get("authorization").map(String::as_str),
        Some("Bearer gemini-key")
    );
}

#[tokio::test]
async fn gemini_fetch_models_follows_pages_despite_presets_and_filters_non_chat_models() {
    let (base_url, requests) = fake_provider_pages(vec![
        (200, r#"{"models":[{"name":"models/gemini-3-flash-preview","supportedGenerationMethods":["generateContent"]},{"name":"models/embedding","supportedGenerationMethods":["embedContent"]}],"nextPageToken":"page+2"}"#),
        (200, r#"{"models":[{"name":"models/gemini-3-flash-preview"},{"name":"models/gemini-3.1-pro-preview","inputTokenLimit":1048576,"outputTokenLimit":65536}]}"#),
    ]).await;
    let provider = GeminiProvider::new(GeminiConfig {
        provider_id: "google".into(),
        provider_name: "Google".into(),
        base_url,
        api_key: Some("gemini-key".into()),
        headers: serde_json::Map::new(),
        models: vec![model("old-preset")],
        timeout_secs: 1,
        generate_kwargs: serde_json::Map::new(),
    })
    .unwrap();
    let models = provider.fetch_models().await.unwrap();
    assert_eq!(
        models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        ["gemini-3-flash-preview", "gemini-3.1-pro-preview"]
    );
    assert_eq!(models[1].max_output_tokens, Some(65536));
    for (request, expected) in requests
        .into_iter()
        .zip(["/v1beta/models", "/v1beta/models?pageToken=page%2B2"])
    {
        let request = request.await.unwrap();
        assert_eq!(request.path, expected);
        assert_eq!(
            request.headers.get("authorization").map(String::as_str),
            Some("Bearer gemini-key")
        );
    }
}

#[tokio::test]
async fn anthropic_fetch_models_follows_pages_and_uses_configured_auth_mode() {
    for auth_mode in [
        llm_multiplex::providers::anthropic::AuthMode::ApiKey,
        llm_multiplex::providers::anthropic::AuthMode::AuthToken,
    ] {
        let (base_url, requests) = fake_provider_pages(vec![
            (200, r#"{"data":[{"id":"claude-opus-4-8","display_name":"Opus","context_window":1000000}],"has_more":true,"last_id":"cursor+1"}"#),
            (200, r#"{"data":[{"id":"claude-opus-4-8"},{"id":"claude-sonnet-5","max_output_tokens":64000}],"has_more":false}"#),
        ]).await;
        let provider = AnthropicProvider::new(AnthropicConfig {
            base_url,
            api_key: Some("anthropic-key".into()),
            models: vec![model("old-preset")],
            timeout_secs: 1,
            auth_mode,
            ..Default::default()
        })
        .unwrap();
        let models = provider.fetch_models().await.unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].name, "Opus");
        assert_eq!(models[0].context_window, Some(1_000_000));
        assert_eq!(models[1].max_output_tokens, Some(64000));
        for (request, expected) in requests.into_iter().zip([
            "/v1/models?limit=100",
            "/v1/models?limit=100&after_id=cursor%2B1",
        ]) {
            let request = request.await.unwrap();
            assert_eq!(request.path, expected);
            assert_eq!(
                request.headers.get("anthropic-version").map(String::as_str),
                Some("2023-06-01")
            );
            match auth_mode {
                llm_multiplex::providers::anthropic::AuthMode::ApiKey => assert_eq!(
                    request.headers.get("x-api-key").map(String::as_str),
                    Some("anthropic-key")
                ),
                llm_multiplex::providers::anthropic::AuthMode::AuthToken => {
                    assert_eq!(
                        request.headers.get("authorization").map(String::as_str),
                        Some("Bearer anthropic-key")
                    );
                    assert!(!request.headers.contains_key("x-api-key"));
                }
            }
        }
    }
}

#[tokio::test]
async fn native_model_discovery_rejects_repeated_pagination_cursors() {
    let (base_url, _) = fake_provider_pages(vec![
        (200, r#"{"data":[],"has_more":true,"last_id":"same"}"#),
        (200, r#"{"data":[],"has_more":true,"last_id":"same"}"#),
    ])
    .await;
    let anthropic = AnthropicProvider::new(AnthropicConfig {
        base_url,
        timeout_secs: 1,
        ..Default::default()
    })
    .unwrap();
    assert!(matches!(
        anthropic.fetch_models().await,
        Err(LlmError::InvalidResponse { .. })
    ));
    let (base_url, _) = fake_provider_pages(vec![
        (200, r#"{"models":[],"nextPageToken":"same"}"#),
        (200, r#"{"models":[],"nextPageToken":"same"}"#),
    ])
    .await;
    let gemini = GeminiProvider::new(GeminiConfig {
        provider_id: "google".into(),
        provider_name: "Google".into(),
        base_url,
        api_key: None,
        headers: serde_json::Map::new(),
        models: vec![],
        timeout_secs: 1,
        generate_kwargs: serde_json::Map::new(),
    })
    .unwrap();
    assert!(matches!(
        gemini.fetch_models().await,
        Err(LlmError::InvalidResponse { .. })
    ));
}

#[tokio::test]
async fn anthropic_check_connection_posts_minimal_ping() {
    let (base_url, request) = fake_provider_server(200, r#"{"id":"msg-test"}"#).await;
    let provider = AnthropicProvider::new(AnthropicConfig {
        provider_id: "anthropic".into(),
        provider_name: "Anthropic".into(),
        base_url,
        api_key: Some("anthropic-key".into()),
        headers: serde_json::Map::new(),
        models: vec![model("claude-test")],
        timeout_secs: 1,
        generate_kwargs: serde_json::Map::new(),
        support_connection_check: true,
        api_version: "2023-06-01".into(),
        auth_mode: Default::default(),
    })
    .expect("provider");

    assert!(provider
        .check_connection()
        .await
        .expect("connection status")
        .is_connected());

    let request = request.await.expect("captured request");
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/v1/messages");
    assert_eq!(
        request.headers.get("x-api-key").map(String::as_str),
        Some("anthropic-key")
    );
    assert_eq!(
        request.headers.get("anthropic-version").map(String::as_str),
        Some("2023-06-01")
    );

    let body: serde_json::Value = serde_json::from_str(&request.body).expect("json body");
    assert_eq!(body["model"], "claude-test");
    assert_eq!(body["max_tokens"], 1);
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(body["messages"][0]["content"], "ping");
}

#[tokio::test]
async fn anthropic_auth_failure_is_connection_status_not_thrown_error() {
    let (base_url, _request) = fake_provider_server(
        401,
        r#"{"error":{"message":"invalid key sk-ant-test-secret-secret"}}"#,
    )
    .await;
    let provider = AnthropicProvider::new(AnthropicConfig {
        provider_id: "anthropic".into(),
        provider_name: "Anthropic".into(),
        base_url,
        api_key: Some("anthropic-key".into()),
        headers: serde_json::Map::new(),
        models: vec![model("claude-test")],
        timeout_secs: 1,
        generate_kwargs: serde_json::Map::new(),
        support_connection_check: true,
        api_version: "2023-06-01".into(),
        auth_mode: Default::default(),
    })
    .expect("provider");

    let status = provider.check_connection().await.expect("status result");
    match status {
        ConnectionStatus::Failed(message) => assert_eq!(message, "authentication failed"),
        ConnectionStatus::Connected => panic!("expected auth failure"),
    }
}

#[tokio::test]
async fn anthropic_message_stop_terminates_stream_and_emits_one_tool_call_representation() {
    let body = [
        (
            "content_block_start",
            serde_json::json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {
                    "type": "tool_use",
                    "id": "call_1",
                    "name": "bash",
                    "input": {}
                }
            }),
        ),
        (
            "content_block_delta",
            serde_json::json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {
                    "type": "input_json_delta",
                    "partial_json": "{\"command\":\"pwd\"}"
                }
            }),
        ),
        (
            "content_block_stop",
            serde_json::json!({
                "type": "content_block_stop",
                "index": 0
            }),
        ),
        (
            "message_delta",
            serde_json::json!({
                "type": "message_delta",
                "delta": {"stop_reason": "tool_use"},
                "usage": {"output_tokens": 5}
            }),
        ),
        ("message_stop", serde_json::json!({"type": "message_stop"})),
    ]
    .into_iter()
    .map(|(event, data)| format!("event: {event}\ndata: {data}\n\n"))
    .collect::<String>();
    let (base_url, release_server) = fake_open_sse_server(body).await;
    let provider = AnthropicProvider::new(AnthropicConfig {
        provider_id: "anthropic".into(),
        provider_name: "Anthropic".into(),
        base_url,
        api_key: Some("anthropic-key".into()),
        headers: serde_json::Map::new(),
        models: vec![model("claude-test")],
        timeout_secs: 1,
        generate_kwargs: serde_json::Map::new(),
        support_connection_check: true,
        api_version: "2023-06-01".into(),
        auth_mode: Default::default(),
    })
    .expect("provider");
    let model = provider
        .create_chat_model("claude-test")
        .expect("chat model");
    let request = ChatRequest::streaming(vec![ModelDialogueItem::text(Role::User, "run pwd")]);

    let events = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        model.chat_stream(request).collect::<Vec<_>>(),
    )
    .await
    .expect("message_stop must terminate without waiting for HTTP EOF");
    let _ = release_server.send(());
    let events = events.into_iter().collect::<Result<Vec<_>, _>>().unwrap();

    assert!(!events
        .iter()
        .any(|event| matches!(event, AgentEvent::ToolCall { .. })));
    let deltas: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ToolCallDelta {
                id,
                arguments_delta,
                ..
            } => Some((id.as_str(), arguments_delta.as_str())),
            _ => None,
        })
        .collect();
    assert!(deltas.iter().all(|(id, _)| *id == "call_1"));
    assert_eq!(
        deltas
            .iter()
            .map(|(_, arguments)| *arguments)
            .collect::<String>(),
        r#"{"command":"pwd"}"#
    );
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::End {
            finish_reason: agent_types::FinishReason::ToolUse,
            ..
        }
    )));
}

#[tokio::test]
async fn anthropic_error_event_terminates_stream_without_http_eof() {
    let body = concat!(
        "event: error\n",
        "data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"provider overloaded\"}}\n\n"
    )
    .to_string();
    let (base_url, release_server) = fake_open_sse_server(body).await;
    let provider = AnthropicProvider::new(AnthropicConfig {
        provider_id: "anthropic".into(),
        provider_name: "Anthropic".into(),
        base_url,
        api_key: Some("anthropic-key".into()),
        headers: serde_json::Map::new(),
        models: vec![model("claude-test")],
        timeout_secs: 1,
        generate_kwargs: serde_json::Map::new(),
        support_connection_check: true,
        api_version: "2023-06-01".into(),
        auth_mode: Default::default(),
    })
    .expect("provider");
    let model = provider
        .create_chat_model("claude-test")
        .expect("chat model");
    let request = ChatRequest::streaming(vec![ModelDialogueItem::text(Role::User, "hi")]);

    let events = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        model.chat_stream(request).collect::<Vec<_>>(),
    )
    .await
    .expect("Anthropic error must terminate without waiting for HTTP EOF");
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

#[tokio::test]
async fn gemini_finish_reason_terminates_stream_without_http_eof() {
    let body = format!(
        "data: {}\n\n",
        serde_json::json!({
            "candidates": [{
                "content": {"parts": [{"text": "done"}]},
                "finishReason": "STOP"
            }]
        })
    );
    let (base_url, release_server) = fake_open_sse_server(body).await;
    let provider = GeminiProvider::new(GeminiConfig {
        provider_id: "gemini".into(),
        provider_name: "Gemini".into(),
        base_url,
        api_key: Some("gemini-key".into()),
        headers: serde_json::Map::new(),
        models: vec![model("gemini-test")],
        timeout_secs: 1,
        generate_kwargs: serde_json::Map::new(),
    })
    .expect("provider");
    let model = provider
        .create_chat_model("gemini-test")
        .expect("chat model");
    let request = ChatRequest::streaming(vec![ModelDialogueItem::text(Role::User, "hi")]);

    let events = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        model.chat_stream(request).collect::<Vec<_>>(),
    )
    .await
    .expect("finishReason must terminate without waiting for HTTP EOF");
    let _ = release_server.send(());
    let events = events.into_iter().collect::<Result<Vec<_>, _>>().unwrap();

    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::End { .. })));
}

#[tokio::test]
async fn ollama_done_terminates_stream_without_http_eof() {
    let body = format!(
        "{}\n",
        serde_json::json!({
            "message": {"role": "assistant", "content": "done"},
            "done": true
        })
    );
    let (base_url, release_server) = fake_open_sse_server(body).await;
    let provider = OllamaProvider::new(ollama_config(base_url)).expect("provider");
    let model = provider
        .create_chat_model("qwen2.5:latest")
        .expect("chat model");
    let request = ChatRequest::streaming(vec![ModelDialogueItem::text(Role::User, "hi")]);

    let events = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        model.chat_stream(request).collect::<Vec<_>>(),
    )
    .await
    .expect("done=true must terminate without waiting for HTTP EOF");
    let _ = release_server.send(());
    let events = events.into_iter().collect::<Result<Vec<_>, _>>().unwrap();

    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::End { .. })));
}

#[tokio::test]
async fn ollama_error_event_terminates_stream_without_http_eof() {
    let body = format!(
        "{}\n",
        serde_json::json!({
            "error": "model runner failed"
        })
    );
    let (base_url, release_server) = fake_open_sse_server(body).await;
    let provider = OllamaProvider::new(ollama_config(base_url)).expect("provider");
    let model = provider
        .create_chat_model("qwen2.5:latest")
        .expect("chat model");
    let request = ChatRequest::streaming(vec![ModelDialogueItem::text(Role::User, "hi")]);

    let events = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        model.chat_stream(request).collect::<Vec<_>>(),
    )
    .await
    .expect("Ollama error must terminate without waiting for HTTP EOF");
    let _ = release_server.send(());
    let events = events.into_iter().collect::<Result<Vec<_>, _>>().unwrap();

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::Error { message, .. } if message == "model runner failed"
    )));
    assert!(!events
        .iter()
        .any(|event| matches!(event, AgentEvent::End { .. })));
}

#[tokio::test]
async fn ollama_fetch_models_normalizes_base_url_before_native_api_paths() {
    for suffix in ["", "/", "/v1", "/v1/"] {
        let (base_url, request) =
            fake_provider_server(200, r#"{"models":[{"name":"qwen2.5:latest"}]}"#).await;
        let provider =
            OllamaProvider::new(ollama_config(format!("{base_url}{suffix}"))).expect("provider");

        let models = provider.fetch_models().await.expect("discovered models");

        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "qwen2.5:latest");
        assert_eq!(models[0].probe_source.as_deref(), Some("ollama-api"));

        let request = request.await.expect("captured request");
        assert_eq!(request.method, "GET");
        assert_eq!(
            request.path, "/api/tags",
            "Ollama base_url suffix {suffix:?} must not leak an OpenAI-style /v1 into native paths"
        );
    }
}
