use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use async_trait::async_trait;
use rua_memory::{Embedder, EmbeddingTransport, HttpEmbeddingTransport, LlmEmbedder, MemoryError};
use serde_json::{json, Value};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

async fn start_embedding_endpoint(response: Value) -> (String, tokio::task::JoinHandle<String>) {
    start_embedding_endpoint_with_status(200, response.to_string()).await
}

async fn start_embedding_endpoint_with_status(
    status: u16,
    body: String,
) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind embedding endpoint");
    let addr = listener.local_addr().expect("local addr");
    let handle = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept request");
        let mut buf = Vec::new();
        let header_end = loop {
            let mut chunk = [0_u8; 1024];
            let n = stream.read(&mut chunk).await.expect("read request");
            assert!(n > 0, "client closed before headers");
            buf.extend_from_slice(&chunk[..n]);
            if let Some(pos) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
                break pos + 4;
            }
        };

        let headers = String::from_utf8_lossy(&buf[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.split_once(':').and_then(|(name, value)| {
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().expect("content length"))
                })
            })
            .unwrap_or(0);

        while buf.len() < header_end + content_length {
            let mut chunk = [0_u8; 1024];
            let n = stream.read(&mut chunk).await.expect("read body");
            assert!(n > 0, "client closed before body");
            buf.extend_from_slice(&chunk[..n]);
        }

        let reason = match status {
            200 => "OK",
            401 => "Unauthorized",
            429 => "Too Many Requests",
            _ => "Error",
        };
        let http = format!(
            "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(http.as_bytes())
            .await
            .expect("write response");
        String::from_utf8(buf).expect("utf8 request")
    });

    (format!("http://{addr}/"), handle)
}

#[tokio::test]
async fn http_embedding_transport_trims_base_url_and_omits_auth_when_key_absent() {
    let (base_url, handle) = start_embedding_endpoint(json!({
        "data": [{"embedding": [1.0, 0.0]}],
    }))
    .await;
    let transport = HttpEmbeddingTransport::new(&base_url, None);

    let response = transport
        .post_embedding(&json!({ "model": "memory-embed", "input": ["hello"] }))
        .await
        .expect("embedding response");

    assert_eq!(response["data"][0]["embedding"][0], 1.0);
    let raw_request = handle.await.expect("request capture");
    assert!(raw_request.starts_with("POST /embeddings HTTP/1.1"));
    assert!(
        !raw_request.to_ascii_lowercase().contains("authorization:"),
        "blank/absent embedding API keys must not emit an Authorization header"
    );
}

#[tokio::test]
async fn http_embedding_transport_sends_bearer_auth_when_key_present() {
    let (base_url, handle) = start_embedding_endpoint(json!({
        "data": [{"embedding": [0.5, 0.5]}],
    }))
    .await;
    let transport =
        HttpEmbeddingTransport::new(base_url.trim_end_matches('/'), Some("sk-memory".into()));

    transport
        .post_embedding(&json!({ "model": "memory-embed", "input": ["hello"] }))
        .await
        .expect("embedding response");

    let raw_request = handle.await.expect("request capture");
    assert!(raw_request.contains("authorization: Bearer sk-memory"));
}

#[tokio::test]
async fn http_embedding_transport_reports_non_success_status_and_body() {
    let (base_url, _handle) = start_embedding_endpoint_with_status(
        429,
        json!({"error": {"message": "embedding quota exceeded"}}).to_string(),
    )
    .await;
    let transport = HttpEmbeddingTransport::new(&base_url, Some("sk-memory".into()));

    let err = transport
        .post_embedding(&json!({ "model": "memory-embed", "input": ["hello"] }))
        .await
        .expect_err("non-success response should fail");

    assert!(err.contains("429 Too Many Requests"), "got {err}");
    assert!(err.contains("embedding quota exceeded"), "got {err}");
}

#[tokio::test]
async fn http_embedding_transport_reports_invalid_json_response() {
    let (base_url, _handle) =
        start_embedding_endpoint_with_status(200, "not-json".to_string()).await;
    let transport = HttpEmbeddingTransport::new(&base_url, None);

    let err = transport
        .post_embedding(&json!({ "model": "memory-embed", "input": ["hello"] }))
        .await
        .expect_err("invalid json should fail");

    assert!(
        err.contains("error decoding response body"),
        "invalid JSON parse error should be visible, got {err}"
    );
}

#[derive(Default)]
struct RecordingTransport {
    calls: AtomicUsize,
    bodies: Mutex<Vec<Value>>,
}

#[async_trait]
impl EmbeddingTransport for RecordingTransport {
    async fn post_embedding(&self, body: &Value) -> Result<Value, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.bodies.lock().expect("lock bodies").push(body.clone());
        Ok(json!({
            "data": [
                {"embedding": [0.1, 0.2]},
                {"embedding": [0.3, 0.4]}
            ]
        }))
    }
}

#[tokio::test]
async fn llm_embedder_skips_transport_for_empty_batches() {
    let transport = Arc::new(RecordingTransport::default());
    let embedder = LlmEmbedder::new(transport.clone(), "memory-embed", 2);

    let vectors = embedder.embed(&[]).await.expect("empty embedding batch");

    assert!(vectors.is_empty());
    assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn llm_embedder_posts_model_and_inputs_then_parses_vectors() {
    let transport = Arc::new(RecordingTransport::default());
    let embedder = LlmEmbedder::new(transport.clone(), "memory-embed", 2);

    let vectors = embedder
        .embed(&["alpha".to_string(), "beta".to_string()])
        .await
        .expect("embedding vectors");

    assert_eq!(vectors, vec![vec![0.1, 0.2], vec![0.3, 0.4]]);
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    let bodies = transport.bodies.lock().expect("lock bodies");
    assert_eq!(bodies[0]["model"], "memory-embed");
    assert_eq!(bodies[0]["input"], json!(["alpha", "beta"]));
}

struct MalformedTransport;

#[async_trait]
impl EmbeddingTransport for MalformedTransport {
    async fn post_embedding(&self, _body: &Value) -> Result<Value, String> {
        Ok(json!({"data": [{"object": "embedding"}]}))
    }
}

#[tokio::test]
async fn llm_embedder_reports_missing_embedding_items() {
    let embedder = LlmEmbedder::new(Arc::new(MalformedTransport), "memory-embed", 2);

    let err = embedder
        .embed(&["alpha".to_string()])
        .await
        .expect_err("missing embedding should fail");

    assert!(
        matches!(err, MemoryError::Embedding(ref message) if message.contains("missing 'embedding' for item 0")),
        "unexpected error: {err}"
    );
}
