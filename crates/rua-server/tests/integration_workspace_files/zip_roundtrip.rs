use std::io::{Cursor, Write};

use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use serde_json::json;
use serial_test::serial;
use tower::ServiceExt;
use zip::write::SimpleFileOptions;
use zip::ZipArchive;

use super::support::{build_app, create_agent, delete_agent, read_bytes, read_json, request};

fn multipart_body(field_name: &str, filename: &str, data: &[u8]) -> Vec<u8> {
    multipart_body_with_content_type(field_name, filename, "application/zip", data)
}

fn multipart_body_with_content_type(
    field_name: &str,
    filename: &str,
    content_type: &str,
    data: &[u8],
) -> Vec<u8> {
    let boundary = "BOUNDARY";
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"{field_name}\"; filename=\"{filename}\"\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
    body.extend_from_slice(data);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

fn zip_body(entries: &[(&str, &str)]) -> Vec<u8> {
    let cursor = Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(cursor);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, content) in entries {
        zip.start_file(name, options).expect("start zip entry");
        zip.write_all(content.as_bytes()).expect("write zip entry");
    }
    zip.finish().expect("finish zip").into_inner()
}

fn zip_names(data: &[u8]) -> Vec<String> {
    let mut archive = ZipArchive::new(Cursor::new(data)).expect("valid zip archive");
    let mut names = Vec::new();
    for index in 0..archive.len() {
        names.push(
            archive
                .by_index(index)
                .expect("zip entry")
                .name()
                .to_string(),
        );
    }
    names
}

async fn upload_zip(
    app: &axum::Router,
    uri: &str,
    agent_id: Option<&str>,
    zip_bytes: &[u8],
) -> axum::response::Response {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header("content-type", "multipart/form-data; boundary=BOUNDARY");
    if let Some(agent_id) = agent_id {
        builder = builder.header("X-Agent-Id", agent_id);
    }
    let upload_req = builder
        .body(Body::from(multipart_body(
            "file",
            "workspace.zip",
            zip_bytes,
        )))
        .expect("upload request");
    app.clone().oneshot(upload_req).await.expect("upload")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(workspace_files)]
async fn scoped_workspace_download_then_upload_restores_files() {
    let app = build_app();
    let agent_id = "integ_ws_zip";
    let file_stem = "zip_roundtrip_note";
    create_agent(&app.app, agent_id, "Workspace zip agent").await;

    let original = "# zip roundtrip original\n";
    let put_original = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}/files/{file_stem}"),
        Some(json!({"content": original})),
        None,
    )
    .await;
    assert_eq!(put_original.status(), StatusCode::OK);

    let info = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/workspace"),
        None,
        None,
    )
    .await;
    assert_eq!(info.status(), StatusCode::OK);
    let info_body = read_json(info).await;
    assert!(info_body["workspace_dir"]
        .as_str()
        .expect("workspace_dir")
        .contains(agent_id));
    assert!(info_body["file_count"].as_u64().expect("file_count") > 0);

    let download = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/workspace/download"),
        None,
        None,
    )
    .await;
    if download.status() != StatusCode::OK {
        let status = download.status();
        let body = String::from_utf8_lossy(&read_bytes(download).await).to_string();
        panic!("workspace download returned {status}: {body}");
    }
    assert_eq!(
        download
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/octet-stream")
    );
    assert!(download
        .headers()
        .get(axum::http::header::CONTENT_DISPOSITION)
        .and_then(|value| value.to_str().ok())
        .expect("content-disposition")
        .contains(&format!("workspace_{agent_id}_")));
    let zip_bytes = read_bytes(download).await;
    assert_eq!(&zip_bytes[..2], b"PK");
    assert!(
        zip_names(&zip_bytes).contains(&format!("prompts/{file_stem}.md")),
        "downloaded zip should contain the written workspace file"
    );

    let overwritten = "# zip roundtrip overwritten\n";
    let put_overwritten = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}/files/{file_stem}"),
        Some(json!({"content": overwritten})),
        None,
    )
    .await;
    assert_eq!(put_overwritten.status(), StatusCode::OK);

    let upload = upload_zip(
        &app.app,
        &format!("/api/agents/{agent_id}/workspace/upload"),
        None,
        &zip_bytes,
    )
    .await;
    assert_eq!(upload.status(), StatusCode::OK);
    let upload_body = read_json(upload).await;
    assert_eq!(upload_body["success"], true);
    assert_eq!(upload_body["agents_imported"], 1);

    let restored = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/files/{file_stem}"),
        None,
        None,
    )
    .await;
    assert_eq!(restored.status(), StatusCode::OK);
    assert_eq!(read_json(restored).await["content"], original.trim());

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(workspace_files)]
async fn root_workspace_zip_routes_use_header_agent_context_and_merge_overwrites() {
    let app = build_app();
    let source_agent = "integ_ws_zip_header_src";
    let target_agent = "integ_ws_zip_header_tgt";
    let marker_stem = "header_roundtrip_marker";
    let marker_file = format!("prompts/{marker_stem}.md");
    let marker_content = "# source marker\nfrom header-selected workspace\n";
    create_agent(&app.app, source_agent, "Workspace header zip source").await;
    create_agent(&app.app, target_agent, "Workspace header zip target").await;

    let seed_source = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/{source_agent}/files/{marker_stem}"),
        Some(json!({"content": marker_content})),
        None,
    )
    .await;
    assert_eq!(seed_source.status(), StatusCode::OK);

    let seed_target = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/{target_agent}/files/{marker_stem}"),
        Some(json!({"content": "# stale target marker\n"})),
        None,
    )
    .await;
    assert_eq!(seed_target.status(), StatusCode::OK);

    let download = request(
        &app.app,
        Method::GET,
        "/api/workspace/download",
        None,
        Some(source_agent),
    )
    .await;
    assert_eq!(download.status(), StatusCode::OK);
    assert!(download
        .headers()
        .get(axum::http::header::CONTENT_DISPOSITION)
        .and_then(|value| value.to_str().ok())
        .expect("content-disposition")
        .contains(&format!("workspace_{source_agent}_")));
    let zip_bytes = read_bytes(download).await;
    assert!(
        zip_names(&zip_bytes).contains(&marker_file),
        "root download should export the X-Agent-Id workspace, not default"
    );

    let upload = upload_zip(
        &app.app,
        "/api/workspace/upload",
        Some(target_agent),
        &zip_bytes,
    )
    .await;
    assert_eq!(upload.status(), StatusCode::OK);
    let upload_body = read_json(upload).await;
    assert_eq!(upload_body["success"], true);
    assert_eq!(upload_body["agents_imported"], 1);

    let target_after = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{target_agent}/files/{marker_stem}"),
        None,
        None,
    )
    .await;
    assert_eq!(target_after.status(), StatusCode::OK);
    assert_eq!(
        read_json(target_after).await["content"],
        marker_content.trim()
    );

    delete_agent(&app.app, source_agent).await;
    delete_agent(&app.app, target_agent).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(workspace_files)]
async fn workspace_upload_rejects_unsafe_zip_paths() {
    let app = build_app();
    let agent_id = "integ_ws_zip_unsafe";
    create_agent(&app.app, agent_id, "Workspace unsafe zip agent").await;

    let unsafe_zip = zip_body(&[("../outside.md", "must not escape\n")]);
    let upload = upload_zip(
        &app.app,
        &format!("/api/agents/{agent_id}/workspace/upload"),
        None,
        &unsafe_zip,
    )
    .await;
    assert_eq!(upload.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = read_json(upload).await;
    assert!(
        body["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("unsafe path")),
        "expected unsafe-path detail, got {body:?}"
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(workspace_files)]
async fn scoped_workspace_upload_requires_file_field() {
    let app = build_app();
    let agent_id = "integ_ws_zip_missing_file";
    create_agent(&app.app, agent_id, "Workspace zip missing file agent").await;

    let upload_req = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/agents/{agent_id}/workspace/upload"))
        .header("content-type", "multipart/form-data; boundary=BOUNDARY")
        .body(Body::from(multipart_body(
            "not_file",
            "workspace.zip",
            b"PK",
        )))
        .expect("upload request");
    let upload = app.app.clone().oneshot(upload_req).await.expect("upload");
    assert_eq!(upload.status(), StatusCode::BAD_REQUEST);
    assert!(read_json(upload).await["detail"]
        .as_str()
        .expect("detail")
        .contains("Missing 'file' field"));

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(workspace_files)]
async fn scoped_workspace_upload_rejects_invalid_zip_archive() {
    let app = build_app();
    let agent_id = "integ_ws_zip_invalid";
    create_agent(&app.app, agent_id, "Workspace invalid zip agent").await;

    let upload_body = multipart_body("file", "workspace.zip", b"not a zip archive");
    let upload_req = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/agents/{agent_id}/workspace/upload"))
        .header("content-type", "multipart/form-data; boundary=BOUNDARY")
        .body(Body::from(upload_body))
        .expect("upload request");
    let upload = app.app.clone().oneshot(upload_req).await.expect("upload");
    assert_eq!(upload.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(read_json(upload).await["detail"]
        .as_str()
        .expect("detail")
        .contains("valid zip archive"));

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(workspace_files)]
async fn scoped_workspace_upload_accepts_valid_zip_by_archive_bytes_not_part_content_type() {
    let app = build_app();
    let agent_id = "integ_ws_zip_mime_divergence";
    let marker_stem = "mime_divergence_marker";
    let marker_file = format!("prompts/{marker_stem}.md");
    create_agent(&app.app, agent_id, "Workspace zip MIME divergence agent").await;

    let zip_bytes = zip_body(&[(&marker_file, "# mime divergence\n")]);
    let upload_body =
        multipart_body_with_content_type("file", "workspace.zip", "text/plain", &zip_bytes);
    let upload_req = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/agents/{agent_id}/workspace/upload"))
        .header("content-type", "multipart/form-data; boundary=BOUNDARY")
        .body(Body::from(upload_body))
        .expect("upload request");
    let upload = app.app.clone().oneshot(upload_req).await.expect("upload");

    assert_eq!(upload.status(), StatusCode::OK);
    assert_eq!(read_json(upload).await["success"], true);

    let imported = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/files/{marker_stem}"),
        None,
        None,
    )
    .await;
    assert_eq!(imported.status(), StatusCode::OK);
    assert_eq!(read_json(imported).await["content"], "# mime divergence");

    delete_agent(&app.app, agent_id).await;
}
