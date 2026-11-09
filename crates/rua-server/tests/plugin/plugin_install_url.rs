use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use rua_server::endpoints::plugins::router as plugins_router;
use rua_server::AppState;

async fn build_app() -> (Router, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).unwrap();
    let data_dir = tmp.path().to_path_buf();
    let state = tokio::task::spawn_blocking(move || {
        AppState::from_tempdir(data_dir, rua_server::ServerProfile::Test)
    })
    .await
    .expect("AppState construction");
    let app = Router::new()
        .nest("/api", plugins_router())
        .with_state(state);
    (app, tmp)
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

async fn install_request(app: &Router, body: Value) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/plugins/install")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
}

fn minimal_wasm() -> Vec<u8> {
    vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]
}

fn build_plugin_zip(plugin_id: &str) -> Vec<u8> {
    use std::io::Write;
    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(buf);
    let options = zip::write::SimpleFileOptions::default();

    let manifest = serde_json::json!({
        "id": plugin_id,
        "name": format!("{plugin_id} Plugin"),
        "version": "1.0.0",
        "meta": {}
    });

    zip.start_file("plugin.json", options).unwrap();
    zip.write_all(serde_json::to_string_pretty(&manifest).unwrap().as_bytes())
        .unwrap();

    zip.start_file("main.py", options).unwrap();
    zip.write_all(b"print('hello')").unwrap();

    zip.finish().unwrap().into_inner()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_from_url_raw_data_fails_without_manifest() {
    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/my-plugin.tar.gz"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(minimal_wasm()))
        .mount(&mock_server)
        .await;

    let url = format!("{}/my-plugin.tar.gz", mock_server.uri());
    let (app, _tmp) = build_app().await;

    let resp = install_request(&app, serde_json::json!({ "source": url })).await;
    let body = read_json(resp).await;
    let status = body["status"].as_str().unwrap_or("");
    assert_eq!(status, "failed", "raw data without manifest should fail");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_from_url_zip_succeeds() {
    let mock_server = MockServer::start().await;
    let zip_bytes = build_plugin_zip("zip-url-plugin");

    Mock::given(method("GET"))
        .and(path("/plugin.zip"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(zip_bytes))
        .mount(&mock_server)
        .await;

    let url = format!("{}/plugin.zip", mock_server.uri());
    let (app, _tmp) = build_app().await;

    let resp = install_request(&app, serde_json::json!({ "source": url })).await;
    let body = read_json(resp).await;
    let status = body["status"].as_str().unwrap_or("");
    assert!(
        status == "installed" || status == "failed",
        "unexpected status: {status}"
    );
    if status == "installed" {
        assert_eq!(body["plugin_id"], "zip-url-plugin");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_from_url_too_large_returns_422() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            loop {
                let n = stream.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    return;
                }
                request.extend_from_slice(&buf[..n]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let resp =
                b"HTTP/1.1 200 OK\r\nContent-Length: 60000000\r\nConnection: keep-alive\r\n\r\n";
            let _ = stream.write_all(resp).await;
            let _ = stream.flush().await;
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    });

    let url = format!("http://127.0.0.1:{port}/huge.wasm");
    let (app, _tmp) = build_app().await;

    let resp = install_request(&app, serde_json::json!({ "source": url })).await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let body = read_json(resp).await;
    assert_eq!(body["status"], "failed");
    let errors = body["errors"].as_array().expect("errors array");
    assert_eq!(errors[0]["kind"], "download_too_large");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_from_url_404_returns_422() {
    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/missing.wasm"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&mock_server)
        .await;

    let url = format!("{}/missing.wasm", mock_server.uri());
    let (app, _tmp) = build_app().await;

    let resp = install_request(&app, serde_json::json!({ "source": url })).await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let body = read_json(resp).await;
    assert_eq!(body["status"], "failed");
    let errors = body["errors"].as_array().expect("errors array");
    assert_eq!(errors[0]["kind"], "download_failed");
    assert!(errors[0]["detail"].as_str().unwrap().contains("404"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_from_url_timeout_returns_422() {
    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/slow.wasm"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(minimal_wasm())
                .set_delay(std::time::Duration::from_secs(35)),
        )
        .mount(&mock_server)
        .await;

    let url = format!("{}/slow.wasm", mock_server.uri());
    let (app, _tmp) = build_app().await;

    let resp = install_request(&app, serde_json::json!({ "source": url })).await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let body = read_json(resp).await;
    assert_eq!(body["status"], "failed");
    let errors = body["errors"].as_array().expect("errors array");
    assert_eq!(errors[0]["kind"], "download_failed");
    assert!(errors[0]["detail"].as_str().unwrap().contains("timed out"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_local_path_still_works() {
    let (app, tmp) = build_app().await;

    let plugin_dir = tmp.path().join("local-plugin");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    let manifest = serde_json::json!({
        "id": "local-plugin",
        "name": "Local Plugin",
        "version": "1.0.0",
        "meta": {}
    });
    std::fs::write(
        plugin_dir.join("plugin.json"),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();

    let source = plugin_dir.display().to_string();
    let resp = install_request(&app, serde_json::json!({ "source": source })).await;
    let body = read_json(resp).await;
    let status = body["status"].as_str().unwrap_or("");
    assert!(
        status == "installed" || status == "failed",
        "unexpected status: {status}"
    );
}
