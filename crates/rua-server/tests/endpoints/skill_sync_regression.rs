use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::skills::{agent_scoped_router, shared_router};
use rua_server::middleware::agent_context_middleware;
use rua_server::AppState;

fn build_app() -> (Router, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default/skills")).expect("seed workspace");
    std::fs::create_dir_all(tmp.path().join("skill_pool")).expect("seed skill_pool");

    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    assert!(
        !state
            .sync_enabled
            .load(std::sync::atomic::Ordering::Relaxed),
        "sync should be disabled for regression test"
    );

    let app = Router::new()
        .nest("/api", shared_router())
        .nest("/api", agent_scoped_router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state);
    (app, tmp)
}

fn build_sync_enabled_app() -> (Router, AppState, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default/skills")).expect("seed workspace");
    std::fs::create_dir_all(tmp.path().join("skill_pool")).expect("seed skill_pool");
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
        .nest("/api", shared_router())
        .nest("/api", agent_scoped_router())
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
async fn list_pool_sync_disabled() {
    let (app, _tmp) = build_app();
    let req = json_req(Method::GET, "/api/skills/pool", None);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_tags_sync_disabled() {
    let (app, _tmp) = build_app();

    let resp = app
        .clone()
        .oneshot(json_req(
            Method::POST,
            "/api/skills/pool/create",
            Some(
                r#"{"name":"my-skill","description":"sync disabled","content":"---\nname: my-skill\ndescription: sync disabled\n---\n"}"#,
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let req = json_req(
        Method::PUT,
        "/api/skills/pool/my-skill/tags",
        Some(r#"{"tags":["test","demo"]}"#),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_workspace_skill_emits_skill_blob_manifest_v4_when_sync_enabled() {
    let (app, state, _tmp) = build_sync_enabled_app();
    let skill_name = "sync-runtime-skill";
    let content =
        "---\nname: sync-runtime-skill\ndescription: runtime sync\n---\n\nskill-v4-marker";

    let resp = app
        .oneshot(json_req(
            Method::POST,
            "/api/agents/default/skills",
            Some(&format!(
                r#"{{
                    "name": "{skill_name}",
                    "description": "runtime sync",
                    "content": {}
                }}"#,
                serde_json::to_string(content).unwrap()
            )),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let changes = collect_v4_changes(&state).await;
    let skill_change = changes.iter().find(|change| {
        change.domain == "skill_blob_manifest"
            && change.record_id == format!("workspace/default/{skill_name}")
    });
    assert!(
        skill_change.is_some_and(|change| {
            change.payload["skill_name"] == Value::String(skill_name.to_string())
                && change.payload["scope"] == Value::String("workspace".to_string())
                && change.payload["agent_id"] == Value::String("default".to_string())
                && change.payload["files"].as_array().is_some_and(|files| {
                    files
                        .iter()
                        .any(|file| file["rel_path"] == Value::String("SKILL.md".to_string()))
                })
        }),
        "workspace skill POST must append a skill_blob_manifest v4 manifest; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_skill_delete_emits_skill_blob_manifest_v4_delete_when_sync_enabled() {
    let (app, state, _tmp) = build_sync_enabled_app();
    let skill_name = "sync-workspace-delete";
    let content =
        "---\nname: sync-workspace-delete\ndescription: runtime sync delete\n---\n\nskill-delete-v4-marker";

    let resp = app
        .clone()
        .oneshot(json_req(
            Method::POST,
            "/api/agents/default/skills",
            Some(&format!(
                r#"{{
                    "name": "{skill_name}",
                    "description": "runtime sync delete",
                    "content": {}
                }}"#,
                serde_json::to_string(content).unwrap()
            )),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .oneshot(json_req(
            Method::DELETE,
            &format!("/api/agents/default/skills/{skill_name}"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let delete_change = changes.iter().find(|change| {
        change.domain == "skill_blob_manifest"
            && change.record_id == format!("workspace/default/{skill_name}")
            && change.kind == "delete"
    });
    assert!(
        delete_change.is_some(),
        "workspace skill DELETE must append a skill_blob_manifest v4 delete; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pool_skill_create_and_delete_emit_skill_blob_manifest_v4_when_sync_enabled() {
    let (app, state, _tmp) = build_sync_enabled_app();
    let skill_name = "sync-pool-lifecycle";

    let resp = app
        .clone()
        .oneshot(json_req(
            Method::POST,
            "/api/skills/pool/create",
            Some(
                r#"{"name":"sync-pool-lifecycle","description":"runtime sync pool","content":"---\nname: sync-pool-lifecycle\ndescription: runtime sync pool\n---\n\npool-v4-marker"}"#,
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let changes = collect_v4_changes(&state).await;
    let upsert_change = changes.iter().find(|change| {
        change.domain == "skill_blob_manifest"
            && change.record_id == skill_name
            && change.kind == "upsert"
    });
    assert!(
        upsert_change.is_some_and(|change| {
            change.payload["skill_name"] == Value::String(skill_name.to_string())
                && change.payload["scope"] == Value::String("pool".to_string())
        }),
        "pool skill create must append a pool skill_blob_manifest v4 upsert; got {changes:#?}"
    );

    let resp = app
        .oneshot(json_req(
            Method::DELETE,
            &format!("/api/skills/pool/{skill_name}"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let delete_change = changes.iter().find(|change| {
        change.domain == "skill_blob_manifest"
            && change.record_id == skill_name
            && change.kind == "delete"
    });
    assert!(
        delete_change.is_some(),
        "pool skill DELETE must append a pool skill_blob_manifest v4 delete; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_skill_sync_disabled() {
    let (app, _tmp) = build_app();

    let resp = app
        .clone()
        .oneshot(json_req(
            Method::POST,
            "/api/skills/pool/create",
            Some(
                r#"{"name":"deletable","description":"test","content":"---\nname: deletable\ndescription: test\n---\n"}"#,
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let req = json_req(Method::DELETE, "/api/skills/pool/deletable", None);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
