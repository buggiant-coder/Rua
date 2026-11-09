use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::cron;
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

fn build_state(tmp: &TempDir) -> AppState {
    AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test)
}

fn build_sync_enabled_state(tmp: &TempDir) -> AppState {
    let sync_dir = tmp.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    std::fs::write(sync_dir.join("enabled"), "true").expect("sync enabled flag");
    rua_sync::identity::NetworkSecret::generate()
        .save(&sync_dir)
        .expect("network secret");

    let state = build_state(tmp);
    assert!(
        state.sync_enabled.load(std::sync::atomic::Ordering::SeqCst),
        "sync should be enabled for device-local cron regression test"
    );
    state
}

fn build_app(state: AppState) -> Router {
    Router::new()
        .nest(
            "/api",
            cron::root_router().merge(cron::agent_scoped_router()),
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
    if bytes.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
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

fn create_payload(name: &str, cron: &str) -> Value {
    json!({
        "name": name,
        "schedule": { "cron": cron, "timezone": "UTC" },
        "task_type": "text",
        "text": format!("trigger {name}"),
        "dispatch": {
            "channel": "console",
            "target": { "user_id": "default", "session_id": "test-session" },
            "mode": "stream"
        }
    })
}

async fn create_job_and_get_id(app: &Router, payload: Value) -> String {
    let resp = app
        .clone()
        .oneshot(req("POST", "/api/cron/jobs", Some(payload)))
        .await
        .expect("send POST /cron/jobs");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = read_json(resp).await;
    body["id"].as_str().expect("id").to_string()
}

async fn list_ids(app: &Router) -> Vec<String> {
    let resp = app
        .clone()
        .oneshot(req("GET", "/api/cron/jobs", None))
        .await
        .expect("send GET /cron/jobs");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    body.as_array()
        .expect("list array")
        .iter()
        .filter_map(|j| j["id"].as_str().map(String::from))
        .collect()
}

async fn read_paused_from_disk(tmp: &TempDir, job_id: &str) -> Option<bool> {
    let path = tmp.path().join("workspaces/default/jobs.json");
    let raw = tokio::fs::read_to_string(&path).await.ok()?;
    let json: Value = serde_json::from_str(&raw).ok()?;
    json["jobs"][job_id]["paused"].as_bool()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pause_then_simulated_restart_keeps_paused() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);

    let state = build_state(&tmp);
    let app = build_app(state);
    let job_id = create_job_and_get_id(&app, create_payload("paused-survivor", "0 * * * *")).await;

    let resp = app
        .clone()
        .oneshot(req("POST", &format!("/api/cron/jobs/{job_id}/pause"), None))
        .await
        .expect("send pause");
    assert_eq!(resp.status(), StatusCode::OK);

    let on_disk = read_paused_from_disk(&tmp, &job_id)
        .await
        .expect("paused flag on disk");
    assert!(on_disk, "paused must be persisted to disk; got {on_disk}");

    drop(app);
    let state2 = build_state(&tmp);
    let app2 = build_app(state2);

    let resp = app2
        .clone()
        .oneshot(req("GET", &format!("/api/cron/jobs/{job_id}"), None))
        .await
        .expect("send GET after restart");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;

    let after_restart = read_paused_from_disk(&tmp, &job_id)
        .await
        .expect("paused flag after restart");
    assert!(
        after_restart,
        "paused must survive restart; got {after_restart} for {body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_then_simulated_restart_keeps_active() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);

    let state = build_state(&tmp);
    let app = build_app(state);
    let job_id = create_job_and_get_id(&app, create_payload("ping-pong", "0 * * * *")).await;

    let _ = app
        .clone()
        .oneshot(req("POST", &format!("/api/cron/jobs/{job_id}/pause"), None))
        .await
        .unwrap();
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/api/cron/jobs/{job_id}/resume"),
            None,
        ))
        .await
        .expect("send resume");
    assert_eq!(resp.status(), StatusCode::OK);

    let on_disk = read_paused_from_disk(&tmp, &job_id)
        .await
        .expect("paused flag on disk");
    assert!(!on_disk, "after resume, paused=false on disk");

    drop(app);
    let _ = build_state(&tmp);
    let after = read_paused_from_disk(&tmp, &job_id)
        .await
        .expect("paused flag after restart");
    assert!(!after, "paused=false survives restart");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn add_job_appears_in_list_immediately() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    let app = build_app(build_state(&tmp));

    let job_id = create_job_and_get_id(&app, create_payload("immediate", "0 * * * *")).await;
    let ids = list_ids(&app).await;
    assert!(
        ids.contains(&job_id),
        "POST /cron/jobs must show up in next GET /cron/jobs immediately; got {ids:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_job_disappears_from_list_immediately() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    let app = build_app(build_state(&tmp));

    let job_id = create_job_and_get_id(&app, create_payload("doomed", "0 * * * *")).await;
    assert!(list_ids(&app).await.contains(&job_id));

    let resp = app
        .clone()
        .oneshot(req("DELETE", &format!("/api/cron/jobs/{job_id}"), None))
        .await
        .expect("send DELETE");
    assert_eq!(resp.status(), StatusCode::OK);

    let ids = list_ids(&app).await;
    assert!(
        !ids.contains(&job_id),
        "DELETE /cron/jobs/{{id}} must hide it from the next GET; got {ids:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_job_persists_to_disk() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    let app = build_app(build_state(&tmp));

    let job_id = create_job_and_get_id(&app, create_payload("renamer", "0 * * * *")).await;

    let resp = app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/api/cron/jobs/{job_id}"),
            Some(json!({ "name": "renamed-on-disk" })),
        ))
        .await
        .expect("send PUT");
    assert_eq!(resp.status(), StatusCode::OK);

    let path = tmp.path().join("workspaces/default/jobs.json");
    let raw = tokio::fs::read_to_string(&path).await.expect("read disk");
    let json: Value = serde_json::from_str(&raw).expect("parse json");
    assert_eq!(
        json["jobs"][&job_id]["name"].as_str(),
        Some("renamed-on-disk"),
        "PUT /cron/jobs/{{id}} must persist to disk; got {raw}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cron_job_lifecycle_does_not_emit_v4_when_sync_enabled() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    let state = build_sync_enabled_state(&tmp);
    let app = build_app(state.clone());

    let job_id =
        create_job_and_get_id(&app, create_payload("device-local-cron", "0 * * * *")).await;

    let resp = app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/api/cron/jobs/{job_id}"),
            Some(json!({ "name": "device-local-cron-updated" })),
        ))
        .await
        .expect("send PUT");
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(req("POST", &format!("/api/cron/jobs/{job_id}/pause"), None))
        .await
        .expect("send pause");
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(req("DELETE", &format!("/api/cron/jobs/{job_id}"), None))
        .await
        .expect("send DELETE");
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    assert!(
        changes.is_empty(),
        "cron is device-local and job create/update/pause/delete must not append v4 sync changes; got {changes:#?}"
    );
}
