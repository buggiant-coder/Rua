use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use serial_test::serial;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::skills_scanner::register_scanner_routes;
use rua_server::middleware::agent_context_middleware;
use rua_server::AppState;

const BLOCKED_FILE: &str = "skill_scan_blocked.json";

fn build_state(tmp: &TempDir) -> AppState {
    AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test)
}

fn build_test_app(state: AppState) -> Router {
    Router::new()
        .nest("/api", register_scanner_routes())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state)
}

async fn read_json(resp: axum::response::Response) -> Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("body is json")
}

fn rua_dir(tmp: &TempDir) -> std::path::PathBuf {
    tmp.path().join(".rua")
}

fn seed_history(tmp: &TempDir, records: &[Value]) {
    let dir = rua_dir(tmp);
    std::fs::create_dir_all(&dir).expect("create .rua dir");
    let path = dir.join(BLOCKED_FILE);
    std::fs::write(&path, serde_json::to_string_pretty(records).unwrap())
        .expect("write seed history");
}

fn fake_record(name: &str, hash: &str) -> Value {
    json!({
        "skill_name": name,
        "blocked_at": "2026-05-10T00:00:00Z",
        "max_severity": "critical",
        "findings": [],
        "content_hash": hash,
        "action": "blocked",
    })
}

struct HomeGuard {
    old_home: Option<String>,
    old_rua_root: Option<String>,
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        match &self.old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        match &self.old_rua_root {
            Some(dir) => std::env::set_var("RUA_ROOT", dir),
            None => std::env::remove_var("RUA_ROOT"),
        }
    }
}

fn redirect_home(tmp: &TempDir) -> HomeGuard {
    let old_home = std::env::var("HOME").ok();
    let old_rua_root = std::env::var("RUA_ROOT").ok();
    std::env::set_var("HOME", tmp.path());
    std::env::set_var("RUA_ROOT", rua_dir(tmp));
    std::fs::create_dir_all(rua_dir(tmp)).expect("create .rua dir");
    HomeGuard {
        old_home,
        old_rua_root,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn list_returns_empty_when_no_history_file() {
    let tmp = TempDir::new().expect("tempdir");
    let _guard = redirect_home(&tmp);

    let state = build_state(&tmp);
    let app = build_test_app(state);

    let req = Request::builder()
        .uri("/api/skills/scanner/blocked")
        .header("X-Agent-Id", "default")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);

    let body = read_json(resp).await;
    assert!(body.is_array(), "expected JSON array, got {body}");
    assert_eq!(body.as_array().unwrap().len(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn list_returns_seeded_records() {
    let tmp = TempDir::new().expect("tempdir");
    let _guard = redirect_home(&tmp);
    seed_history(
        &tmp,
        &[
            fake_record("alpha-skill", "hash-a"),
            fake_record("beta-skill", "hash-b"),
        ],
    );

    let state = build_state(&tmp);
    let app = build_test_app(state);

    let req = Request::builder()
        .uri("/api/skills/scanner/blocked")
        .header("X-Agent-Id", "default")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);

    let body = read_json(resp).await;
    let arr = body.as_array().expect("array body");
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["skill_name"], "alpha-skill");
    assert_eq!(arr[0]["content_hash"], "hash-a");
    assert_eq!(arr[1]["skill_name"], "beta-skill");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn clear_wipes_history_then_list_is_empty() {
    let tmp = TempDir::new().expect("tempdir");
    let _guard = redirect_home(&tmp);
    seed_history(
        &tmp,
        &[
            fake_record("alpha-skill", "hash-a"),
            fake_record("beta-skill", "hash-b"),
        ],
    );

    let state = build_state(&tmp);
    let app = build_test_app(state.clone());

    let req = Request::builder()
        .method("POST")
        .uri("/api/skills/scanner/blocked/clear")
        .header("X-Agent-Id", "default")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["cleared"], true);

    assert!(!rua_dir(&tmp).join(BLOCKED_FILE).exists());

    let app2 = build_test_app(state);
    let req2 = Request::builder()
        .uri("/api/skills/scanner/blocked")
        .header("X-Agent-Id", "default")
        .body(Body::empty())
        .unwrap();
    let resp2 = app2.oneshot(req2).await.expect("oneshot");
    assert_eq!(resp2.status(), StatusCode::OK);
    let body2 = read_json(resp2).await;
    assert_eq!(body2.as_array().unwrap().len(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn delete_drops_only_matching_entry() {
    let tmp = TempDir::new().expect("tempdir");
    let _guard = redirect_home(&tmp);
    seed_history(
        &tmp,
        &[
            fake_record("alpha-skill", "hash-a"),
            fake_record("beta-skill", "hash-b"),
        ],
    );

    let state = build_state(&tmp);
    let app = build_test_app(state.clone());

    let req = Request::builder()
        .method("DELETE")
        .uri("/api/skills/scanner/blocked/alpha-skill/hash-a")
        .header("X-Agent-Id", "default")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["removed"], true);

    let app2 = build_test_app(state);
    let req2 = Request::builder()
        .uri("/api/skills/scanner/blocked")
        .header("X-Agent-Id", "default")
        .body(Body::empty())
        .unwrap();
    let resp2 = app2.oneshot(req2).await.expect("oneshot");
    assert_eq!(resp2.status(), StatusCode::OK);
    let arr = read_json(resp2).await;
    let arr = arr.as_array().expect("array");
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["skill_name"], "beta-skill");
    assert_eq!(arr[0]["content_hash"], "hash-b");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn delete_returns_404_when_file_missing() {
    let tmp = TempDir::new().expect("tempdir");
    let _guard = redirect_home(&tmp);

    let state = build_state(&tmp);
    let app = build_test_app(state);

    let req = Request::builder()
        .method("DELETE")
        .uri("/api/skills/scanner/blocked/missing/abc")
        .header("X-Agent-Id", "default")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn delete_returns_404_when_entry_missing() {
    let tmp = TempDir::new().expect("tempdir");
    let _guard = redirect_home(&tmp);
    seed_history(&tmp, &[fake_record("alpha-skill", "hash-a")]);

    let state = build_state(&tmp);
    let app = build_test_app(state);

    let req = Request::builder()
        .method("DELETE")
        .uri("/api/skills/scanner/blocked/alpha-skill/wrong-hash")
        .header("X-Agent-Id", "default")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = read_json(resp).await;
    assert!(body["detail"]
        .as_str()
        .unwrap_or_default()
        .contains("alpha-skill"));
}
