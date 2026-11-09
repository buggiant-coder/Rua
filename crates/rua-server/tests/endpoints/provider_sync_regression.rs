use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::models;
use rua_server::middleware::agent_context_middleware;
use rua_server::AppState;

fn build_app() -> (Router, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed workspace");

    let secret = tmp.path().to_path_buf().join("secret");
    std::fs::create_dir_all(secret.join("providers").join("providers"))
        .expect("seed providers dir");

    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    assert!(
        !state
            .sync_enabled
            .load(std::sync::atomic::Ordering::Relaxed),
        "sync should be disabled for regression test"
    );

    let app = Router::new()
        .nest("/api", models::router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state);
    (app, tmp)
}

fn build_sync_enabled_app() -> (Router, AppState, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed workspace");
    std::fs::create_dir_all(tmp.path().join("secret/providers/providers"))
        .expect("seed providers dir");
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
        .nest("/api", models::router())
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
async fn list_providers_sync_disabled() {
    let (app, _tmp) = build_app();
    let req = json_req(Method::GET, "/api/models/providers", None);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn add_custom_provider_sync_disabled() {
    let (app, _tmp) = build_app();
    let body = r#"{
        "id": "test-provider",
        "name": "Test Provider",
        "base_url": "https://api.example.com/v1",
        "api_key": "sk-test",
        "models": [{"id": "test-model", "name": "Test Model"}]
    }"#;
    let req = json_req(Method::POST, "/api/models/custom-providers", Some(body));
    let resp = app.oneshot(req).await.unwrap();
    assert!(
        resp.status() == StatusCode::OK || resp.status() == StatusCode::CREATED,
        "add custom provider must succeed, got {}",
        resp.status()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn add_custom_provider_emits_provider_secret_v4_when_sync_enabled() {
    let (app, state, _tmp) = build_sync_enabled_app();
    let body = r#"{
        "id": "sync-runtime-provider",
        "name": "Sync Runtime Provider",
        "base_url": "https://api.example.com/v1",
        "api_key": "sk-sync-runtime",
        "models": [{"id": "sync-model", "name": "Sync Model"}]
    }"#;

    let resp = app
        .oneshot(json_req(
            Method::POST,
            "/api/models/custom-providers",
            Some(body),
        ))
        .await
        .unwrap();
    assert!(
        resp.status() == StatusCode::OK || resp.status() == StatusCode::CREATED,
        "add custom provider must succeed, got {}",
        resp.status()
    );

    let changes = collect_v4_changes(&state).await;
    let provider_change = changes.iter().find(|change| {
        change.domain == "provider_secret" && change.record_id == "sync-runtime-provider"
    });
    assert!(
        provider_change.is_some_and(|change| {
            change.payload["api_key"] == Value::String("sk-sync-runtime".to_string())
                && change.payload["name"] == Value::String("Sync Runtime Provider".to_string())
        }),
        "custom provider POST must append a provider_secret v4 change with logical provider payload; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_model_set_and_clear_remain_device_local_when_sync_enabled() {
    let (app, state, _tmp) = build_sync_enabled_app();
    let provider_body = r#"{
        "id": "sync-active-provider",
        "name": "Sync Active Provider",
        "base_url": "https://api.example.com/v1",
        "api_key": "sk-sync-active",
        "models": [{"id": "sync-active-model", "name": "Sync Active Model"}]
    }"#;
    let resp = app
        .clone()
        .oneshot(json_req(
            Method::POST,
            "/api/models/custom-providers",
            Some(provider_body),
        ))
        .await
        .unwrap();
    assert!(
        resp.status() == StatusCode::OK || resp.status() == StatusCode::CREATED,
        "add custom provider must succeed before active model update, got {}",
        resp.status()
    );

    let active_body = r#"{
        "provider_id": "sync-active-provider",
        "model": "sync-active-model"
    }"#;
    let resp = app
        .clone()
        .oneshot(json_req(
            Method::PUT,
            "/api/models/providers/active",
            Some(active_body),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    assert!(
        changes.iter().all(|change| {
            change.domain != "root_config" || change.record_id != "default_model"
        }),
        "active model PUT must not publish device-local default_model; got {changes:#?}"
    );

    let resp = app
        .oneshot(json_req(
            Method::DELETE,
            "/api/models/providers/active",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    assert!(
        changes.iter().all(|change| {
            change.domain != "root_config" || change.record_id != "default_model"
        }),
        "active model DELETE must not publish device-local default_model; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_update_and_delete_emit_provider_secret_v4_when_sync_enabled() {
    let (app, state, _tmp) = build_sync_enabled_app();
    let body = r#"{
        "id": "sync-provider-lifecycle",
        "name": "Sync Provider Lifecycle",
        "base_url": "https://api.example.com/v1",
        "api_key": "sk-sync-lifecycle",
        "models": [{"id": "sync-life-model", "name": "Sync Life Model"}]
    }"#;
    let resp = app
        .clone()
        .oneshot(json_req(
            Method::POST,
            "/api/models/custom-providers",
            Some(body),
        ))
        .await
        .unwrap();
    assert!(
        resp.status() == StatusCode::OK || resp.status() == StatusCode::CREATED,
        "add custom provider must succeed before update/delete, got {}",
        resp.status()
    );

    let update = r#"{
        "name": "Sync Provider Lifecycle Updated",
        "base_url": "https://api.example.com/v2",
        "api_key": "sk-sync-lifecycle-updated",
        "models": [{"id": "sync-life-model-2", "name": "Sync Life Model 2"}]
    }"#;
    let resp = app
        .clone()
        .oneshot(json_req(
            Method::PUT,
            "/api/models/providers/sync-provider-lifecycle",
            Some(update),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let updated_provider = changes.iter().find(|change| {
        change.domain == "provider_secret"
            && change.record_id == "sync-provider-lifecycle"
            && change.kind == "upsert"
            && change.payload["name"] == "Sync Provider Lifecycle Updated"
    });
    assert!(
        updated_provider.is_some_and(|change| {
            change.payload["api_key"] == "sk-sync-lifecycle-updated"
                && change.payload["base_url"] == "https://api.example.com/v2"
                && change.payload["models"][0]["id"] == "sync-life-model-2"
        }),
        "provider PUT must append a provider_secret v4 upsert with logical provider payload; got {changes:#?}"
    );

    let resp = app
        .oneshot(json_req(
            Method::DELETE,
            "/api/models/providers/sync-provider-lifecycle",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let provider_delete = changes.iter().find(|change| {
        change.domain == "provider_secret"
            && change.record_id == "sync-provider-lifecycle"
            && change.kind == "delete"
    });
    assert!(
        provider_delete.is_some(),
        "provider DELETE must append a provider_secret v4 delete; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_model_add_update_delete_emit_provider_secret_v4_when_sync_enabled() {
    let (app, state, _tmp) = build_sync_enabled_app();
    let provider_id = "sync-provider-model-life";
    let body = r#"{
        "id": "sync-provider-model-life",
        "name": "Sync Provider Model Life",
        "base_url": "https://api.example.com/v1",
        "api_key": "sk-sync-model-life",
        "models": []
    }"#;
    let resp = app
        .clone()
        .oneshot(json_req(
            Method::POST,
            "/api/models/custom-providers",
            Some(body),
        ))
        .await
        .unwrap();
    assert!(
        resp.status() == StatusCode::OK || resp.status() == StatusCode::CREATED,
        "add custom provider must succeed before model lifecycle, got {}",
        resp.status()
    );

    let model_body = r#"{
        "id": "sync-model-life",
        "name": "Sync Model Life",
        "supports_image": true,
        "probe_source": "manual"
    }"#;
    let resp = app
        .clone()
        .oneshot(json_req(
            Method::POST,
            "/api/models/sync-provider-model-life/models",
            Some(model_body),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let changes = collect_v4_changes(&state).await;
    let added_model = changes.iter().find(|change| {
        change.domain == "provider_secret"
            && change.record_id == provider_id
            && change.kind == "upsert"
            && change.payload["models"].as_array().is_some_and(|models| {
                models.iter().any(|model| {
                    model["id"] == "sync-model-life"
                        && model["name"] == "Sync Model Life"
                        && model["supports_image"] == true
                        && model["probe_source"] == "manual"
                })
            })
    });
    assert!(
        added_model.is_some(),
        "model POST must append a provider_secret v4 upsert containing the added model; got {changes:#?}"
    );

    let update_body = r#"{
        "name": "Sync Model Life Updated",
        "supports_video": true,
        "probe_source": "updated"
    }"#;
    let resp = app
        .clone()
        .oneshot(json_req(
            Method::PUT,
            "/api/models/sync-provider-model-life/models/sync-model-life/config",
            Some(update_body),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let updated_model = changes.iter().find(|change| {
        change.domain == "provider_secret"
            && change.record_id == provider_id
            && change.kind == "upsert"
            && change.payload["models"].as_array().is_some_and(|models| {
                models.iter().any(|model| {
                    model["id"] == "sync-model-life"
                        && model["name"] == "Sync Model Life Updated"
                        && model["supports_video"] == true
                        && model["probe_source"] == "updated"
                })
            })
    });
    assert!(
        updated_model.is_some(),
        "model config PUT must append a provider_secret v4 upsert containing the updated model; got {changes:#?}"
    );

    let resp = app
        .oneshot(json_req(
            Method::DELETE,
            "/api/models/sync-provider-model-life/models/sync-model-life",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let deleted_model = changes.iter().find(|change| {
        change.domain == "provider_secret"
            && change.record_id == provider_id
            && change.kind == "upsert"
            && change.payload["models"]
                .as_array()
                .is_some_and(|models| !models.iter().any(|model| model["id"] == "sync-model-life"))
    });
    assert!(
        deleted_model.is_some(),
        "model DELETE must append a provider_secret v4 upsert without the removed model; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_provider_sync_disabled() {
    let (app, _tmp) = build_app();
    let req = json_req(Method::GET, "/api/models/nonexistent", None);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
