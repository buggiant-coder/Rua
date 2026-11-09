use agent_types::{AgentEvent, ChatRequest, ModelDialogueItem, Role, ToolDefinition};
use futures::StreamExt;
use llm_multiplex::providers::gemini::GeminiConfig;
use llm_multiplex::traits::Provider;
use rua_provider::idealab_vertex::IdealabVertexProvider;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

async fn fake_gemini_sse_server(body: String) -> (String, oneshot::Sender<()>) {
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

#[tokio::test]
async fn finish_reason_terminates_stream_without_waiting_for_http_eof() {
    let sse = concat!(
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"functionCall\":{\"name\":\"bash\",\"args\":{\"command\":\"pwd\"}},\"thoughtSignature\":\"gemini-call-1\"}]},\"finishReason\":\"STOP\"}]}\n\n",
        "data: [DONE]\n\n"
    )
    .to_string();
    let (base_url, release_server) = fake_gemini_sse_server(sse).await;
    let provider = IdealabVertexProvider::new(GeminiConfig {
        provider_id: "idealab-gemini-test".into(),
        provider_name: "Test".into(),
        base_url,
        api_key: Some("test-key".into()),
        models: Vec::new(),
        timeout_secs: 5,
        headers: Default::default(),
        generate_kwargs: Default::default(),
    })
    .unwrap();
    let model = provider.create_chat_model("gemini-test").unwrap();
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
    .expect("finishReason must terminate the stream without waiting for HTTP EOF");
    let _ = release_server.send(());
    let events: Vec<_> = events.into_iter().collect::<Result<_, _>>().unwrap();

    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::ToolCall { .. })));
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::End { .. })));
}
