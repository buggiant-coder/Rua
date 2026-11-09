use axum::body::Body;
use axum::http::{Method, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints;
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
            "workspace_dir": ws.to_string_lossy(),
            "language": "",
            "system_prompt_files": []
        }))
        .expect("agent json"),
    )
    .expect("seed agent");
    rua_config::atomic_write_json(&tmp.path().join("config.json"), &serde_json::json!({
        "agents": {"active_agent": "default", "agent_order": ["default"], "profiles": {"default": {"id": "default", "name": "Default", "workspace_dir": ws.to_string_lossy()}}}
    })).expect("register fixture agent");
    let sync_dir = tmp.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    std::fs::write(sync_dir.join("enabled"), "true").expect("sync enabled flag");
    rua_sync::identity::NetworkSecret::generate()
        .save(&sync_dir)
        .expect("network secret");

    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    assert!(
        state.sync_enabled.load(std::sync::atomic::Ordering::SeqCst),
        "sync should be enabled for media v4 emit regression test"
    );
    let app = Router::new()
        .nest("/api", endpoints::chat_media::router())
        .layer(axum::extract::DefaultBodyLimit::disable())
        .with_state(state.clone());
    (app, state, tmp)
}

fn multipart_req(uri: &str, file_name: &str, data: &[u8]) -> axum::extract::Request {
    let boundary = "rua-media-sync-regression-boundary";
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend_from_slice(data);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    axum::extract::Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .expect("request")
}

async fn read_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("response json")
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
async fn scoped_upload_does_not_publish_sync_in_the_foreground() {
    let (app, state, _tmp) = build_sync_enabled_app();
    let payload = b"media runtime v4 marker\n";

    let resp = app
        .oneshot(multipart_req(
            "/api/agents/default/chats/00000000-0000-4000-8000-000000000116/media",
            "sync-upload.txt",
            payload,
        ))
        .await
        .expect("send upload");
    let status = resp.status();
    let body = read_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let stored_name = body["media_id"].as_str().expect("media id");
    let stored_path = state
        .rua_root
        .join("workspaces/default/media")
        .join(stored_name);
    assert_eq!(std::fs::read(stored_path).expect("stored media"), payload);

    let changes = collect_v4_changes(&state).await;
    assert!(
        changes.iter().all(|change| change.domain != "media"),
        "foreground upload must not publish a Media Sync fact; got {changes:#?}"
    );
}
