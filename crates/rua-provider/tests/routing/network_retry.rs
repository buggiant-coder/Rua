use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_types::{AgentEvent, ChatRequest, ModelDialogueItem, Role};
use futures::StreamExt;
use llm_multiplex::{
    ChatModel, ModelInfo, OpenAiCompatibleConfig, OpenAiCompatibleProvider, Provider,
};
use rua_provider::{RetryChatModel, RetryPolicy};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

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

async fn read_request(socket: &mut tokio::net::TcpStream) {
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
                content_length = parse_content_length(&String::from_utf8_lossy(&bytes[..end]));
            }
        }
        if header_end.is_some_and(|end| bytes.len() >= end + 4 + content_length) {
            break;
        }
    }
}

async fn flaky_provider_server(delay: Duration) -> (String, Arc<AtomicU32>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake provider");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
    let attempts = Arc::new(AtomicU32::new(0));
    let server_attempts = Arc::clone(&attempts);

    tokio::spawn(async move {
        for attempt in 0..2 {
            let (mut socket, _) = listener.accept().await.expect("accept request");
            server_attempts.fetch_add(1, Ordering::SeqCst);
            read_request(&mut socket).await;
            tokio::time::sleep(delay).await;
            if attempt == 0 {
                continue;
            }

            let body = concat!(
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"RECOVERED\"},",
                "\"finish_reason\":null}]}\n\n",
                "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n"
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write response");
        }
    });

    (base_url, attempts)
}

fn provider_config(base_url: String) -> OpenAiCompatibleConfig {
    OpenAiCompatibleConfig {
        provider_id: "network-retry".into(),
        provider_name: "Network Retry".into(),
        base_url,
        api_key: None,
        api_keys: Vec::new(),
        headers: serde_json::Map::new(),
        models: vec![ModelInfo {
            id: "test-model".into(),
            name: "test-model".into(),
            context_window: None,
            max_output_tokens: None,
            supports_multimodal: None,
            supports_image: None,
            supports_video: None,
            probe_source: None,
            generate_kwargs: serde_json::Map::new(),
        }],
        timeout_secs: 1,
        generate_kwargs: serde_json::Map::new(),
        support_model_discovery: false,
        support_connection_check: false,
        is_local: false,
    }
}

#[tokio::test]
async fn retries_disconnect_before_headers_and_recovers_on_next_attempt() {
    let (base_url, attempts) = flaky_provider_server(Duration::ZERO).await;
    let provider = OpenAiCompatibleProvider::new(provider_config(base_url)).expect("provider");
    let model = provider
        .create_chat_model("test-model")
        .expect("chat model");
    let retry = RetryChatModel::new(
        model,
        RetryPolicy {
            max_retries: 1,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(1),
            backoff_factor: 1.0,
            max_total_time: None,
        },
    );
    let events = retry
        .chat_stream(ChatRequest::streaming(vec![ModelDialogueItem::text(
            Role::User,
            "hello",
        )]))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("retry must recover");

    let text = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::TextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect::<String>();
    let starts = events.iter().filter(|event| event.is_start()).count();

    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(starts, 1, "retry must suppress the duplicate Start event");
    assert_eq!(text, "RECOVERED");
    assert!(events.iter().any(AgentEvent::is_terminal));
}

#[tokio::test]
async fn slow_disconnect_and_backoff_leave_a_full_timeout_for_the_next_request() {
    let (base_url, attempts) = flaky_provider_server(Duration::from_millis(700)).await;
    let provider = OpenAiCompatibleProvider::new(provider_config(base_url)).unwrap();
    let retry = RetryChatModel::new(
        provider.create_chat_model("test-model").unwrap(),
        RetryPolicy {
            max_retries: 1,
            initial_delay: Duration::from_millis(600),
            max_delay: Duration::from_millis(600),
            backoff_factor: 1.0,
            max_total_time: None,
        },
    );
    let events = retry
        .chat_stream_with_idle_timeout(
            ChatRequest::streaming(vec![ModelDialogueItem::text(Role::User, "hello")]),
            Duration::from_secs(1),
        )
        .collect::<Vec<_>>()
        .await;
    assert!(events.iter().all(Result::is_ok), "{events:?}");
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, Ok(AgentEvent::Start { .. })))
            .count(),
        1
    );
    assert!(events.iter().any(
        |event| matches!(event, Ok(AgentEvent::TextDelta { delta, .. })
        if delta == "RECOVERED")
    ));
    assert!(events
        .iter()
        .any(|event| matches!(event, Ok(AgentEvent::End { .. }))));
}
