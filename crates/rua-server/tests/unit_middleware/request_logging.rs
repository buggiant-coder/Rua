use std::io::Write;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Method, Request};
use axum::middleware;
use axum::routing::get;
use axum::Router;
use rua_server::middleware::request_logging_middleware;
use tower::ServiceExt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Layer;

#[derive(Clone, Default)]
struct CapturedWriter {
    chunks: Arc<Mutex<Vec<String>>>,
}

impl CapturedWriter {
    fn output(&self) -> String {
        self.chunks.lock().unwrap().join("")
    }
}

impl Write for CapturedWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.chunks
            .lock()
            .unwrap()
            .push(String::from_utf8_lossy(buf).to_string());
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

async fn exercise_logged_path(path: &str) -> String {
    let writer = CapturedWriter::default();
    let captured = writer.clone();
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .with_filter(tracing_subscriber::filter::LevelFilter::INFO),
    );

    let _guard = tracing::subscriber::set_default(subscriber);
    let app = Router::new()
        .route("/normal", get(|| async { "ok" }))
        .route("/healthz", get(|| async { "ok" }))
        .route(
            "/api/agents/default/console/push-messages",
            get(|| async { "ok" }),
        )
        .layer(middleware::from_fn(request_logging_middleware));

    app.oneshot(
        Request::builder()
            .method(Method::GET)
            .uri(path)
            .body(Body::empty())
            .expect("request"),
    )
    .await
    .expect("response");

    captured.output()
}

#[tokio::test]
async fn request_logging_records_normal_api_paths_at_info() {
    let output = exercise_logged_path("/normal").await;

    assert!(output.contains("request"), "got logs:\n{output}");
    assert!(output.contains("method=GET"), "got logs:\n{output}");
    assert!(output.contains("path=/normal"), "got logs:\n{output}");
    assert!(output.contains("status=200"), "got logs:\n{output}");
}

#[tokio::test]
async fn request_logging_keeps_health_and_polling_paths_out_of_info_logs() {
    for path in [
        "/healthz",
        "/rua-symbol.svg",
        "/api/agents/default/console/push-messages",
    ] {
        let output = exercise_logged_path(path).await;

        assert!(
            output.trim().is_empty(),
            "quiet path {path} should not emit info access logs, got:\n{output}"
        );
    }
}
