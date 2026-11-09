use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::tools::agent_scoped_router;
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

fn agent_skeleton(builtin_tools: Value) -> Value {
    json!({
        "id": "default",
        "name": "default",
        "description": "",
        "workspace_dir": "",
        "language": "",
        "system_prompt_files": [],
        "tools": {
            "builtin_tools": builtin_tools,
        },
    })
}

fn build_app(tmp: &TempDir) -> Router {
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    Router::new()
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
        state
            .sync_enabled
            .load(std::sync::atomic::Ordering::Relaxed),
        "sync should be enabled for tools v4 emit regression test"
    );
    let app = Router::new()
        .nest("/api", agent_scoped_router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state.clone());
    (app, state)
}

async fn read_json(resp: axum::response::Response) -> Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
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
async fn tools_endpoint_write_persists_then_external_read_sees() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(
        &tmp,
        agent_skeleton(json!(rua_config::types::tools::default_builtin_tools())),
    );
    let app = build_app(&tmp);

    let resp = app
        .oneshot(req(
            "PATCH",
            "/api/agents/default/tools/bash/toggle",
            Some(json!({"enabled": false})),
        ))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);

    let path = tmp.path().join("workspaces/default/agent.json");
    let raw: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");

    assert_eq!(
        raw["tools"]["builtin_tools"]["bash"]["enabled"], false,
        "external reader must see the disabled bash tool"
    );

    assert_eq!(raw["name"], "default");
    assert_eq!(raw["description"], "");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tools_toggle_and_async_execution_emit_agent_config_v4_when_sync_enabled() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(
        &tmp,
        agent_skeleton(json!(rua_config::types::tools::default_builtin_tools())),
    );
    let (app, state) = build_sync_enabled_app(&tmp);

    let resp = app
        .clone()
        .oneshot(req(
            "PATCH",
            "/api/agents/default/tools/bash/toggle",
            Some(json!({"enabled": false})),
        ))
        .await
        .expect("send toggle");
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let toggle_change = changes
        .iter()
        .find(|change| change.domain == "agent_config" && change.record_id == "default");
    assert!(
        toggle_change.is_some_and(|change| {
            change.kind == "upsert"
                && change.payload["tools"]["builtin_tools"]["bash"]["enabled"] == false
        }),
        "tool toggle must append an agent_config v4 change with tools payload; got {changes:#?}"
    );

    let resp = app
        .oneshot(req(
            "PATCH",
            "/api/agents/default/tools/bash/async-execution",
            Some(json!({"async_execution": true})),
        ))
        .await
        .expect("send async execution");
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let async_change = changes.iter().find(|change| {
        change.domain == "agent_config"
            && change.record_id == "default"
            && change.payload["tools"]["builtin_tools"]["bash"]["async_execution"] == true
    });
    assert!(
        async_change.is_some(),
        "tool async-execution update must append an agent_config v4 change; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tools_set_then_simulated_restart_keeps_value() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(
        &tmp,
        agent_skeleton(json!(rua_config::types::tools::default_builtin_tools())),
    );

    {
        let app = build_app(&tmp);
        let resp = app
            .oneshot(req(
                "PATCH",
                "/api/agents/default/tools/bash/toggle",
                Some(json!({"enabled": false})),
            ))
            .await
            .expect("send");
        assert_eq!(resp.status(), StatusCode::OK);
    }

    {
        let app = build_app(&tmp);
        let resp = app
            .oneshot(req("GET", "/api/agents/default/tools", None))
            .await
            .expect("send");
        assert_eq!(resp.status(), StatusCode::OK);
        let body = read_json(resp).await;
        let tools = body.as_array().expect("array");
        let bash = tools.iter().find(|t| t["name"] == "bash").expect("bash");
        assert_eq!(
            bash["enabled"], false,
            "after restart, bash must still be disabled"
        );
    }
}
