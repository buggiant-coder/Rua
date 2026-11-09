use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::shared_mcp;
use rua_server::middleware::agent_context_middleware;
use rua_server::AppState;

fn seed_default_agent(tmp: &TempDir) {
    let dir = tmp.path().join("workspaces").join("default");
    std::fs::create_dir_all(&dir).expect("seed default workspace");
    std::fs::write(
        dir.join("agent.json"),
        serde_json::to_string_pretty(&json!({
            "id": "default",
            "name": "default",
            "description": "",
            "workspace_dir": "",
            "language": "",
            "system_prompt_files": [],
        }))
        .unwrap(),
    )
    .expect("write agent.json");
}

fn build_app_with_state(tmp: &TempDir) -> (Router, AppState) {
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    let app = Router::new()
        .nest("/api", shared_mcp::router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state.clone());
    (app, state)
}

fn enable_sync(tmp: &TempDir) {
    let sync_dir = tmp.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    std::fs::write(sync_dir.join("enabled"), "true").expect("sync enabled flag");
    rua_sync::identity::NetworkSecret::generate()
        .save(&sync_dir)
        .expect("network secret");
}

fn build_app(tmp: &TempDir) -> Router {
    build_app_with_state(tmp).0
}

async fn wait_for_inbound_inbox_lock_release(tmp: &TempDir) {
    let inbox_dir = tmp.path().join("sync").join("inbound-inbox");
    for _ in 0..50 {
        match rua_sync::inbound_inbox::InboundInbox::open(&inbox_dir) {
            Ok(inbox) => {
                drop(inbox);
                return;
            }
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
        }
    }
    panic!(
        "inbound inbox redb lock was still held after waiting for simulated restart at {}",
        inbox_dir.display()
    );
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

fn sample_create_body(key: &str) -> Value {
    json!({
        "key": key,
        "name": format!("{key}-name"),
        "enabled": true,
        "transport": "stdio",
        "command": "/bin/echo",
        "args": ["hi"],
        "env": {"K": "v"}
    })
}

fn list_keys(list_body: &Value) -> Vec<String> {
    list_body
        .as_array()
        .expect("list array")
        .iter()
        .filter_map(|c| c["key"].as_str().map(String::from))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn add_server_appears_in_list_immediately() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    let app = build_app(&tmp);

    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/shared-mcp",
            Some(sample_create_body("alpha")),
        ))
        .await
        .expect("send POST");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .oneshot(req("GET", "/api/shared-mcp", None))
        .await
        .expect("send GET");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let keys = list_keys(&body);
    assert!(
        keys.contains(&"alpha".to_string()),
        "expected alpha in list after POST, got {keys:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn add_server_emits_shared_mcp_v4_when_sync_enabled() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    enable_sync(&tmp);
    let (app, state) = build_app_with_state(&tmp);

    let resp = app
        .oneshot(req(
            "POST",
            "/api/shared-mcp",
            Some(sample_create_body("sync-shared")),
        ))
        .await
        .expect("send POST");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let changes = collect_v4_changes(&state).await;
    let shared_mcp_change = changes
        .iter()
        .find(|change| change.domain == "shared_mcp" && change.record_id == "catalog");
    assert!(
        shared_mcp_change.is_some_and(|change| {
            change.payload["clients"]["sync-shared"]["name"] == json!("sync-shared-name")
                && change.payload["clients"]["sync-shared"]["env"]["K"] == json!("v")
        }),
        "shared MCP POST must append a shared_mcp v4 catalog change with logical client payload; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_and_delete_server_emit_shared_mcp_v4_when_sync_enabled() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    enable_sync(&tmp);
    let (app, state) = build_app_with_state(&tmp);

    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/shared-mcp",
            Some(sample_create_body("sync-shared-life")),
        ))
        .await
        .expect("send POST");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .clone()
        .oneshot(req(
            "PUT",
            "/api/shared-mcp/sync-shared-life",
            Some(json!({
                "name": "sync-shared-life-renamed",
                "env": {"K": "v2", "SECRET": "logical-secret"},
            })),
        ))
        .await
        .expect("send PUT");
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let updated = changes.iter().find(|change| {
        change.domain == "shared_mcp"
            && change.record_id == "catalog"
            && change.payload["clients"]["sync-shared-life"]["name"]
                == json!("sync-shared-life-renamed")
    });
    assert!(
        updated.is_some_and(|change| {
            change.kind == "upsert"
                && change.payload["clients"]["sync-shared-life"]["name"]
                    == json!("sync-shared-life-renamed")
                && change.payload["clients"]["sync-shared-life"]["env"]["SECRET"]
                    == json!("logical-secret")
        }),
        "shared MCP PUT must append a shared_mcp v4 catalog upsert with logical payload; got {changes:#?}"
    );

    let resp = app
        .oneshot(req("DELETE", "/api/shared-mcp/sync-shared-life", None))
        .await
        .expect("send DELETE");
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let deleted = changes.iter().find(|change| {
        change.domain == "shared_mcp"
            && change.record_id == "client/sync-shared-life"
            && change.kind == "delete"
    });
    assert!(
        deleted.is_some(),
        "shared MCP DELETE must append a shared_mcp v4 delete for the client key; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_server_persists_to_disk() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    let app = build_app(&tmp);

    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/shared-mcp",
            Some(sample_create_body("beta")),
        ))
        .await
        .expect("send POST");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .clone()
        .oneshot(req(
            "PUT",
            "/api/shared-mcp/beta",
            Some(json!({"name": "beta-renamed"})),
        ))
        .await
        .expect("send PUT");
    assert_eq!(resp.status(), StatusCode::OK);

    let path = tmp.path().join("shared_mcp.json");
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("parse");
    assert_eq!(
        raw["clients"]["beta"]["name"].as_str(),
        Some("beta-renamed"),
        "rename must persist to shared_mcp.json, got {raw:#}"
    );

    let resp = app
        .oneshot(req("GET", "/api/shared-mcp/beta", None))
        .await
        .expect("send GET");
    let body = read_json(resp).await;
    assert_eq!(body["name"].as_str(), Some("beta-renamed"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remove_server_disappears_from_list() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    let app = build_app(&tmp);

    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/shared-mcp",
            Some(sample_create_body("gamma")),
        ))
        .await
        .expect("send POST");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .clone()
        .oneshot(req("DELETE", "/api/shared-mcp/gamma", None))
        .await
        .expect("send DELETE");
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(req("GET", "/api/shared-mcp", None))
        .await
        .expect("send GET");
    let body = read_json(resp).await;
    let keys = list_keys(&body);
    assert!(
        !keys.contains(&"gamma".to_string()),
        "expected gamma absent after DELETE, got {keys:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn add_server_then_simulated_restart_keeps_config() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);

    {
        let (app, state) = build_app_with_state(&tmp);
        let resp = app
            .oneshot(req(
                "POST",
                "/api/shared-mcp",
                Some(sample_create_body("survivor")),
            ))
            .await
            .expect("send POST");
        assert_eq!(resp.status(), StatusCode::CREATED);
        state.signal_shutdown();
    }
    wait_for_inbound_inbox_lock_release(&tmp).await;

    let app = build_app(&tmp);
    let resp = app
        .oneshot(req("GET", "/api/shared-mcp", None))
        .await
        .expect("send GET");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let keys = list_keys(&body);
    assert!(
        keys.contains(&"survivor".to_string()),
        "expected survivor to survive simulated restart, got {keys:?}"
    );
}

#[tokio::test]
async fn shared_catalog_write_does_not_apply_to_the_active_agent() {
    let tmp = TempDir::new().unwrap();
    seed_default_agent(&tmp);
    // Linux has no bundled desktop MCP drivers; give this runtime an explicit owner.
    let agent_path = tmp.path().join("workspaces/default/agent.json");
    let mut agent: Value = serde_json::from_slice(&std::fs::read(&agent_path).unwrap()).unwrap();
    agent["mcp"] = json!({"clients": {"disabled-fixture": {
        "name": "Fixture", "enabled": false, "transport": "stdio",
        "command": "no-driver-needed", "auto_connect": "manual"
    }}});
    rua_config::atomic_write_json(&agent_path, &agent).unwrap();
    let (app, state) = build_app_with_state(&tmp);
    let runtime = state.registry.get_or_create("default").await.unwrap();
    let manager = runtime.mcp_manager().await.unwrap();
    let before = manager.configuration().await.unwrap();
    let response = app
        .oneshot(req(
            "POST",
            "/api/shared-mcp",
            Some(sample_create_body("catalog-only")),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(
        serde_json::to_value(before).unwrap(),
        serde_json::to_value(manager.configuration().await.unwrap()).unwrap()
    );
    assert!(std::sync::Arc::ptr_eq(
        &runtime,
        &state.registry.get("default").await.unwrap()
    ));
}
