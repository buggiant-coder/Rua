use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::agent_templates;
use rua_server::endpoints::agents;
use rua_server::endpoints::gap_fixes;
use rua_server::middleware::agent_context_middleware;
use rua_server::AppState;

fn build_app() -> (Router, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed workspace");

    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    assert!(
        !state
            .sync_enabled
            .load(std::sync::atomic::Ordering::Relaxed),
        "sync should be disabled for regression test"
    );

    let app = Router::new()
        .nest("/api", agent_templates::router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state);
    (app, tmp)
}

fn json_req(method: Method, uri: &str, body: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(json) => {
            builder = builder.header("content-type", "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };
    builder.body(body).unwrap()
}

async fn read_json(resp: axum::response::Response) -> Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("response json")
}

fn agent_ids(payload: &Value) -> Vec<String> {
    payload["agents"]
        .as_array()
        .expect("agents array")
        .iter()
        .map(|row| row["id"].as_str().expect("agent id").to_string())
        .collect()
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

fn build_sync_enabled_agents_app() -> (Router, AppState, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed workspace");
    std::fs::write(
        tmp.path().join("workspaces/default/agent.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "id": "default",
            "name": "Default",
            "description": "",
            "workspace_dir": "",
            "language": "",
            "system_prompt_files": []
        }))
        .expect("agent json"),
    )
    .expect("seed agent");
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
        "sync should be enabled for emit regression test"
    );

    let app = Router::new()
        .nest(
            "/api",
            agents::router().merge(gap_fixes::agent_config_router()),
        )
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state.clone());
    (app, state, tmp)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_templates_sync_disabled() {
    let (app, _tmp) = build_app();
    let req = json_req(Method::GET, "/api/agent-templates", None);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_template_sync_disabled() {
    let (app, _tmp) = build_app();
    let req = json_req(
        Method::POST,
        "/api/agent-templates",
        Some(r#"{"name":"Test Template","description":"A test","prompt_files":["test.md"]}"#),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_and_get_template_sync_disabled() {
    let (app, _tmp) = build_app();

    let req = json_req(
        Method::POST,
        "/api/agent-templates",
        Some(r#"{"name":"My Template","description":"Desc","prompt_files":[]}"#),
    );
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let req = json_req(Method::GET, "/api/agent-templates/my-template", None);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_template_sync_disabled() {
    let (app, _tmp) = build_app();

    let req = json_req(
        Method::POST,
        "/api/agent-templates",
        Some(r#"{"name":"Deletable","description":"x","prompt_files":[]}"#),
    );
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let req = json_req(Method::DELETE, "/api/agent-templates/deletable", None);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_update_delete_agent_emit_agent_config_and_registry_v4_when_sync_enabled() {
    let (app, state, _tmp) = build_sync_enabled_agents_app();
    let agent_id = "sync-agent-life";

    let create_resp = app
        .clone()
        .oneshot(json_req(
            Method::POST,
            "/api/agents",
            Some(
                r#"{
                    "id":"sync-agent-life",
                    "name":"Sync Agent Life",
                    "description":"created through endpoint"
                }"#,
            ),
        ))
        .await
        .expect("create agent");
    assert_eq!(create_resp.status(), StatusCode::CREATED);

    let changes = collect_v4_changes(&state).await;
    let created_agent = changes.iter().find(|change| {
        change.domain == "agent_config"
            && change.record_id == agent_id
            && change.kind == "upsert"
            && change.payload["name"] == serde_json::json!("Sync Agent Life")
    });
    assert!(
        created_agent.is_some(),
        "POST /api/agents must append an agent_config v4 upsert; got {changes:#?}"
    );
    let created_security = changes.iter().find(|change| {
        change.domain == "agent_security" && change.record_id == agent_id && change.kind == "upsert"
    });
    assert!(
        created_security.is_some(),
        "POST /api/agents must append an agent_security v4 upsert; got {changes:#?}"
    );
    let created_registry = changes.iter().find(|change| {
        change.domain == "root_config"
            && change.record_id == "agents"
            && change.kind == "upsert"
            && change.payload["profiles"][agent_id].is_object()
    });
    assert!(
        created_registry.is_some(),
        "POST /api/agents must append root_config/agents v4 registry upsert; got {changes:#?}"
    );

    let update_resp = app
        .clone()
        .oneshot(json_req(
            Method::PUT,
            "/api/agents/sync-agent-life",
            Some(r#"{"name":"Sync Agent Life Updated","language":"zh-CN"}"#),
        ))
        .await
        .expect("update agent");
    assert_eq!(update_resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let updated_agent = changes.iter().find(|change| {
        change.domain == "agent_config"
            && change.record_id == agent_id
            && change.kind == "upsert"
            && change.payload["name"] == serde_json::json!("Sync Agent Life Updated")
            && change.payload["language"] == serde_json::json!("zh-CN")
    });
    assert!(
        updated_agent.is_some(),
        "PUT /api/agents/{{id}} must append an updated agent_config v4 upsert; got {changes:#?}"
    );

    let delete_resp = app
        .oneshot(json_req(
            Method::DELETE,
            "/api/agents/sync-agent-life",
            None,
        ))
        .await
        .expect("delete agent");
    assert_eq!(delete_resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let deleted_agent = changes.iter().find(|change| {
        change.domain == "agent_config" && change.record_id == agent_id && change.kind == "delete"
    });
    assert!(
        deleted_agent.is_some(),
        "DELETE /api/agents/{{id}} must append an agent_config v4 delete; got {changes:#?}"
    );
    let deleted_security = changes.iter().find(|change| {
        change.domain == "agent_security" && change.record_id == agent_id && change.kind == "delete"
    });
    assert!(
        deleted_security.is_some(),
        "DELETE /api/agents/{{id}} must append an agent_security v4 delete; got {changes:#?}"
    );
    let deleted_registry = changes.iter().find(|change| {
        change.domain == "root_config"
            && change.record_id == "agents"
            && change.kind == "upsert"
            && change.payload["profiles"][agent_id].is_null()
    });
    assert!(
        deleted_registry.is_some(),
        "DELETE /api/agents/{{id}} must append root_config/agents v4 registry upsert without the deleted profile; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_order_and_toggle_emit_root_agents_v4_when_sync_enabled() {
    let (app, state, _tmp) = build_sync_enabled_agents_app();
    let agent_id = "sync-agent-order";

    let create_resp = app
        .clone()
        .oneshot(json_req(
            Method::POST,
            "/api/agents",
            Some(
                r#"{
                    "id":"sync-agent-order",
                    "name":"Sync Agent Order",
                    "description":"created for registry tests"
                }"#,
            ),
        ))
        .await
        .expect("create agent");
    assert_eq!(create_resp.status(), StatusCode::CREATED);

    let list_resp = app
        .clone()
        .oneshot(json_req(Method::GET, "/api/agents", None))
        .await
        .expect("list agents");
    assert_eq!(list_resp.status(), StatusCode::OK);
    let current_ids = agent_ids(&read_json(list_resp).await);
    let builtin_ids = current_ids
        .iter()
        .filter(|id| id.as_str() != agent_id)
        .cloned()
        .collect::<Vec<_>>();
    let mut reordered_ids = vec![agent_id.to_string()];
    reordered_ids.extend(builtin_ids);
    let order_body =
        serde_json::to_string(&serde_json::json!({ "agent_ids": reordered_ids })).unwrap();

    let order_resp = app
        .clone()
        .oneshot(json_req(
            Method::PUT,
            "/api/agents/order",
            Some(&order_body),
        ))
        .await
        .expect("save agent order");
    assert_eq!(order_resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let expected_order = serde_json::json!(reordered_ids);
    let order_change = changes.iter().find(|change| {
        change.domain == "root_config"
            && change.record_id == "agents"
            && change.kind == "upsert"
            && change.payload["agent_order"] == expected_order
    });
    assert!(
        order_change.is_some(),
        "PUT /api/agents/order must append root_config/agents v4 with updated order; got {changes:#?}"
    );

    let toggle_resp = app
        .oneshot(json_req(
            Method::PATCH,
            "/api/agents/sync-agent-order/toggle",
            Some(r#"{"enabled":false}"#),
        ))
        .await
        .expect("toggle agent");
    assert_eq!(toggle_resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let toggle_change = changes.iter().find(|change| {
        change.domain == "root_config"
            && change.record_id == "agents"
            && change.kind == "upsert"
            && change.payload["profiles"][agent_id]["enabled"] == serde_json::json!(false)
    });
    assert!(
        toggle_change.is_some(),
        "PATCH /api/agents/{{id}}/toggle must append root_config/agents v4 with enabled=false; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rename_agent_emits_agent_config_v4_change_when_sync_enabled() {
    let (app, state, _tmp) = build_sync_enabled_agents_app();
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

    let resp = app
        .oneshot(json_req(
            Method::POST,
            "/api/agents/rename",
            Some(r#"{"old_id":"default","new_id":"Renamed Default"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["id"], "default");

    // Publication must remain observable after the background inbox consumer runs.
    state
        .repair_scan_and_drain_sync_inbox()
        .await
        .expect("drain published entries before inspecting the Replica Log");

    let report = state
        .inbound_inbox
        .repair_scan_doc_entries(&sync_doc)
        .await
        .expect("repair scan");
    assert!(
        report.scanned > 0,
        "rename should append at least one sync change"
    );

    let entries = sync_doc
        .get_many(iroh_docs::store::Query::all().build())
        .await
        .expect("query Replica Log");
    let entries = futures_util::TryStreamExt::try_collect::<Vec<_>>(entries)
        .await
        .expect("Replica Log entries");
    let mut saw_agent_config = false;
    for entry in entries {
        let Some(frame) = log
            .read_frame(entry.author(), entry.key())
            .await
            .expect("read sync frame")
        else {
            continue;
        };
        for change in frame.into_changes() {
            if change.domain == "agent_config"
                && change.record_id == "default"
                && change.payload["name"] == serde_json::json!("Renamed Default")
            {
                saw_agent_config = true;
                break;
            }
        }
        if saw_agent_config {
            break;
        }
    }

    assert!(
        saw_agent_config,
        "POST /api/agents/rename must emit agent_config v4, not only agent registry"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gap_fix_language_update_emits_agent_config_v4_change_when_sync_enabled() {
    let (app, state, _tmp) = build_sync_enabled_agents_app();
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

    let resp = app
        .oneshot(json_req(
            Method::PUT,
            "/api/agent/language",
            Some(r#"{"language":"en"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let entries = sync_doc
        .get_many(iroh_docs::store::Query::all().build())
        .await
        .expect("query Replica Log");
    let entries = futures_util::TryStreamExt::try_collect::<Vec<_>>(entries)
        .await
        .expect("Replica Log entries");
    let mut saw_agent_config = false;
    for entry in entries {
        let Some(frame) = log
            .read_frame(entry.author(), entry.key())
            .await
            .expect("read sync frame")
        else {
            continue;
        };
        for change in frame.into_changes() {
            if change.domain == "agent_config"
                && change.record_id == "default"
                && change.payload["language"] == serde_json::json!("en")
            {
                saw_agent_config = true;
                break;
            }
        }
        if saw_agent_config {
            break;
        }
    }

    assert!(
        saw_agent_config,
        "gap-fix agent language PUT must emit agent_config v4"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gap_fix_audio_mode_update_emits_root_agents_v4_change_when_sync_enabled() {
    let (app, state, _tmp) = build_sync_enabled_agents_app();
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

    let resp = app
        .oneshot(json_req(
            Method::PUT,
            "/api/agent/audio-mode",
            Some(r#"{"audio_mode":"voice"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let entries = sync_doc
        .get_many(iroh_docs::store::Query::all().build())
        .await
        .expect("query Replica Log");
    let entries = futures_util::TryStreamExt::try_collect::<Vec<_>>(entries)
        .await
        .expect("Replica Log entries");
    let mut saw_root_agents = false;
    for entry in entries {
        let Some(frame) = log
            .read_frame(entry.author(), entry.key())
            .await
            .expect("read sync frame")
        else {
            continue;
        };
        for change in frame.into_changes() {
            if change.domain == "root_config"
                && change.record_id == "agents"
                && change.payload["audio_mode"] == serde_json::json!("voice")
            {
                saw_root_agents = true;
                break;
            }
        }
        if saw_root_agents {
            break;
        }
    }

    assert!(
        saw_root_agents,
        "gap-fix audio-mode PUT must emit root_config agents v4"
    );
}
