use axum::http::{header, Method, StatusCode};
use serial_test::serial;

use super::support::{build_app, preview_uri, read_body, read_json, request};

struct HomeGuard(Option<String>);

impl HomeGuard {
    fn set(path: &std::path::Path) -> Self {
        let previous = std::env::var("HOME").ok();
        std::env::set_var("HOME", path);
        Self(previous)
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        match &self.0 {
            Some(previous) => std::env::set_var("HOME", previous),
            None => std::env::remove_var("HOME"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preview_existing_text_file_streams_file_bytes() {
    let test = build_app();
    let file = test.tmp.path().join("preview_target.txt");
    std::fs::write(&file, b"preview integration payload\n").expect("write preview file");

    let resp = request(&test.app, Method::GET, &preview_uri(&file)).await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/plain; charset=utf-8")
    );
    assert!(
        resp.headers()
            .get(header::CONTENT_DISPOSITION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("preview_target.txt")),
        "expected preview filename in content-disposition"
    );
    assert_eq!(read_body(resp).await, b"preview integration payload\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_preview_streams_absolute_local_image_bytes() {
    let test = build_app();
    let file = test.tmp.path().join("camera-photo.png");
    std::fs::write(&file, b"png image payload").expect("write preview image");
    let path = file.to_string_lossy();

    let resp = request(
        &test.app,
        Method::GET,
        &format!("/api/agents/default/files/preview?path={path}"),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("image/png")
    );
    assert_eq!(
        resp.headers()
            .get(header::CONTENT_DISPOSITION)
            .and_then(|value| value.to_str().ok()),
        Some("inline; filename=\"camera-photo.png\"")
    );
    assert_eq!(read_body(resp).await, b"png image payload");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_preview_resolves_relative_paths_from_agent_workspace() {
    let test = build_app();
    let output_dir = test.tmp.path().join("workspaces/default/output");
    std::fs::create_dir_all(&output_dir).expect("create output directory");
    std::fs::write(output_dir.join("photo.webp"), b"workspace image")
        .expect("write workspace image");

    let resp = request(
        &test.app,
        Method::GET,
        "/api/agents/default/files/preview?path=output%2Fphoto.webp",
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("image/webp")
    );
    assert_eq!(read_body(resp).await, b"workspace image");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn agent_scoped_preview_expands_tilde_from_runtime_home() {
    let test = build_app();
    let _home = HomeGuard::set(test.tmp.path());
    let pictures = test.tmp.path().join("Pictures");
    std::fs::create_dir_all(&pictures).expect("create pictures directory");
    std::fs::write(pictures.join("home-photo.jpg"), b"home image").expect("write home image");

    let resp = request(
        &test.app,
        Method::GET,
        "/api/agents/default/files/preview?path=%7E%2FPictures%2Fhome-photo.jpg",
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(read_body(resp).await, b"home image");

    let resp = request(
        &test.app,
        Method::GET,
        "/api/agents/default/files/preview?path=%7E",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        read_json(resp).await,
        serde_json::json!({
            "detail": "Path is not a file"
        })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_preview_reads_file_url_sources() {
    let test = build_app();
    let file = test.tmp.path().join("file-url-photo.png");
    std::fs::write(&file, b"file url image").expect("write file url image");
    let encoded_path = file.to_string_lossy().replace('/', "%2F");

    let resp = request(
        &test.app,
        Method::GET,
        &format!("/api/agents/default/files/preview?path=file%3A%2F%2F{encoded_path}"),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(read_body(resp).await, b"file url image");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_preview_rejects_non_image_files() {
    let test = build_app();
    let file = test.tmp.path().join("report.txt");
    std::fs::write(&file, b"not an image").expect("write non-image file");
    let path = file.to_string_lossy();

    let resp = request(
        &test.app,
        Method::GET,
        &format!("/api/agents/default/files/preview?path={path}"),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        read_json(resp).await,
        serde_json::json!({
            "detail": "Path is not a supported image"
        })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_preview_accepts_uppercase_image_extensions() {
    let test = build_app();
    let file = test.tmp.path().join("PHONE-PHOTO.JPG");
    std::fs::write(&file, b"uppercase jpeg").expect("write uppercase image");
    let path = file.to_string_lossy();

    let resp = request(
        &test.app,
        Method::GET,
        &format!("/api/agents/default/files/preview?path={path}"),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("image/jpeg")
    );
    assert_eq!(read_body(resp).await, b"uppercase jpeg");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_preview_head_returns_image_metadata_without_body() {
    let test = build_app();
    let file = test.tmp.path().join("head-photo.png");
    std::fs::write(&file, b"12345").expect("write head image");
    let path = file.to_string_lossy();

    let resp = request(
        &test.app,
        Method::HEAD,
        &format!("/api/agents/default/files/preview?path={path}"),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("image/png")
    );
    assert_eq!(
        resp.headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok()),
        Some("5")
    );
    assert!(read_body(resp).await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_preview_reports_missing_files_and_directories() {
    let test = build_app();
    let missing = test.tmp.path().join("missing-photo.png");
    let missing_path = missing.to_string_lossy();

    let resp = request(
        &test.app,
        Method::GET,
        &format!("/api/agents/default/files/preview?path={missing_path}"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert!(read_json(resp).await["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains("File not found")));

    let directory = test.tmp.path().join("photo-directory");
    std::fs::create_dir_all(&directory).expect("create image directory");
    let directory_path = directory.to_string_lossy();
    let resp = request(
        &test.app,
        Method::GET,
        &format!("/api/agents/default/files/preview?path={directory_path}"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        read_json(resp).await,
        serde_json::json!({
            "detail": "Path is not a file"
        })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preview_missing_file_returns_fastapi_detail_error() {
    let test = build_app();
    let file = test.tmp.path().join("missing_preview_target.txt");

    let resp = request(&test.app, Method::GET, &preview_uri(&file)).await;

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = read_json(resp).await;
    assert!(
        body["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("File not found")),
        "expected missing file detail, got {body:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preview_directory_path_returns_rust_bad_request_detail() {
    let test = build_app();
    let dir = test.tmp.path().join("preview_dir_target");
    std::fs::create_dir_all(&dir).expect("create preview dir");

    let resp = request(&test.app, Method::GET, &preview_uri(&dir)).await;

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        read_json(resp).await,
        serde_json::json!({
            "detail": "Path is not a file"
        })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preview_head_returns_metadata_without_body() {
    let test = build_app();
    let file = test.tmp.path().join("preview_target.md");
    std::fs::write(&file, b"# Preview\n").expect("write preview file");

    let resp = request(&test.app, Method::HEAD, &preview_uri(&file)).await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/plain; charset=utf-8")
    );
    assert_eq!(
        resp.headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok()),
        Some("10")
    );
    assert!(
        resp.headers()
            .get(header::CONTENT_DISPOSITION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("preview_target.md")),
        "expected preview filename in HEAD content-disposition"
    );
    assert!(read_body(resp).await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preview_content_disposition_sanitizes_quoted_filename() {
    let test = build_app();
    let file = test.tmp.path().join("bad\"name.txt");
    std::fs::write(&file, b"quoted filename\n").expect("write preview file");

    let resp = request(&test.app, Method::GET, &preview_uri(&file)).await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get(header::CONTENT_DISPOSITION)
            .and_then(|value| value.to_str().ok()),
        Some("attachment; filename=\"bad_name.txt\"")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preview_accepts_url_encoded_absolute_paths() {
    let test = build_app();
    let file = test.tmp.path().join("preview target with spaces.txt");
    std::fs::write(&file, b"encoded path payload\n").expect("write preview file");
    let encoded = file
        .to_string_lossy()
        .trim_start_matches('/')
        .replace(' ', "%20");

    let resp = request(
        &test.app,
        Method::GET,
        &format!("/api/files/preview/{encoded}"),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(read_body(resp).await, b"encoded path payload\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preview_accepts_fully_url_encoded_absolute_paths_with_leading_slash() {
    let test = build_app();
    let file = test.tmp.path().join("fully encoded preview.txt");
    std::fs::write(&file, b"fully encoded path payload\n").expect("write preview file");
    let encoded = file
        .to_string_lossy()
        .replace('/', "%2F")
        .replace(' ', "%20");

    let resp = request(
        &test.app,
        Method::GET,
        &format!("/api/files/preview/{encoded}"),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(read_body(resp).await, b"fully encoded path payload\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preview_head_missing_file_returns_not_found_without_body() {
    let test = build_app();
    let file = test.tmp.path().join("missing_head_preview_target.txt");

    let resp = request(&test.app, Method::HEAD, &preview_uri(&file)).await;

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert!(read_body(resp).await.is_empty());
}
