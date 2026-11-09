use std::time::{Duration, SystemTime};

use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::files;
use rua_server::AppState;

fn build_state() -> (AppState, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed default workspace");
    std::fs::write(
        tmp.path().join("workspaces/default/agent.json"),
        serde_json::to_string_pretty(&json!({
            "id": "default",
            "name": "default",
            "system_prompt_files": [],
        }))
        .unwrap(),
    )
    .expect("write agent.json");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    (state, tmp)
}

fn build_sync_enabled_state() -> (AppState, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed default workspace");
    std::fs::write(
        tmp.path().join("workspaces/default/agent.json"),
        serde_json::to_string_pretty(&json!({
            "id": "default",
            "name": "default",
            "system_prompt_files": [],
        }))
        .unwrap(),
    )
    .expect("write agent.json");
    let sync_dir = tmp.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    std::fs::write(sync_dir.join("enabled"), "true").expect("sync enabled flag");
    rua_sync::identity::NetworkSecret::generate()
        .save(&sync_dir)
        .expect("network secret");

    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    assert!(
        state.sync_enabled.load(std::sync::atomic::Ordering::SeqCst),
        "sync should be enabled for system_prompt_files v4 emit regression test"
    );
    (state, tmp)
}

fn app(state: AppState) -> Router {
    Router::new()
        .nest("/api", files::agent_scoped_router())
        .with_state(state)
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
async fn builtin_system_prompt_virtual_file_route_is_removed() {
    let (state, _tmp) = build_state();
    let router = app(state);

    let resp = router
        .clone()
        .oneshot(req(
            Method::GET,
            "/api/agents/default/system-prompt-files/builtin",
            None,
        ))
        .await
        .expect("GET builtin");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let resp = router
        .oneshot(req(
            Method::GET,
            "/api/agents/default/system-prompt-files",
            None,
        ))
        .await
        .expect("GET files");
    assert_eq!(read_json(resp).await, json!([]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn system_prompt_files_endpoint_write_persists_then_external_read_sees() {
    let (state, tmp) = build_state();
    let router = app(state);

    let resp = router
        .clone()
        .oneshot(req(
            Method::GET,
            "/api/agents/default/system-prompt-files",
            None,
        ))
        .await
        .expect("GET initial");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body, json!([]));

    let resp = router
        .clone()
        .oneshot(req(
            Method::PUT,
            "/api/agents/default/system-prompt-files",
            Some(json!(["AGENT.md", "TOOLS.md"])),
        ))
        .await
        .expect("PUT");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body, json!(["AGENT.md", "TOOLS.md"]));

    let resp = router
        .clone()
        .oneshot(req(
            Method::GET,
            "/api/agents/default/system-prompt-files",
            None,
        ))
        .await
        .expect("GET after PUT");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body, json!(["AGENT.md", "TOOLS.md"]));

    let raw: Value = serde_json::from_str(
        &std::fs::read_to_string(tmp.path().join("workspaces/default/agent.json")).unwrap(),
    )
    .unwrap();
    let on_disk: Vec<String> = raw
        .get("system_prompt_files")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(on_disk, vec!["AGENT.md", "TOOLS.md"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn system_prompt_files_update_emits_agent_config_v4_when_sync_enabled() {
    let (state, _tmp) = build_sync_enabled_state();
    let router = app(state.clone());

    let resp = router
        .clone()
        .oneshot(req(
            Method::PUT,
            "/api/agents/default/system-prompt-files",
            Some(json!(["SYNC_SYSTEM.md", "SYNC_TOOLS.md"])),
        ))
        .await
        .expect("PUT");
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let agent_change = changes.iter().find(|change| {
        change.domain == "agent_config"
            && change.record_id == "default"
            && change.kind == "upsert"
            && change.payload["system_prompt_files"] == json!(["SYNC_SYSTEM.md", "SYNC_TOOLS.md"])
    });
    assert!(
        agent_change.is_some(),
        "system-prompt-files PUT must append an agent_config v4 upsert; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn system_prompt_files_set_then_simulated_restart_keeps_value() {
    let (state, tmp) = build_state();
    let router = app(state.clone());

    let resp = router
        .clone()
        .oneshot(req(
            Method::PUT,
            "/api/agents/default/system-prompt-files",
            Some(json!(["RESTART.md"])),
        ))
        .await
        .expect("PUT");
    assert_eq!(resp.status(), StatusCode::OK);

    drop(router);
    drop(state);

    let state2 = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    let router2 = app(state2);

    let resp = router2
        .oneshot(req(
            Method::GET,
            "/api/agents/default/system-prompt-files",
            None,
        ))
        .await
        .expect("GET after restart");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body, json!(["RESTART.md"]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn system_prompt_files_external_edit_visible_after_mtime_bump() {
    let (state, tmp) = build_state();
    let router = app(state);

    let resp = router
        .clone()
        .oneshot(req(
            Method::GET,
            "/api/agents/default/system-prompt-files",
            None,
        ))
        .await
        .expect("GET prime");
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(read_json(resp).await, json!([]));

    let agent_json_path = tmp.path().join("workspaces/default/agent.json");
    std::fs::write(
        &agent_json_path,
        serde_json::to_string_pretty(&json!({
            "id": "default",
            "name": "default",
            "system_prompt_files": ["EXTERNAL.md"],
        }))
        .unwrap(),
    )
    .unwrap();

    let later = SystemTime::now() + Duration::from_secs(2);
    let f = std::fs::OpenOptions::new()
        .write(true)
        .open(&agent_json_path)
        .unwrap();
    f.set_modified(later).unwrap();

    let resp = router
        .oneshot(req(
            Method::GET,
            "/api/agents/default/system-prompt-files",
            None,
        ))
        .await
        .expect("GET after external edit");
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(read_json(resp).await, json!(["EXTERNAL.md"]));
}
