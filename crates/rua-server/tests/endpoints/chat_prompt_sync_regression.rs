use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use serde_json::Value;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints;
use rua_server::middleware::agent_context_middleware;
use rua_server::AppState;

fn build_sync_enabled_app() -> (Router, AppState, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let ws = tmp.path().join("workspaces/default");
    std::fs::create_dir_all(&ws).expect("seed workspace");
    std::fs::write(
        ws.join("agent.json"),
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
        "sync should be enabled for v4 emit regression test"
    );

    let app = Router::new()
        .merge(endpoints::chat_prompt::root_router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state.clone());
    (app, state, tmp)
}

fn json_req(method: Method, uri: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
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
async fn put_prompt_state_emits_evergreen_chat_v4_when_sync_enabled() {
    let (app, state, _tmp) = build_sync_enabled_app();
    let chat_id = "sync-prompt-state-chat";

    let resp = app
        .oneshot(json_req(
            Method::PUT,
            &format!("/chats/prompt-state?chat_id={chat_id}"),
            r#"{
                "text": "evergreen prompt-state v4 marker",
                "files": []
            }"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let deadline = Instant::now() + Duration::from_secs(15);
    let changes = loop {
        let changes = collect_v4_changes(&state).await;
        if changes.iter().any(|change| {
            change.domain == "evergreen_chat"
                && change.record_id == format!("agent/default/{chat_id}/prompt_state")
        }) || Instant::now() >= deadline
        {
            break changes;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    let prompt_state_change = changes.iter().find(|change| {
        change.domain == "evergreen_chat"
            && change.record_id == format!("agent/default/{chat_id}/prompt_state")
    });
    assert!(
        prompt_state_change.is_some_and(|change| {
            change.kind == "file_snapshot"
                && change.payload["text"]
                    == Value::String("evergreen prompt-state v4 marker".to_string())
                && change.payload.get("mode").is_none()
        }),
        "prompt-state PUT must append an Evergreen chat v4 prompt_state snapshot; got {changes:#?}"
    );
}
