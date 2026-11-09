#![cfg(unix)]

use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::tunnel_status;
use rua_server::AppState;

fn write_fixture_script(dir: &std::path::Path, name: &str, content: &str) {
    std::fs::create_dir_all(dir).unwrap();
    let script = dir.join(name);
    std::fs::write(&script, content).unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(&script).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script, perms).unwrap();
}

async fn build_app(tmp: &TempDir) -> (Router, AppState) {
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).unwrap();
    let data_dir = tmp.path().to_path_buf();
    let state = tokio::task::spawn_blocking(move || {
        AppState::from_tempdir(data_dir, rua_server::ServerProfile::Test)
    })
    .await
    .unwrap();
    let app = Router::new()
        .nest("/api", tunnel_status::router())
        .with_state(state.clone());
    (app, state)
}

async fn read_json(resp: axum::response::Response) -> Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("response is JSON")
}

#[tokio::test(flavor = "multi_thread")]
async fn status_returns_disconnected_when_no_tunnel() {
    let tmp = TempDir::new().unwrap();
    let (app, _state) = build_app(&tmp).await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/tunnel/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["status"], "disconnected");
    assert!(body.get("public_url").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_returns_stopped_when_no_tunnel() {
    let tmp = TempDir::new().unwrap();
    let (app, _state) = build_app(&tmp).await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/tunnel/stop")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["status"], "stopped");
}

#[tokio::test(flavor = "multi_thread")]
async fn start_then_status_then_stop() {
    let tmp = TempDir::new().unwrap();
    let tunnel_dir = tmp.path().join("tunnel");
    write_fixture_script(
        &tunnel_dir,
        "cloudflared",
        "#!/bin/sh\n\
         echo '2024-01-01T00:00:00Z INF |  https://test-integ.trycloudflare.com  |' >&2\n\
         sleep 60\n",
    );

    let (app, _state) = build_app(&tmp).await;

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/tunnel/start")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"local_port":8080}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["status"], "running");
    assert!(body["public_url"]
        .as_str()
        .unwrap()
        .contains("trycloudflare.com"));
    assert!(body["public_wss_url"]
        .as_str()
        .unwrap()
        .starts_with("wss://"));
    assert_eq!(body["local_port"], 8080);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/tunnel/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["status"], "running");
    assert!(body["public_url"].as_str().is_some());

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/tunnel/stop")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["status"], "stopped");

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/tunnel/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["status"], "disconnected");
}

#[tokio::test(flavor = "multi_thread")]
async fn start_twice_returns_conflict() {
    let tmp = TempDir::new().unwrap();
    let tunnel_dir = tmp.path().join("tunnel");
    write_fixture_script(
        &tunnel_dir,
        "cloudflared",
        "#!/bin/sh\n\
         echo 'https://conflict-test.trycloudflare.com' >&2\n\
         sleep 60\n",
    );

    let (app, _state) = build_app(&tmp).await;

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/tunnel/start")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"local_port":9090}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/tunnel/start")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"local_port":9091}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = read_json(resp).await;
    assert!(body["detail"].as_str().unwrap().contains("already running"));

    let _ = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/tunnel/stop")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
}
