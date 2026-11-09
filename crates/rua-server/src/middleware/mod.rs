mod agent_context;

pub use agent_context::agent_context_middleware;

pub use crate::extractors::AgentExtractorContext;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures::FutureExt;

use uuid::Uuid;

pub const REQUEST_ID_HEADER: &str = "x-request-id";

pub async fn request_id_middleware(mut req: Request<Body>, next: Next) -> Response {
    let request_id = req
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    if let Ok(val) = request_id.parse() {
        req.headers_mut().insert(REQUEST_ID_HEADER, val);
    }

    req.extensions_mut().insert(RequestId(request_id.clone()));

    let mut response = next.run(req).await;

    if let Ok(val) = request_id.parse() {
        response.headers_mut().insert(REQUEST_ID_HEADER, val);
    }

    response
}

#[derive(Debug, Clone)]
pub struct RequestId(pub String);

pub async fn panic_catcher_middleware(req: Request<Body>, next: Next) -> Response {
    let is_api = req.uri().path().starts_with("/api/");
    let path = req.uri().path().to_string();
    let method = req.method().clone();

    match std::panic::AssertUnwindSafe(next.run(req))
        .catch_unwind()
        .await
    {
        Ok(response) => response,
        Err(payload) => {
            let message = panic_payload_to_string(&payload);
            tracing::error!(
                path = %path,
                method = %method,
                panic.message = %message,
                "handler panicked"
            );
            if is_api {
                let body = serde_json::json!({ "detail": "Internal server error" });
                (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error").into_response()
            }
        }
    }
}

fn panic_payload_to_string(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

fn is_quiet_path(path: &str) -> bool {
    path == "/healthz"
        || path == "/readyz"
        || path.starts_with("/assets/")
        || path == "/rua-symbol.svg"
        || path.ends_with("/console/debug/backend-logs")
        || path.ends_with("/console/push-messages")
        || path.ends_with("/console/recent_messages")
}

pub async fn request_logging_middleware(req: Request<Body>, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let started = std::time::Instant::now();

    let response = next.run(req).await;

    let status = response.status().as_u16();
    let elapsed_ms = started.elapsed().as_millis();
    if is_quiet_path(&path) {
        tracing::trace!(method = %method, path = %path, status, elapsed_ms, "request");
    } else {
        tracing::info!(method = %method, path = %path, status, elapsed_ms, "request");
    }
    response
}

pub fn cors_layer() -> tower_http::cors::CorsLayer {
    use axum::http::header;
    use tower_http::cors;

    cors::CorsLayer::new()
        .allow_origin(cors::Any)
        .allow_methods(cors::Any)
        .allow_headers([
            header::CONTENT_TYPE,
            header::AUTHORIZATION,
            header::ACCEPT,
            header::HeaderName::from_static("x-agent-id"),
            header::HeaderName::from_static("x-request-id"),
            header::HeaderName::from_static("x-api-token"),
        ])
        .expose_headers([header::HeaderName::from_static("x-request-id")])
        .max_age(std::time::Duration::from_secs(3600))
}

pub const DEFAULT_BODY_LIMIT: usize = 10 * 1024 * 1024;

pub fn sse_headers() -> [(String, String); 4] {
    [
        ("content-type".to_string(), "text/event-stream".to_string()),
        ("cache-control".to_string(), "no-cache".to_string()),
        ("connection".to_string(), "keep-alive".to_string()),
        ("x-accel-buffering".to_string(), "no".to_string()),
    ]
}
