use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::prompt_library;
use rua_server::middleware::agent_context_middleware;
use rua_server::AppState;

fn build_app() -> (Router, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed default workspace");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    let app = Router::new()
        .nest("/api", prompt_library::router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
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
    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        panic!(
            "non-JSON body: {} (body={})",
            e,
            String::from_utf8_lossy(&bytes)
        )
    })
}

async fn read_bytes(resp: axum::response::Response) -> Vec<u8> {
    resp.into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes()
        .to_vec()
}

fn json_req(method: Method, uri: &str, body: Option<Value>) -> Request<Body> {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_files_empty_then_write_then_round_trip() {
    let (app, _tmp) = build_app();

    let resp = app
        .clone()
        .oneshot(json_req(Method::GET, "/api/prompt-library/files", None))
        .await
        .expect("list call");
    assert_eq!(resp.status(), StatusCode::OK);
    let listed: Value = read_json(resp).await;
    let initial = listed.as_array().expect("array").clone();
    let initial_len = initial.len();
    assert!(!initial.iter().any(|entry| entry["filename"] == "HELLO.md"));

    let resp = app
        .clone()
        .oneshot(json_req(
            Method::PUT,
            "/api/prompt-library/files/HELLO.md",
            Some(json!({ "content": "hi there" })),
        ))
        .await
        .expect("write call");
    assert_eq!(resp.status(), StatusCode::OK);
    let written: Value = read_json(resp).await;
    assert_eq!(written["filename"], "HELLO.md");

    assert_eq!(written["content"], "hi there");

    let resp = app
        .clone()
        .oneshot(json_req(
            Method::GET,
            "/api/prompt-library/files/HELLO.md",
            None,
        ))
        .await
        .expect("get call");
    assert_eq!(resp.status(), StatusCode::OK);
    let fetched: Value = read_json(resp).await;
    assert_eq!(fetched["filename"], "HELLO.md");
    assert_eq!(fetched["content"], "hi there");

    let resp = app
        .oneshot(json_req(Method::GET, "/api/prompt-library/files", None))
        .await
        .expect("list call");
    let listed: Value = read_json(resp).await;
    let arr = listed.as_array().expect("array");
    assert_eq!(arr.len(), initial_len + 1);
    assert!(arr.iter().any(|entry| entry["filename"] == "HELLO.md"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn archive_download_then_upload_round_trip_through_http() {
    let (app, _tmp) = build_app();

    let _ = app
        .clone()
        .oneshot(json_req(
            Method::PUT,
            "/api/prompt-library/files/alpha.md",
            Some(json!({ "content": "alpha-content" })),
        ))
        .await
        .expect("write alpha");
    let _ = app
        .clone()
        .oneshot(json_req(
            Method::PUT,
            "/api/prompt-library/subdirs/group/beta.md",
            Some(json!({ "content": "beta-content" })),
        ))
        .await
        .expect("write beta in subdir");

    let resp = app
        .clone()
        .oneshot(json_req(Method::GET, "/api/prompt-library/download", None))
        .await
        .expect("download call");
    assert_eq!(resp.status(), StatusCode::OK);
    let zip_bytes = read_bytes(resp).await;
    assert!(!zip_bytes.is_empty(), "zip body must not be empty");
    assert_eq!(&zip_bytes[..2], b"PK", "zip magic bytes");

    let (fresh_app, _fresh_tmp) = build_app();
    let multipart_body = build_multipart_body("file", "archive.zip", &zip_bytes);
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/prompt-library/upload")
        .header("content-type", "multipart/form-data; boundary=BOUNDARY")
        .body(Body::from(multipart_body))
        .expect("upload request");
    let resp = fresh_app.clone().oneshot(req).await.expect("upload call");
    assert_eq!(resp.status(), StatusCode::OK);
    let upload_resp: Value = read_json(resp).await;
    assert_eq!(upload_resp["success"], true);
    assert_eq!(upload_resp["message"], "Extracted zip");

    let resp = fresh_app
        .clone()
        .oneshot(json_req(
            Method::GET,
            "/api/prompt-library/files/alpha.md",
            None,
        ))
        .await
        .expect("get alpha");
    assert_eq!(resp.status(), StatusCode::OK);
    let alpha: Value = read_json(resp).await;
    assert_eq!(alpha["content"], "alpha-content");

    let resp = fresh_app
        .oneshot(json_req(
            Method::GET,
            "/api/prompt-library/subdirs/group/beta.md",
            None,
        ))
        .await
        .expect("get beta");
    assert_eq!(resp.status(), StatusCode::OK);
    let beta: Value = read_json(resp).await;
    assert_eq!(beta["content"], "beta-content");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restore_builtin_seeds_files() {
    let (app, _tmp) = build_app();
    let resp = app
        .oneshot(json_req(
            Method::POST,
            "/api/prompt-library/restore-builtin",
            None,
        ))
        .await
        .expect("restore call");
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = read_json(resp).await;
    assert_eq!(body["success"], true);
    let msg = body["message"].as_str().unwrap();
    assert!(
        msg.contains("Seeded") || msg.contains("already complete"),
        "expected seed or idempotent restore message, got: {msg}"
    );
}

fn build_multipart_body(field_name: &str, filename: &str, data: &[u8]) -> Vec<u8> {
    let boundary = "BOUNDARY";
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"{field_name}\"; filename=\"{filename}\"\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(b"Content-Type: application/zip\r\n\r\n");
    body.extend_from_slice(data);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}
