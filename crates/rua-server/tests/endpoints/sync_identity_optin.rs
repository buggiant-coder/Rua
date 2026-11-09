use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use serial_test::serial;
use std::sync::atomic::Ordering;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::sync::router as sync_router;
use rua_server::{AppState, ServerProfile};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn sync_disabled_startup_initializes_device_identity_without_network_secret() {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed dir");

    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Desktop);

    assert!(
        !state.sync_enabled.load(Ordering::SeqCst),
        "sync_enabled should be false when no flag file"
    );

    assert!(
        state.sync_network_secret.read().unwrap().is_none(),
        "sync_network_secret should be None on disabled startup"
    );
    assert!(
        state.sync_device.read().unwrap().is_some(),
        "sync_device should exist for explicit Chat authority"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn enable_sync_does_not_create_network_secret() {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed dir");

    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Desktop);

    let sync_dir = tmp.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");

    assert!(
        state.sync_network_secret.read().unwrap().is_none(),
        "sync_network_secret must be None before enable"
    );
    assert!(
        !state.sync_enabled.load(Ordering::SeqCst),
        "sync_enabled must be false before enable"
    );

    let app = Router::new()
        .nest("/api", sync_router())
        .with_state(state.clone());

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sync/enable")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    let body: Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(body["enabled"], true);
    assert_eq!(body["identity_created"], false);
    assert_eq!(body["membership"]["state"], "enabled_no_network");

    assert!(
        state.sync_network_secret.read().unwrap().is_none(),
        "sync_network_secret must remain None after enable without network creation"
    );
    assert!(
        state.sync_enabled.load(Ordering::SeqCst),
        "sync_enabled must be true after enable"
    );

    assert!(
        sync_dir.join("enabled").exists(),
        "sync/enabled flag file must exist after enable"
    );

    assert!(
        !sync_dir.join("network_secret.key").exists(),
        "network_secret.key must not exist after enable-only"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn create_sync_network_explicitly_creates_network_secret() {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed dir");
    let sync_dir = tmp.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Desktop);
    let app = Router::new()
        .nest("/api", sync_router())
        .with_state(state.clone());

    let enable = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sync/enable")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(enable.status(), StatusCode::OK);

    let create = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sync/network/create")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(create.status(), StatusCode::OK);
    let bytes = create.into_body().collect().await.expect("body").to_bytes();
    let body: Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(body["enabled"], true);
    assert_eq!(body["created"], true);
    assert!(body["namespace_id"]
        .as_str()
        .is_some_and(|id| !id.is_empty()));
    assert!(sync_dir.join("network_secret.key").exists());
    assert!(state.sync_network_secret.read().unwrap().is_some());
    assert!(state.sync_node.lock().await.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn disable_delete_enable_reopens_sync_storage_without_restarting_app() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).unwrap();
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Desktop);
    let app = Router::new()
        .nest("/api", sync_router())
        .with_state(state.clone());
    let sync_dir = tmp.path().join("sync");
    let author = rua_sync::identity::DeviceIdentity::generate()
        .to_iroh_author()
        .id();
    let hash = iroh_blobs::Hash::new(b"old network content");
    let identity = std::fs::read(tmp.path().join("device_identity.json")).unwrap();
    let business_file = tmp.path().join("workspaces/default/keep.txt");
    std::fs::write(&business_file, "local business data").unwrap();

    for path in ["/api/sync/enable", "/api/sync/network/create"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    let old_namespace = state
        .sync_network_secret
        .read()
        .unwrap()
        .as_ref()
        .unwrap()
        .namespace_id();
    state
        .sidecar_store
        .mark_applied_op("old-operation")
        .unwrap();
    state
        .inbound_inbox
        .record_remote_insert(author, b"old-entry", hash)
        .unwrap();

    // Retain the same AppState and store handles, as a live desktop process does.
    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/sync/disable")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    assert!(
        state
            .sidecar_store
            .mark_applied_op("late-operation")
            .is_err(),
        "disabled Sync must reject writes through retained store handles"
    );
    assert!(state
        .inbound_inbox
        .record_remote_insert(author, b"late-entry", hash)
        .is_err());
    assert!(
        !sync_dir.join("sync-state.sqlite-wal").exists(),
        "all SQLite connections must close"
    );

    let trash = tmp.path().join("trash-sync");
    std::fs::rename(&sync_dir, &trash).unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/sync/diagnostics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        !sync_dir.exists(),
        "disabled diagnostics must not recreate the deleted directory"
    );

    for path in ["/api/sync/enable", "/api/sync/network/create"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    assert_ne!(
        state
            .sync_network_secret
            .read()
            .unwrap()
            .as_ref()
            .unwrap()
            .namespace_id(),
        old_namespace
    );
    assert!(!state.sidecar_store.has_applied_op("old-operation").unwrap());
    assert_eq!(
        state
            .inbound_inbox
            .count_by_state(rua_sync::inbound_inbox::InboxState::Pending)
            .unwrap(),
        0
    );
    state
        .sidecar_store
        .mark_applied_op("new-operation")
        .unwrap();
    state
        .inbound_inbox
        .record_remote_insert(author, b"new-entry", hash)
        .unwrap();
    let old = rua_sync::sidecar::SidecarStore::open(&trash).unwrap();
    assert!(old.has_applied_op("old-operation").unwrap());
    assert!(!old.has_applied_op("new-operation").unwrap());
    assert_eq!(
        std::fs::read(tmp.path().join("device_identity.json")).unwrap(),
        identity
    );
    assert_eq!(
        std::fs::read_to_string(business_file).unwrap(),
        "local business data"
    );
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sync/disable")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
