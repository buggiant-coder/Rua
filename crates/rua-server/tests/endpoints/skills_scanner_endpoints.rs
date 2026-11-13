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
        .nest(
            "/api",
            register_scanner_routes().merge(rua_server::endpoints::security::router()),
        )
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

struct ScanEnvGuard {
    root: Option<std::ffi::OsString>,
    mode: Option<std::ffi::OsString>,
}

impl Drop for ScanEnvGuard {
    fn drop(&mut self) {
        for (key, value) in [
            ("RUA_ROOT", &self.root),
            ("RUA_SKILL_SCAN_MODE", &self.mode),
        ] {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

async fn scanner_request(app: &Router, method: &str, path: &str, body: Value) -> Value {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("Content-Type", "application/json")
                .header("X-Agent-Id", "default")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = read_json(response).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn manual_scan_uses_saved_settings_and_reloads_them_without_restart() {
    let tmp = TempDir::new().unwrap();
    let _env = ScanEnvGuard {
        root: std::env::var_os("RUA_ROOT"),
        mode: std::env::var_os("RUA_SKILL_SCAN_MODE"),
    };
    // Keep scanner history isolated, and ensure configuration comes from AppState's root.
    std::env::set_var("RUA_ROOT", tmp.path().join("ambient-root"));
    std::env::remove_var("RUA_SKILL_SCAN_MODE");
    let app = build_test_app(build_state(&tmp));
    let fixtures = [
        (
            "workspace-probe",
            tmp.path().join("workspaces/default/skills/workspace-probe"),
        ),
        ("pool-probe", tmp.path().join("skill_pool/pool-probe")),
    ];
    for (name, dir) in &fixtures {
        std::fs::create_dir_all(dir).unwrap();
        rua_config::atomic_write(
            &dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: scanner fixture\n---\n# Probe\n").as_bytes(),
        )
        .unwrap();
        // Static input only; the scanner must never execute this file.
        rua_config::atomic_write(
            &dir.join("main.py"),
            b"import subprocess\nsubprocess.call('echo hi', shell=True)\n",
        )
        .unwrap();
    }
    let settings = "/api/config/security/skill-scanner";
    let scan = "/api/skills/scanner/scan";
    scanner_request(
        &app,
        "PUT",
        settings,
        json!({"mode":"warn", "timeout":7, "whitelist":[]}),
    )
    .await;
    let warned = scanner_request(&app, "POST", scan, Value::Null).await;
    assert!(
        warned["issues"].as_u64().unwrap() > 0,
        "fixture must trigger findings"
    );
    assert!(warned["results"]
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["action"] == "warned"));

    scanner_request(
        &app,
        "PUT",
        settings,
        json!({"mode":"off", "timeout":7, "whitelist":[]}),
    )
    .await;
    let off = scanner_request(&app, "POST", scan, Value::Null).await;
    assert_eq!(off["issues"], 0, "saved off mode must skip scanning");
    assert_eq!(off["results"], json!([]));

    let whitelist: Vec<_> = fixtures
        .iter()
        .map(|(name, dir)| {
            json!({
                "skill_name": name, "content_hash": compute_hash(dir), "added_at":""
            })
        })
        .collect();
    scanner_request(
        &app,
        "PUT",
        settings,
        json!({"mode":"warn", "timeout":7, "whitelist":whitelist}),
    )
    .await;
    let trusted = scanner_request(&app, "POST", scan, Value::Null).await;
    assert_eq!(
        trusted["issues"], 0,
        "saved content whitelist must be applied"
    );
    rua_config::atomic_write(&fixtures[0].1.join("changed.txt"), b"changed bundle").unwrap();
    let changed = scanner_request(&app, "POST", scan, Value::Null).await;
    assert!(changed["issues"].as_u64().unwrap() > 0);
    assert!(changed["results"]
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["skill_name"] == "workspace-probe"));

    scanner_request(
        &app,
        "PUT",
        settings,
        json!({"mode":"block", "timeout":7, "whitelist":[]}),
    )
    .await;
    let blocked = scanner_request(&app, "POST", scan, Value::Null).await;
    assert!(blocked["issues"].as_u64().unwrap() > 0);
    assert!(blocked["results"]
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["action"] == "blocked"));

    let path = tmp.path().join("config.json");
    let mut config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    config["security"]["skill_scanner"]["timeout"] = json!("invalid");
    rua_config::atomic_write_json(&path, &config).unwrap();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(scan)
                .header("X-Agent-Id", "default")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        read_json(response).await["detail"].is_string(),
        "invalid settings must not silently fall back"
    );
}

fn compute_hash(dir: &std::path::Path) -> String {
    rua_skills_scanner::compute_skill_content_hash(dir).unwrap()
}
