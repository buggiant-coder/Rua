use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::acp::{agent_scoped_router, root_router};
use rua_server::middleware::agent_context_middleware;
use rua_server::AppState;

fn seed_default_agent(tmp: &TempDir, body: Value) {
    let dir = tmp.path().join("workspaces").join("default");
    std::fs::create_dir_all(&dir).expect("seed default workspace");
    std::fs::write(
        dir.join("agent.json"),
        serde_json::to_string_pretty(&body).unwrap(),
    )
    .expect("write agent.json");
}

fn agent_skeleton() -> Value {
    json!({
        "id": "default",
        "name": "default",
        "description": "",
        "workspace_dir": "",
        "language": "",
        "system_prompt_files": [],
    })
}

fn build_app(tmp: &TempDir) -> Router {
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    Router::new()
        .nest("/api", root_router())
        .nest("/api", agent_scoped_router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state)
}

fn build_sync_enabled_app(tmp: &TempDir) -> (Router, AppState) {
    let sync_dir = tmp.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    std::fs::write(sync_dir.join("enabled"), "true").expect("sync enabled flag");
    rua_sync::identity::NetworkSecret::generate()
        .save(&sync_dir)
        .expect("network secret");

    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    assert!(
        state.sync_enabled.load(std::sync::atomic::Ordering::SeqCst),
        "sync should be enabled for ACP v4 emit regression test"
    );
    let app = Router::new()
        .nest("/api", root_router())
        .nest("/api", agent_scoped_router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state.clone());
    (app, state)
}

fn mock_acp_agent_path() -> String {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../rua-acp/tests/fixtures/mock_acp_agent.py");
    path.to_string_lossy().to_string()
}

fn mock_acp_command() -> &'static str {
    if cfg!(windows) {
        "python"
    } else {
        "python3"
    }
}

async fn read_json(resp: axum::response::Response) -> Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    if bytes.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

fn req(method: &str, uri: &str, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("X-Agent-Id", "default");
    let body = match body {
        Some(json) => {
            builder = builder.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&json).unwrap())
        }
        None => Body::empty(),
    };
    builder.body(body).unwrap()
}

async fn collect_v4_changes(state: &AppState) -> Vec<rua_sync::opaque_replica_log::SyncChange> {
    let (sync_doc, blobs_store) = {
        let guard = state.sync_node.lock().await;
        let node = guard.as_ref().expect("sync node");
        (node.doc().clone(), node.blobs_store().clone())
    };
    let network_secret = state
        .sync_network_secret
        .read()
        .unwrap()
        .as_ref()
        .expect("network secret")
        .clone();
    let device = state
        .sync_device
        .read()
        .unwrap()
        .as_ref()
        .expect("device")
        .clone();
    let log = rua_sync::opaque_replica_log::OpaqueReplicaLog::new(
        sync_doc.clone(),
        device.to_iroh_author(),
        network_secret.derive_sync_key(),
        blobs_store,
    );

    let entries = sync_doc
        .get_many(iroh_docs::store::Query::all().build())
        .await
        .expect("query Replica Log");
    let entries = futures_util::TryStreamExt::try_collect::<Vec<_>>(entries)
        .await
        .expect("Replica Log entries");

    let mut changes = Vec::new();
    for entry in entries {
        if let Some(frame) = log
            .read_frame(entry.author(), entry.key())
            .await
            .expect("read sync frame")
        {
            changes.extend(frame.into_changes());
        }
    }
    changes
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_returns_four_builtin_runners() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp, agent_skeleton());
    let app = build_app(&tmp);

    let resp = app
        .oneshot(req("GET", "/api/acp", None))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);

    let body = read_json(resp).await;
    let runners = body.as_array().expect("runners is an array");
    assert!(
        runners.len() >= 4,
        "expected at least 4 builtins, got {}",
        runners.len()
    );

    let keys: Vec<&str> = runners.iter().filter_map(|r| r["key"].as_str()).collect();
    for builtin in &["opencode", "qwen", "claude", "codex"] {
        assert!(
            keys.contains(builtin),
            "missing builtin '{builtin}' in {keys:?}"
        );
    }

    for runner in runners {
        if ["opencode", "qwen", "claude", "codex"].contains(&runner["key"].as_str().unwrap_or("")) {
            assert_eq!(
                runner["is_builtin"], true,
                "builtin runner should have is_builtin=true"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crud_lifecycle() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp, agent_skeleton());
    let app = build_app(&tmp);

    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/acp",
            Some(json!({
                "key": "my_runner",
                "enabled": true,
                "command": mock_acp_command(),
                "args": [mock_acp_agent_path()],
                "trusted": false,
            })),
        ))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = read_json(resp).await;
    assert_eq!(body["key"], "my_runner");
    assert_eq!(body["command"], mock_acp_command());
    assert_eq!(body["is_builtin"], false);

    let resp = app
        .clone()
        .oneshot(req("GET", "/api/acp/my_runner", None))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["key"], "my_runner");
    assert_eq!(body["command"], mock_acp_command());

    let resp = app
        .clone()
        .oneshot(req(
            "PUT",
            "/api/acp/my_runner",
            Some(json!({
                "enabled": true,
                "command": mock_acp_command(),
                "args": [mock_acp_agent_path(), "--verbose"],
                "trusted": true,
            })),
        ))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["command"], mock_acp_command());
    assert_eq!(body["trusted"], true);

    let resp = app
        .clone()
        .oneshot(req("PATCH", "/api/acp/my_runner/toggle", None))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["enabled"], false);

    let resp = app
        .clone()
        .oneshot(req("PATCH", "/api/acp/my_runner/toggle", None))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["enabled"], true);

    let resp = app
        .clone()
        .oneshot(req("DELETE", "/api/acp/my_runner", None))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(req("GET", "/api/acp/my_runner", None))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acp_runner_lifecycle_emits_agent_config_v4_without_device_local_runtime_fields() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp, agent_skeleton());
    let (app, state) = build_sync_enabled_app(&tmp);

    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/acp",
            Some(json!({
                "key": "sync_acp_runner",
                "enabled": false,
                "command": mock_acp_command(),
                "args": [mock_acp_agent_path(), "--local-only"],
                "env": {"LOCAL_SECRET": "must-not-sync"},
                "trusted": true,
            })),
        ))
        .await
        .expect("create ACP runner");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let changes = collect_v4_changes(&state).await;
    let created = changes.iter().find(|change| {
        let runner = &change.payload["acp"]["agents"]["sync_acp_runner"];
        change.domain == "agent_config"
            && change.record_id == "default"
            && change.kind == "upsert"
            && runner["trusted"] == true
            && runner["enabled"] == false
            && runner.get("command").is_none()
            && runner.get("args").is_none()
            && runner.get("env").is_none()
    });
    assert!(
        created.is_some(),
        "ACP POST must append agent_config v4 and strip device-local command/args/env; got {changes:#?}"
    );

    let resp = app
        .clone()
        .oneshot(req(
            "PUT",
            "/api/acp/sync_acp_runner",
            Some(json!({
                "enabled": false,
                "command": mock_acp_command(),
                "args": [mock_acp_agent_path(), "--updated-local-only"],
                "env": {"UPDATED_LOCAL_SECRET": "must-not-sync"},
                "trusted": false,
            })),
        ))
        .await
        .expect("update ACP runner");
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let updated = changes.iter().find(|change| {
        let runner = &change.payload["acp"]["agents"]["sync_acp_runner"];
        change.domain == "agent_config"
            && change.record_id == "default"
            && change.kind == "upsert"
            && runner["trusted"] == false
            && runner["enabled"] == false
            && runner.get("command").is_none()
            && runner.get("args").is_none()
            && runner.get("env").is_none()
    });
    assert!(
        updated.is_some(),
        "ACP PUT must append agent_config v4 and strip device-local command/args/env; got {changes:#?}"
    );

    let resp = app
        .oneshot(req("DELETE", "/api/acp/sync_acp_runner", None))
        .await
        .expect("delete ACP runner");
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let deleted = changes.iter().find(|change| {
        change.domain == "agent_config"
            && change.record_id == "default"
            && change.kind == "upsert"
            && change.payload["acp"]["agents"]["sync_acp_runner"].is_null()
    });
    assert!(
        deleted.is_some(),
        "ACP DELETE must append agent_config v4 without the deleted runner; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_builtin_runner_rejected() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp, agent_skeleton());
    let app = build_app(&tmp);

    for builtin in &["opencode", "qwen", "claude", "codex"] {
        let resp = app
            .clone()
            .oneshot(req("DELETE", &format!("/api/acp/{builtin}"), None))
            .await
            .expect("send");
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "deleting builtin '{builtin}' should be forbidden"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_duplicate_rejected() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp, agent_skeleton());
    let app = build_app(&tmp);

    let body = json!({
        "key": "dup_runner",
        "enabled": false,
        "command": "dup",
        "args": [],
        "trusted": false,
    });

    let resp = app
        .clone()
        .oneshot(req("POST", "/api/acp", Some(body.clone())))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .clone()
        .oneshot(req("POST", "/api/acp", Some(body)))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn check_is_advisory_and_is_not_cached_on_runner_detail() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp, agent_skeleton());
    let app = build_app(&tmp);

    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/acp",
            Some(json!({
                "key": "mock_check",
                "enabled": false,
                "command": mock_acp_command(),
                "args": [mock_acp_agent_path()],
                "trusted": true,
            })),
        ))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .clone()
        .oneshot(req("GET", "/api/acp/mock_check", None))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body.get("check_result"), None);

    let resp = app
        .clone()
        .oneshot(req("GET", "/api/acp/mock_check/check", None))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["ok"], true);
    assert_eq!(body["status"], "ok");

    let resp = app
        .oneshot(req("GET", "/api/acp/mock_check", None))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body.get("check_result"), None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enabled_runner_check_does_not_send_prompt() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp, agent_skeleton());
    let app = build_app(&tmp);

    let resp = app
        .oneshot(req(
            "POST",
            "/api/acp",
            Some(json!({
                "key": "no_prompt_check",
                "enabled": true,
                "command": mock_acp_command(),
                "args": [mock_acp_agent_path()],
                "env": {
                    "ACP_MOCK_FAIL_ON_TURN": "1",
                    "ACP_MOCK_FAIL_ON_PROMPT": "1"
                },
                "trusted": true,
            })),
        ))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::CREATED);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unavailable_runner_can_be_enabled_updated_and_disabled_without_probe() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp, agent_skeleton());
    let app = build_app(&tmp);

    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/acp",
            Some(json!({
                "key": "bad_runner",
                "enabled": false,
                "command": "definitely-missing-rua-acp-agent",
                "args": [],
                "trusted": true,
            })),
        ))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .clone()
        .oneshot(req("PATCH", "/api/acp/bad_runner/toggle", None))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["enabled"], true);

    let resp = app
        .clone()
        .oneshot(req(
            "PUT",
            "/api/acp/bad_runner",
            Some(json!({
                "enabled": true,
                "command": mock_acp_command(),
                "args": [mock_acp_agent_path()],
                "trusted": true,
            })),
        ))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["enabled"], true);

    let resp = app
        .oneshot(req("PATCH", "/api/acp/bad_runner/toggle", None))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["enabled"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_routes() {
    let tmp = TempDir::new().expect("tempdir");
    let agent_dir = tmp.path().join("workspaces").join("agent1");
    std::fs::create_dir_all(&agent_dir).expect("create agent1 dir");
    std::fs::write(
        agent_dir.join("agent.json"),
        serde_json::to_string_pretty(&json!({
            "id": "agent1",
            "name": "agent1",
            "description": "",
            "workspace_dir": "",
            "language": "",
            "system_prompt_files": [],
        }))
        .unwrap(),
    )
    .expect("write agent.json");

    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    let app = Router::new()
        .nest("/api", root_router())
        .nest("/api", agent_scoped_router())
        .with_state(state);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/agents/agent1/acp")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let runners = body.as_array().expect("runners array");
    assert!(runners.len() >= 4);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/agents/agent1/acp")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "key": "agent1_runner",
                        "enabled": false,
                        "command": "my-cmd",
                        "args": [],
                        "trusted": false,
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/agents/agent1/acp/agent1_runner")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["key"], "agent1_runner");
}
