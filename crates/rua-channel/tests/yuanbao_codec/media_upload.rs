use std::collections::HashMap;
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{post, put};
use axum::{Json, Router};
use rua_channel::yuanbao::media::{
    build_file_msg_body, build_image_msg_body, download_and_upload_media,
};
use tempfile::tempdir;
use tokio::sync::Mutex;

#[derive(Clone, Default)]
struct UploadState {
    gen_headers: Arc<Mutex<Option<HeaderMap>>>,
    gen_body: Arc<Mutex<Option<serde_json::Value>>>,
    put_body: Arc<Mutex<Option<Vec<u8>>>>,
    upload_url: Arc<Mutex<String>>,
}

#[tokio::test]
async fn upload_local_image_builds_tim_image_body() {
    let state = UploadState::default();
    let app = Router::new()
        .route("/api/resource/genUploadInfo", post(gen_upload_info))
        .route("/cos/upload", put(cos_upload))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind yuanbao media fixture");
    let addr = listener.local_addr().expect("fixture addr");
    *state.upload_url.lock().await = format!("http://127.0.0.1:{}/cos/upload", addr.port());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve fixture");
    });

    let temp = tempdir().unwrap();
    let image_path = temp.path().join("image.png");
    std::fs::write(&image_path, png_1x1()).unwrap();
    let auth_headers = HashMap::from([
        ("X-ID".to_string(), "bot-id".to_string()),
        ("X-Token".to_string(), "token".to_string()),
        ("X-Source".to_string(), "bot".to_string()),
    ]);
    let client = reqwest::Client::new();

    let result = download_and_upload_media(
        image_path.to_str().unwrap(),
        &client,
        &format!("127.0.0.1:{}", addr.port()),
        &auth_headers,
    )
    .await
    .expect("upload image");

    assert_eq!(result.filename, "image.png");
    assert_eq!(result.mime_type, "image/png");
    assert_eq!(result.width, 1);
    assert_eq!(result.height, 1);
    assert_eq!(result.url, "https://cdn.example.com/image.png");
    assert_eq!(state.put_body.lock().await.clone().unwrap(), png_1x1());
    let headers = state.gen_headers.lock().await.clone().unwrap();
    assert_eq!(
        headers.get("x-id").and_then(|v| v.to_str().ok()),
        Some("bot-id")
    );
    assert_eq!(
        state.gen_body.lock().await.clone().unwrap()["fileName"],
        "image.png"
    );

    let body = build_image_msg_body(&result);
    assert_eq!(body[0].0, "TIMImageElem");
    assert_eq!(body[0].1["uuid"].as_str().unwrap().len(), 32);
    assert_eq!(body[0].1["image_format"], 255);
    assert_eq!(body[0].1["image_info_array"][0]["url"], result.url);
    assert_eq!(body[0].1["image_info_array"][0]["width"], 1);

    server.abort();
}

#[test]
fn build_file_msg_body_uses_qwenpaw_tim_file_shape() {
    let result = rua_channel::yuanbao::media::UploadResult {
        url: "https://cdn.example.com/report.pdf".into(),
        filename: "report.pdf".into(),
        size: 123,
        mime_type: "application/pdf".into(),
        uuid_hex: "0123456789abcdef0123456789abcdef".into(),
        width: 0,
        height: 0,
    };

    let body = build_file_msg_body(&result);
    assert_eq!(
        body,
        vec![(
            "TIMFileElem".to_string(),
            serde_json::json!({
                "uuid": "0123456789abcdef0123456789abcdef",
                "file_name": "report.pdf",
                "file_size": 123,
                "url": "https://cdn.example.com/report.pdf",
            })
        )]
    );
}

async fn gen_upload_info(
    State(state): State<UploadState>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    *state.gen_headers.lock().await = Some(headers);
    *state.gen_body.lock().await = Some(body);
    let upload_url = state.upload_url.lock().await.clone();
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "data": {
                "upload_full_url": upload_url,
                "resourceUrl": "https://cdn.example.com/image.png"
            }
        })),
    )
}

async fn cos_upload(State(state): State<UploadState>, body: Body) -> StatusCode {
    let bytes = to_bytes(body, 1024 * 1024)
        .await
        .expect("cos body")
        .to_vec();
    *state.put_body.lock().await = Some(bytes);
    StatusCode::NO_CONTENT
}

fn png_1x1() -> Vec<u8> {
    let mut data = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    data.extend_from_slice(&[0, 0, 0, 13]);
    data.extend_from_slice(b"IHDR");
    data.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 1]);
    data.extend_from_slice(&[8, 2, 0, 0, 0]);
    data
}
