use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::files;
use rua_server::middleware::agent_context_middleware;
use rua_server::AppState;

fn build_app() -> (Router, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let ws = tmp.path().join("workspaces/default");
    std::fs::create_dir_all(&ws).expect("seed workspace");

    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    assert!(
        !state
            .sync_enabled
            .load(std::sync::atomic::Ordering::Relaxed),
        "sync should be disabled for regression test"
    );

    let app = Router::new()
        .nest("/api", files::root_router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state);
    (app, tmp)
}

fn build_sync_enabled_app() -> (Router, AppState, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let ws = tmp.path().join("workspaces/default");
    std::fs::create_dir_all(&ws).expect("seed workspace");
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
        "sync should be enabled for v4 emit regression test"
    );

    let app = Router::new()
        .nest("/api", files::root_router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state.clone());
    (app, state, tmp)
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
async fn write_workspace_prompt_sync_disabled() {
    let (app, _tmp) = build_app();
    let req = json_req(
        Method::PUT,
        "/api/agent/files/AGENT.md",
        Some(r##"{"content":"# Updated Prompt"}"##),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn write_workspace_prompt_emits_agent_prompt_file_v4_when_sync_enabled() {
    let (app, state, _tmp) = build_sync_enabled_app();
    let content = "# Updated Prompt\n\nworkspace-prompt-v4-marker";

    let resp = app
        .oneshot(json_req(
            Method::PUT,
            "/api/agent/files/AGENT.md",
            Some(&format!(
                r#"{{"content":{}}}"#,
                serde_json::to_string(content).unwrap()
            )),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let prompt_change = changes.iter().find(|change| {
        change.domain == "agent_prompt_file" && change.record_id == "default/root/AGENT.md"
    });
    assert!(
        prompt_change.is_some_and(|change| {
            change.payload["content"] == Value::String(content.to_string())
        }),
        "workspace prompt PUT must append a agent_prompt_file v4 change with file content; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rename_and_delete_workspace_file_emit_agent_prompt_file_v4_when_sync_enabled() {
    let (app, state, _tmp) = build_sync_enabled_app();
    let original = "sync-workspace-note.md";
    let renamed = "sync-workspace-note-renamed.md";
    let content = "# Workspace Note\n\nworkspace-file-lifecycle-v4-marker";

    let resp = app
        .clone()
        .oneshot(json_req(
            Method::PUT,
            &format!("/api/agent/files/{original}"),
            Some(&format!(
                r#"{{"content":{}}}"#,
                serde_json::to_string(content).unwrap()
            )),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(json_req(
            Method::POST,
            &format!("/api/agent/files/{original}/rename"),
            Some(&format!(r#"{{"new_name":"{renamed}"}}"#)),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let old_delete = changes.iter().find(|change| {
        change.domain == "agent_prompt_file"
            && change.record_id == format!("default/root/{original}")
            && change.kind == "delete"
    });
    let new_upsert = changes.iter().find(|change| {
        change.domain == "agent_prompt_file"
            && change.record_id == format!("default/root/{renamed}")
            && change.kind == "upsert"
    });
    assert!(
        old_delete.is_some(),
        "workspace file rename must append a agent_prompt_file v4 delete for old filename; got {changes:#?}"
    );
    assert!(
        new_upsert.is_some_and(|change| {
            change.payload["content"] == Value::String(content.to_string())
        }),
        "workspace file rename must append a agent_prompt_file v4 upsert for new filename; got {changes:#?}"
    );

    let resp = app
        .oneshot(json_req(
            Method::DELETE,
            &format!("/api/agent/files/{renamed}"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let renamed_delete = changes.iter().find(|change| {
        change.domain == "agent_prompt_file"
            && change.record_id == format!("default/root/{renamed}")
            && change.kind == "delete"
    });
    assert!(
        renamed_delete.is_some(),
        "workspace file DELETE must append a agent_prompt_file v4 delete; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn file_shaped_memory_write_route_is_removed() {
    let (app, _tmp) = build_app();
    let req = json_req(
        Method::PUT,
        "/api/agent/memory/2025-06-01.md",
        Some(r#"{"content":"Updated daily entry."}"#),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn file_shaped_memory_list_route_is_removed() {
    let (app, _tmp) = build_app();
    let req = json_req(Method::GET, "/api/agent/memory", None);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn file_shaped_memory_get_route_is_removed() {
    let (app, _tmp) = build_app();
    let req = json_req(Method::GET, "/api/agent/memory/2025-06-01.md", None);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
