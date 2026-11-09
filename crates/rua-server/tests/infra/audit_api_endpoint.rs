use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::audit;
use rua_server::middleware::agent_context_middleware;
use rua_server::AppState;

fn build_app() -> (Router, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed workspace");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    let app = Router::new()
        .nest("/api", audit::router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state);
    (app, tmp)
}

fn seed_audit_events(data_dir: &std::path::Path, count: usize) -> Vec<String> {
    let audit_dir = data_dir.join("audit");
    std::fs::create_dir_all(&audit_dir).unwrap();

    let today = chrono::Utc::now().format("%Y-%m-%d");
    let file_path = audit_dir.join(format!("audit-{today}.jsonl"));

    let mut timestamps = Vec::new();
    let mut lines = Vec::new();

    for i in 0..count {
        let ts = format!("2025-05-13T10:{:02}:00Z", 10 + i);
        timestamps.push(ts.clone());
        let entry = serde_json::json!({
            "timestamp": ts,
            "message": "security_audit_event",
            "kind": "ToolGuardEvaluation",
            "agent_id": "default",
            "session_id": "sess-1",
            "tool_name": format!("tool_{i}"),
            "decision": "Allow",
            "finding_count": 0,
        });
        lines.push(serde_json::to_string(&entry).unwrap());
    }

    std::fs::write(&file_path, lines.join("\n") + "\n").unwrap();
    timestamps
}

async fn read_json(resp: axum::response::Response) -> Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        panic!(
            "non-JSON body: {} (body={})",
            e,
            String::from_utf8_lossy(&bytes)
        )
    })
}

fn req(method: Method, uri: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .expect("request")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audit_endpoint_returns_entries_with_limit() {
    let (app, tmp) = build_app();
    let _timestamps = seed_audit_events(tmp.path(), 5);

    let resp = app
        .oneshot(req(Method::GET, "/api/security/audit?limit=3"))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let entries = body.as_array().expect("response should be array");
    assert_eq!(entries.len(), 3, "limit=3 should return 3 entries");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audit_endpoint_filters_by_since() {
    let (app, tmp) = build_app();
    let timestamps = seed_audit_events(tmp.path(), 5);

    let since = &timestamps[3];
    let uri = format!("/api/security/audit?since={since}");
    let resp = app.oneshot(req(Method::GET, &uri)).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let entries = body.as_array().expect("response should be array");
    assert_eq!(
        entries.len(),
        2,
        "since={since} should return 2 entries (at and after)"
    );

    for entry in entries {
        let ts = entry["timestamp"].as_str().unwrap();
        assert!(ts >= since.as_str(), "{ts} should be >= {since}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audit_endpoint_limit_and_since_combined() {
    let (app, tmp) = build_app();
    let timestamps = seed_audit_events(tmp.path(), 5);

    let since = &timestamps[2];
    let uri = format!("/api/security/audit?since={since}&limit=3");
    let resp = app.oneshot(req(Method::GET, &uri)).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let entries = body.as_array().expect("response should be array");
    assert_eq!(entries.len(), 3, "since + limit should return 3 entries");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audit_endpoint_empty_when_no_audit_dir() {
    let (app, _tmp) = build_app();

    let resp = app
        .oneshot(req(Method::GET, "/api/security/audit"))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let entries = body.as_array().expect("response should be array");
    assert!(entries.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audit_endpoint_bad_since_returns_400() {
    let (app, _tmp) = build_app();

    let resp = app
        .oneshot(req(Method::GET, "/api/security/audit?since=not-a-date"))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
