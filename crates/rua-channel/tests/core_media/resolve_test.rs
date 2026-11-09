use std::path::PathBuf;

use base64::Engine;
use rua_channel::core::content::OutgoingContentPart;
use rua_channel::core::media::{resolve_outgoing_media, sanitize_filename};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn http_client() -> reqwest::Client {
    reqwest::Client::new()
}

async fn spawn_http_response(status: u16, body: &'static [u8]) -> String {
    spawn_http_response_with_headers(status, &[], body).await
}

async fn spawn_http_response_with_headers(
    status: u16,
    headers: &[(&'static str, &'static str)],
    body: &'static [u8],
) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let headers = headers.to_vec();

    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0; 1024];
        let _ = socket.read(&mut buf).await.unwrap();

        let reason = match status {
            200 => "OK",
            404 => "Not Found",
            _ => "Error",
        };
        let mut response = format!(
            "HTTP/1.1 {status} {reason}\r\ncontent-length: {}\r\nconnection: close\r\n",
            body.len()
        );
        for (name, value) in headers {
            response.push_str(name);
            response.push_str(": ");
            response.push_str(value);
            response.push_str("\r\n");
        }
        response.push_str("\r\n");
        socket.write_all(response.as_bytes()).await.unwrap();
        socket.write_all(body).await.unwrap();
        let _ = socket.shutdown().await;
    });

    format!("http://{addr}")
}

async fn spawn_http_raw_response(response: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0; 1024];
        let _ = socket.read(&mut buf).await.unwrap();
        socket.write_all(response.as_bytes()).await.unwrap();
        let _ = socket.shutdown().await;
    });

    format!("http://{addr}")
}

#[test]
fn sanitize_strips_directory_components() {
    assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
}

#[test]
fn sanitize_rejects_bare_dotdot() {
    assert_eq!(sanitize_filename(".."), "file.bin");
}

#[test]
fn sanitize_rejects_bare_dot() {
    assert_eq!(sanitize_filename("."), "file.bin");
}

#[test]
fn sanitize_preserves_normal_name() {
    assert_eq!(sanitize_filename("photo.png"), "photo.png");
}

#[test]
fn sanitize_rejects_empty() {
    assert_eq!(sanitize_filename(""), "file.bin");
}

#[test]
fn sanitize_strips_leading_path() {
    assert_eq!(sanitize_filename("/tmp/evil/file.txt"), "file.txt");
}

#[tokio::test]
async fn decode_data_url_png() {
    let png_1px = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
    let data_url = format!("data:image/png;base64,{png_1px}");

    let part = OutgoingContentPart::Image {
        url: Some(data_url),
        path: None,
        alt: Some("test".into()),
    };

    let result = resolve_outgoing_media(&part, &http_client())
        .await
        .expect("should succeed");
    let resolved = result.expect("should return Some");

    assert!(resolved.path.exists(), "decoded file should exist on disk");
    let contents = std::fs::read(&resolved.path).unwrap();
    assert!(!contents.is_empty());
    assert!(resolved.name.ends_with(".png"));

    std::fs::remove_file(&resolved.path).ok();
}

#[tokio::test]
async fn decode_data_url_uses_alt_as_name() {
    let data_url = "data:image/jpeg;base64,/9j/4AAQSkZJRg==";

    let part = OutgoingContentPart::Image {
        url: Some(data_url.into()),
        path: None,
        alt: Some("my_photo".into()),
    };

    let result = resolve_outgoing_media(&part, &http_client()).await;
    match result {
        Ok(Some(resolved)) => {
            assert!(resolved.name.starts_with("my_photo"));
            std::fs::remove_file(&resolved.path).ok();
        }
        Ok(None) => panic!("expected Some"),
        Err(_) => {}
    }
}

#[tokio::test]
async fn decode_data_url_uses_mime_extension_after_sanitizing_name_hint() {
    let pdf_bytes = b"%PDF-1.4\n";
    let data_url = format!(
        "data:application/pdf;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(pdf_bytes)
    );

    let part = OutgoingContentPart::File {
        url: Some(data_url),
        path: None,
        name: Some("../../report".into()),
    };

    let resolved = resolve_outgoing_media(&part, &http_client())
        .await
        .expect("data URL decode should succeed")
        .expect("decoded media");

    assert_eq!(resolved.name, "report.pdf");
    assert_eq!(
        std::fs::read(&resolved.path).expect("decoded file"),
        pdf_bytes
    );

    std::fs::remove_file(&resolved.path).ok();
}

#[tokio::test]
async fn resolve_audio_data_url_uses_audio_name_and_extension() {
    let bytes = b"OggS audio bytes";
    let data_url = format!(
        "data:audio/ogg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    );
    let part = OutgoingContentPart::Audio {
        url: Some(data_url),
        path: None,
        name: Some("../../voice".into()),
    };

    let resolved = resolve_outgoing_media(&part, &http_client())
        .await
        .expect("audio data URL should resolve")
        .expect("resolved audio");

    assert_eq!(resolved.name, "voice.ogg");
    assert_eq!(std::fs::read(&resolved.path).expect("decoded audio"), bytes);
    std::fs::remove_file(&resolved.path).ok();
}

#[tokio::test]
async fn resolve_video_data_url_uses_video_name_and_extension() {
    let bytes = b"\0\0\0 ftypisom";
    let data_url = format!(
        "data:video/mp4;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    );
    let part = OutgoingContentPart::Video {
        url: Some(data_url),
        path: None,
        name: Some("../../clip".into()),
    };

    let resolved = resolve_outgoing_media(&part, &http_client())
        .await
        .expect("video data URL should resolve")
        .expect("resolved video");

    assert_eq!(resolved.name, "clip.mp4");
    assert_eq!(std::fs::read(&resolved.path).expect("decoded video"), bytes);
    std::fs::remove_file(&resolved.path).ok();
}

#[tokio::test]
async fn data_url_rejects_invalid_base64_before_channel_send() {
    let part = OutgoingContentPart::File {
        url: Some("data:application/pdf;base64,this is not base64".into()),
        path: None,
        name: Some("bad.pdf".into()),
    };

    let err = resolve_outgoing_media(&part, &http_client())
        .await
        .expect_err("invalid base64 data URL should fail");

    assert!(
        err.to_string().contains("base64"),
        "invalid data URL error should mention base64: {err}"
    );
}

#[tokio::test]
async fn data_url_rejects_empty_payload_before_channel_send() {
    let part = OutgoingContentPart::File {
        url: Some("data:application/octet-stream;base64,".into()),
        path: None,
        name: Some("empty.bin".into()),
    };

    let err = resolve_outgoing_media(&part, &http_client())
        .await
        .expect_err("empty data URL should fail");

    assert!(
        err.to_string().contains("empty"),
        "empty data URL error should be explicit: {err}"
    );
}

#[tokio::test]
async fn data_url_rejects_non_base64_data_url_before_channel_send() {
    let part = OutgoingContentPart::File {
        url: Some("data:text/plain,hello".into()),
        path: None,
        name: Some("hello.txt".into()),
    };

    let err = resolve_outgoing_media(&part, &http_client())
        .await
        .expect_err("non-base64 data URL should fail");

    assert!(
        err.to_string().contains(";base64,"),
        "non-base64 data URL error should explain the required form: {err}"
    );
}

#[tokio::test]
async fn reject_dotdot_path_traversal() {
    let part = OutgoingContentPart::File {
        url: None,
        path: Some(PathBuf::from("../../../etc/passwd")),
        name: Some("passwd".into()),
    };

    let result = resolve_outgoing_media(&part, &http_client()).await;
    assert!(result.is_err(), "should reject path with ..");
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("traversal"),
        "error should mention traversal: {err}"
    );
}

#[tokio::test]
async fn reject_absolute_path() {
    let part = OutgoingContentPart::File {
        url: None,
        path: Some(PathBuf::from("/etc/passwd")),
        name: Some("passwd".into()),
    };

    let result = resolve_outgoing_media(&part, &http_client()).await;
    assert!(result.is_err(), "should reject absolute path");
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("absolute"),
        "error should mention absolute: {err}"
    );
}

#[tokio::test]
async fn local_relative_path_must_exist_and_be_a_file() {
    let unique = uuid::Uuid::new_v4().as_simple().to_string();
    let dir = std::env::current_dir()
        .unwrap()
        .join(format!(".tmp-core-media-{unique}"));
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(dir.clone());

    std::fs::create_dir_all(&dir).unwrap();
    let file_path = dir.join("local.txt");
    std::fs::write(&file_path, b"local bytes").unwrap();
    let relative_file = file_path
        .strip_prefix(std::env::current_dir().unwrap())
        .unwrap()
        .to_path_buf();

    let part = OutgoingContentPart::File {
        url: None,
        path: Some(relative_file.clone()),
        name: Some("ignored-name.txt".into()),
    };

    let resolved = resolve_outgoing_media(&part, &http_client())
        .await
        .expect("existing local file should resolve")
        .expect("resolved media");
    assert_eq!(resolved.path, relative_file);
    assert_eq!(resolved.name, "local.txt");

    let missing = OutgoingContentPart::File {
        url: None,
        path: Some(
            dir.join("missing.txt")
                .strip_prefix(std::env::current_dir().unwrap())
                .unwrap()
                .to_path_buf(),
        ),
        name: Some("missing.txt".into()),
    };
    let err = resolve_outgoing_media(&missing, &http_client())
        .await
        .expect_err("missing local file should fail before channel send");
    assert!(
        err.to_string().contains("not found"),
        "missing-file error should be explicit: {err}"
    );

    let directory = OutgoingContentPart::File {
        url: None,
        path: Some(
            dir.strip_prefix(std::env::current_dir().unwrap())
                .unwrap()
                .to_path_buf(),
        ),
        name: Some("dir".into()),
    };
    let err = resolve_outgoing_media(&directory, &http_client())
        .await
        .expect_err("directory should not resolve as a media file");
    assert!(
        err.to_string().contains("not a file"),
        "directory error should be explicit: {err}"
    );
}

#[tokio::test]
async fn local_relative_path_rejects_empty_files_before_channel_send() {
    let unique = uuid::Uuid::new_v4().as_simple().to_string();
    let dir = std::env::current_dir()
        .unwrap()
        .join(format!(".tmp-core-media-empty-{unique}"));
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(dir.clone());

    std::fs::create_dir_all(&dir).unwrap();
    let empty_path = dir.join("empty.txt");
    std::fs::write(&empty_path, b"").unwrap();
    let relative_empty = empty_path
        .strip_prefix(std::env::current_dir().unwrap())
        .unwrap()
        .to_path_buf();

    let part = OutgoingContentPart::File {
        url: None,
        path: Some(relative_empty),
        name: Some("empty.txt".into()),
    };

    let err = resolve_outgoing_media(&part, &http_client())
        .await
        .expect_err("empty local media file should fail before channel send");
    assert!(
        err.to_string().contains("empty"),
        "empty-file error should be explicit: {err}"
    );
}

#[tokio::test]
async fn local_relative_path_rejects_too_large_files_before_channel_send() {
    let unique = uuid::Uuid::new_v4().as_simple().to_string();
    let dir = std::env::current_dir()
        .unwrap()
        .join(format!(".tmp-core-media-large-{unique}"));
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(dir.clone());

    std::fs::create_dir_all(&dir).unwrap();
    let large_path = dir.join("large.bin");
    let file = std::fs::File::create(&large_path).unwrap();
    file.set_len(50 * 1024 * 1024 + 1).unwrap();
    let relative_large = large_path
        .strip_prefix(std::env::current_dir().unwrap())
        .unwrap()
        .to_path_buf();

    let part = OutgoingContentPart::File {
        url: None,
        path: Some(relative_large),
        name: Some("large.bin".into()),
    };

    let err = resolve_outgoing_media(&part, &http_client())
        .await
        .expect_err("too-large local media file should fail before channel send");
    assert!(
        err.to_string().contains("too large"),
        "large-file error should be explicit: {err}"
    );
}

#[tokio::test]
async fn file_url_resolves_existing_local_file_and_decodes_filename() {
    let unique = uuid::Uuid::new_v4().as_simple().to_string();
    let dir = std::env::current_dir()
        .unwrap()
        .join(format!(".tmp-core-media-file-url-{unique}"));
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(dir.clone());

    std::fs::create_dir_all(&dir).unwrap();
    let file_path = dir.join("report 1.pdf");
    std::fs::write(&file_path, b"pdf bytes").unwrap();
    let file_url = format!("file://{}", file_path.display()).replace(' ', "%20");

    let part = OutgoingContentPart::File {
        url: Some(file_url),
        path: None,
        name: Some("fallback.bin".into()),
    };

    let resolved = resolve_outgoing_media(&part, &http_client())
        .await
        .expect("file URL should resolve")
        .expect("resolved media");

    assert_eq!(resolved.path, file_path);
    assert_eq!(resolved.name, "report 1.pdf");
}

#[tokio::test]
async fn file_url_accepts_localhost_authority_for_local_files() {
    let unique = uuid::Uuid::new_v4().as_simple().to_string();
    let dir = std::env::current_dir()
        .unwrap()
        .join(format!(".tmp-core-media-file-url-localhost-{unique}"));
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(dir.clone());

    std::fs::create_dir_all(&dir).unwrap();
    let file_path = dir.join("localhost-file.txt");
    std::fs::write(&file_path, b"localhost file bytes").unwrap();
    let file_url = format!("file://localhost{}", file_path.display());

    let part = OutgoingContentPart::File {
        url: Some(file_url),
        path: None,
        name: Some("fallback.bin".into()),
    };

    let resolved = resolve_outgoing_media(&part, &http_client())
        .await
        .expect("localhost file URL should resolve")
        .expect("resolved media");

    assert_eq!(resolved.path, file_path);
    assert_eq!(resolved.name, "localhost-file.txt");
}

#[tokio::test]
async fn file_url_rejects_missing_empty_and_traversal_paths() {
    let unique = uuid::Uuid::new_v4().as_simple().to_string();
    let dir = std::env::current_dir()
        .unwrap()
        .join(format!(".tmp-core-media-file-url-invalid-{unique}"));
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(dir.clone());

    std::fs::create_dir_all(&dir).unwrap();
    let empty = dir.join("empty.txt");
    std::fs::write(&empty, b"").unwrap();

    let empty_part = OutgoingContentPart::File {
        url: Some(format!("file://{}", empty.display())),
        path: None,
        name: Some("empty.txt".into()),
    };
    let err = resolve_outgoing_media(&empty_part, &http_client())
        .await
        .expect_err("empty file URL should be rejected");
    assert!(
        err.to_string().contains("empty"),
        "empty-file error should be explicit: {err}"
    );

    let missing_part = OutgoingContentPart::File {
        url: Some(format!("file://{}/missing.txt", dir.display())),
        path: None,
        name: Some("missing.txt".into()),
    };
    let err = resolve_outgoing_media(&missing_part, &http_client())
        .await
        .expect_err("missing file URL should be rejected");
    assert!(
        err.to_string().contains("not found"),
        "missing-file error should be explicit: {err}"
    );

    let traversal_part = OutgoingContentPart::File {
        url: Some("file:///tmp/../etc/passwd".into()),
        path: None,
        name: Some("passwd".into()),
    };
    let err = resolve_outgoing_media(&traversal_part, &http_client())
        .await
        .expect_err("file URL traversal should be rejected");
    assert!(
        err.to_string().contains("traversal"),
        "traversal error should be explicit: {err}"
    );

    let remote_authority = OutgoingContentPart::File {
        url: Some("file://example.com/tmp/remote.txt".into()),
        path: None,
        name: Some("remote.txt".into()),
    };
    let err = resolve_outgoing_media(&remote_authority, &http_client())
        .await
        .expect_err("non-local file URL authority should be rejected");
    assert!(
        err.to_string().contains("unsupported file URL"),
        "remote file URL error should be explicit: {err}"
    );
}

#[tokio::test]
async fn text_part_returns_none() {
    let part = OutgoingContentPart::Text {
        text: "hello".into(),
    };
    let result = resolve_outgoing_media(&part, &http_client())
        .await
        .expect("should succeed");
    assert!(result.is_none());
}

#[tokio::test]
async fn markdown_part_returns_none() {
    let part = OutgoingContentPart::Markdown {
        text: "# hello".into(),
    };
    let result = resolve_outgoing_media(&part, &http_client())
        .await
        .expect("should succeed");
    assert!(result.is_none());
}

#[tokio::test]
async fn image_with_no_source_returns_none() {
    let part = OutgoingContentPart::Image {
        url: None,
        path: None,
        alt: None,
    };
    let result = resolve_outgoing_media(&part, &http_client())
        .await
        .expect("should succeed");
    assert!(result.is_none());
}

#[tokio::test]
async fn download_http_url_to_temp_file_uses_url_filename_and_writes_body() {
    let base_url = spawn_http_response(200, b"downloaded image bytes").await;
    let part = OutgoingContentPart::Image {
        url: Some(format!("{base_url}/assets/photo.png?token=secret")),
        path: None,
        alt: Some("fallback.jpg".into()),
    };

    let resolved = resolve_outgoing_media(&part, &http_client())
        .await
        .expect("download should succeed")
        .expect("downloaded media");

    assert_eq!(resolved.name, "photo.png");
    assert_eq!(
        std::fs::read(&resolved.path).expect("downloaded file"),
        b"downloaded image bytes"
    );

    std::fs::remove_file(&resolved.path).ok();
}

#[tokio::test]
async fn download_http_url_with_empty_tail_uses_sanitized_fallback_name() {
    let base_url = spawn_http_response(200, b"file bytes").await;
    let part = OutgoingContentPart::File {
        url: Some(format!("{base_url}/download/")),
        path: None,
        name: Some("../../report.pdf".into()),
    };

    let resolved = resolve_outgoing_media(&part, &http_client())
        .await
        .expect("download should succeed")
        .expect("downloaded media");

    assert_eq!(resolved.name, "report.pdf");
    assert_eq!(
        std::fs::read(&resolved.path).expect("downloaded file"),
        b"file bytes"
    );

    std::fs::remove_file(&resolved.path).ok();
}

#[tokio::test]
async fn download_http_file_suffix_uses_content_type_when_available() {
    let base_url =
        spawn_http_response_with_headers(200, &[("content-type", "application/pdf")], b"%PDF-1.7")
            .await;
    let part = OutgoingContentPart::File {
        url: Some(format!("{base_url}/media/download.file")),
        path: None,
        name: Some("fallback.bin".into()),
    };

    let resolved = resolve_outgoing_media(&part, &http_client())
        .await
        .expect("download should succeed")
        .expect("downloaded media");

    assert_eq!(resolved.name, "download.pdf");
    assert_eq!(
        std::fs::read(&resolved.path).expect("downloaded file"),
        b"%PDF-1.7"
    );

    std::fs::remove_file(&resolved.path).ok();
}

#[tokio::test]
async fn download_http_uses_content_disposition_filename_when_available() {
    let base_url = spawn_http_response_with_headers(
        200,
        &[(
            "content-disposition",
            "attachment; filename=\"document.pdf\"",
        )],
        b"downloaded file bytes",
    )
    .await;
    let part = OutgoingContentPart::File {
        url: Some(format!("{base_url}/download")),
        path: None,
        name: Some("fallback.bin".into()),
    };

    let resolved = resolve_outgoing_media(&part, &http_client())
        .await
        .expect("download should succeed")
        .expect("downloaded media");

    assert_eq!(resolved.name, "document.pdf");
    assert_eq!(
        std::fs::read(&resolved.path).expect("downloaded file"),
        b"downloaded file bytes"
    );

    std::fs::remove_file(&resolved.path).ok();
}

#[tokio::test]
async fn download_http_prefers_encoded_content_disposition_filename() {
    let base_url = spawn_http_response_with_headers(
        200,
        &[(
            "content-disposition",
            "attachment; filename=\"fallback.pdf\"; filename*=UTF-8''report%20final.pdf",
        )],
        b"downloaded file bytes",
    )
    .await;
    let part = OutgoingContentPart::File {
        url: Some(format!("{base_url}/download")),
        path: None,
        name: Some("fallback.bin".into()),
    };

    let resolved = resolve_outgoing_media(&part, &http_client())
        .await
        .expect("download should succeed")
        .expect("downloaded media");

    assert_eq!(resolved.name, "report final.pdf");

    std::fs::remove_file(&resolved.path).ok();
}

#[tokio::test]
async fn download_http_sanitizes_content_disposition_filename() {
    let base_url = spawn_http_response_with_headers(
        200,
        &[(
            "content-disposition",
            "attachment; filename=\"../../secret.pdf\"",
        )],
        b"downloaded file bytes",
    )
    .await;
    let part = OutgoingContentPart::File {
        url: Some(format!("{base_url}/download")),
        path: None,
        name: Some("fallback.bin".into()),
    };

    let resolved = resolve_outgoing_media(&part, &http_client())
        .await
        .expect("download should succeed")
        .expect("downloaded media");

    assert_eq!(resolved.name, "secret.pdf");

    std::fs::remove_file(&resolved.path).ok();
}

#[tokio::test]
async fn download_http_file_suffix_falls_back_to_magic_bytes() {
    let base_url = spawn_http_response(200, b"\x89PNG\r\n\x1a\nrest").await;
    let part = OutgoingContentPart::Image {
        url: Some(format!("{base_url}/oss/object.file")),
        path: None,
        alt: Some("fallback.jpg".into()),
    };

    let resolved = resolve_outgoing_media(&part, &http_client())
        .await
        .expect("download should succeed")
        .expect("downloaded media");

    assert_eq!(resolved.name, "object.png");
    assert_eq!(
        std::fs::read(&resolved.path).expect("downloaded file"),
        b"\x89PNG\r\n\x1a\nrest"
    );

    std::fs::remove_file(&resolved.path).ok();
}

#[tokio::test]
async fn download_http_empty_body_is_rejected_before_channel_send() {
    let base_url = spawn_http_response(200, b"").await;
    let part = OutgoingContentPart::File {
        url: Some(format!("{base_url}/empty.file")),
        path: None,
        name: Some("empty.file".into()),
    };

    let err = resolve_outgoing_media(&part, &http_client())
        .await
        .expect_err("empty media download should fail");

    assert!(
        err.to_string().contains("empty"),
        "empty download error should be explicit: {err}"
    );
}

#[tokio::test]
async fn download_http_non_success_status_returns_io_error() {
    let base_url = spawn_http_response(404, b"missing").await;
    let part = OutgoingContentPart::File {
        url: Some(format!("{base_url}/missing.pdf")),
        path: None,
        name: Some("missing.pdf".into()),
    };

    let err = resolve_outgoing_media(&part, &http_client())
        .await
        .expect_err("non-2xx media download should fail");

    assert!(
        err.to_string().contains("404"),
        "error should include HTTP status: {err}"
    );
}

#[tokio::test]
async fn download_http_content_length_too_large_is_rejected_before_body() {
    let base_url = spawn_http_raw_response(
        "HTTP/1.1 200 OK\r\ncontent-length: 52428801\r\nconnection: close\r\n\r\n",
    )
    .await;
    let part = OutgoingContentPart::File {
        url: Some(format!("{base_url}/large.bin")),
        path: None,
        name: Some("large.bin".into()),
    };

    let err = resolve_outgoing_media(&part, &http_client())
        .await
        .expect_err("too-large content-length should fail before channel send");

    assert!(
        err.to_string().contains("too large"),
        "large content-length error should be explicit: {err}"
    );
}
