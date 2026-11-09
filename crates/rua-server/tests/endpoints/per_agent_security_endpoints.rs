use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::security;
use rua_server::middleware::agent_context_middleware;
use rua_server::AppState;

fn enable_sync(tmp: &TempDir) {
    let sync_dir = tmp.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    std::fs::write(sync_dir.join("enabled"), "true").expect("sync enabled flag");
    rua_sync::identity::NetworkSecret::generate()
        .save(&sync_dir)
        .expect("network secret");
}

fn build_app() -> (Router, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed default workspace");
    std::fs::create_dir_all(tmp.path().join("workspaces/agent-a")).expect("seed agent-a workspace");
    std::fs::create_dir_all(tmp.path().join("workspaces/agent-b")).expect("seed agent-b workspace");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    let app = Router::new()
        .nest("/api", security::router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state);
    (app, tmp)
}

fn build_sync_enabled_app() -> (Router, TempDir, AppState) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed default workspace");
    std::fs::create_dir_all(tmp.path().join("workspaces/agent-a")).expect("seed agent-a workspace");
    std::fs::create_dir_all(tmp.path().join("workspaces/agent-b")).expect("seed agent-b workspace");
    enable_sync(&tmp);

    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    assert!(
        state.sync_enabled.load(std::sync::atomic::Ordering::SeqCst),
        "sync should be enabled for security v4 emit regression test"
    );
    let app = Router::new()
        .nest("/api", security::router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state.clone());
    (app, tmp, state)
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

fn req(method: Method, uri: &str, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(v) => {
            builder = builder.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    builder.body(body).expect("request")
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
async fn get_agent_tool_guard_returns_builtin_default() {
    let (app, _tmp) = build_app();

    let resp = app
        .oneshot(req(
            Method::GET,
            "/api/agents/agent-a/security/tool-guard",
            None,
        ))
        .await
        .expect("response");

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["enabled"], true);
    assert!(body.get("denied_tools").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn put_agent_tool_guard_persists_and_returns_updated() {
    let (app, tmp) = build_app();

    let update = json!({
        "enabled": true,
        "custom_rules": [],
        "disabled_rules": ["TOOL_CMD_DANGEROUS_RM"]
    });

    let resp = app
        .oneshot(req(
            Method::PUT,
            "/api/agents/agent-a/security/tool-guard",
            Some(update),
        ))
        .await
        .expect("response");

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert!(body.get("denied_tools").is_none());
    assert_eq!(body["disabled_rules"], json!(["TOOL_CMD_DANGEROUS_RM"]));

    let on_disk: Value = serde_json::from_str(
        &std::fs::read_to_string(tmp.path().join("workspaces/agent-a/security.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        on_disk["tool_guard"]["disabled_rules"],
        json!(["TOOL_CMD_DANGEROUS_RM"])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn put_agent_tool_guard_emits_agent_security_v4_when_sync_enabled() {
    let (app, _tmp, state) = build_sync_enabled_app();

    let update = json!({
        "enabled": true,
        "approval_mode": "strict",
        "custom_rules": [],
        "disabled_rules": ["TOOL_CMD_DANGEROUS_RM"]
    });

    let resp = app
        .oneshot(req(
            Method::PUT,
            "/api/agents/agent-a/security/tool-guard",
            Some(update),
        ))
        .await
        .expect("response");

    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let security_change = changes
        .iter()
        .find(|change| change.domain == "agent_security" && change.record_id == "agent-a");
    assert!(
        security_change.is_some_and(|change| {
            change.kind == "upsert"
                && change.payload["tool_guard"]["approval_mode"] == "hands_off"
                && change.payload["tool_guard"]["disabled_rules"]
                    == json!(["TOOL_CMD_DANGEROUS_RM"])
        }),
        "tool guard PUT must append an agent_security v4 change; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn put_agent_approval_mode_emits_agent_security_v4_when_sync_enabled() {
    let (app, _tmp, state) = build_sync_enabled_app();

    let resp = app
        .oneshot(req(
            Method::PUT,
            "/api/agents/agent-a/security/approval-mode",
            Some(json!({"approval_mode": "ask_first"})),
        ))
        .await
        .expect("response");

    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let security_change = changes
        .iter()
        .find(|change| change.domain == "agent_security" && change.record_id == "agent-a");
    assert!(
        security_change.is_some_and(|change| {
            change.kind == "upsert" && change.payload["tool_guard"]["approval_mode"] == "ask_first"
        }),
        "approval-mode PUT must append an agent_security v4 change; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn put_process_security_emits_root_config_security_without_auth_bypass_hosts() {
    let (app, _tmp, state) = build_sync_enabled_app();

    let allow_resp = app
        .clone()
        .oneshot(req(
            Method::PUT,
            "/api/config/security/allow-no-auth-hosts",
            Some(json!({"hosts": ["203.0.113.10"]})),
        ))
        .await
        .expect("allow-no-auth response");
    assert_eq!(allow_resp.status(), StatusCode::OK);

    let update = json!({
        "enabled": false,
        "paths": ["**/.env.local"],
        "disabled_patterns": []
    });
    let resp = app
        .oneshot(req(
            Method::PUT,
            "/api/config/security/file-guard",
            Some(update),
        ))
        .await
        .expect("response");

    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let security_change = changes
        .iter()
        .rev()
        .find(|change| change.domain == "root_config" && change.record_id == "security");
    assert!(
        security_change.is_some_and(|change| {
            change.kind == "upsert"
                && change.payload["file_guard"]["enabled"] == false
                && change.payload["file_guard"]["sensitive_files"] == json!(["**/.env.local"])
                && change.payload.get("allow_no_auth_hosts").is_none()
        }),
        "process file-guard PUT must append a root_config/security v4 change without auth-bypass hosts; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cross_agent_isolation_via_endpoints() {
    let (app, _tmp) = build_app();

    let update = json!({
        "enabled": true,
        "custom_rules": [],
        "disabled_rules": ["TOOL_CMD_DANGEROUS_RM"]
    });
    let resp = app
        .clone()
        .oneshot(req(
            Method::PUT,
            "/api/agents/agent-a/security/tool-guard",
            Some(update),
        ))
        .await
        .expect("response");
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(req(
            Method::GET,
            "/api/agents/agent-b/security/tool-guard",
            None,
        ))
        .await
        .expect("response");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert!(body.get("denied_tools").is_none());
    assert_ne!(
        body["disabled_rules"],
        json!(["TOOL_CMD_DANGEROUS_RM"]),
        "agent-b must not inherit agent-a disabled rules"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_agent_builtin_rules_returns_catalog() {
    let (app, _tmp) = build_app();

    let resp = app
        .oneshot(req(
            Method::GET,
            "/api/agents/agent-a/security/tool-guard/builtin-rules",
            None,
        ))
        .await
        .expect("response");

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let rules = body.as_array().expect("rules array");
    assert!(rules.len() >= 10, "expected >= 10 builtin rules");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reset_to_builtin_restores_defaults() {
    let (app, _tmp) = build_app();

    let update = json!({
        "enabled": false,
        "custom_rules": [],
        "disabled_rules": ["rule-1"]
    });
    let resp = app
        .clone()
        .oneshot(req(
            Method::PUT,
            "/api/agents/agent-a/security/tool-guard",
            Some(update),
        ))
        .await
        .expect("response");
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(req(
            Method::POST,
            "/api/agents/agent-a/security/tool-guard/reset-to-builtin",
            None,
        ))
        .await
        .expect("response");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["enabled"], true);
    assert!(body.get("denied_tools").is_none());

    let resp = app
        .oneshot(req(
            Method::GET,
            "/api/agents/agent-a/security/tool-guard",
            None,
        ))
        .await
        .expect("response");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["enabled"], true);
    assert!(body.get("denied_tools").is_none());
}
