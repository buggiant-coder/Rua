use rua_local_model::{
    cuda::{CudaInfo, DetectedArch, DetectedOs},
    download_ipc::{DownloadStatus, IpcMessage},
    ServerBinaryDownloader,
};
use std::io::Write;
use tempfile::TempDir;
use tokio::net::TcpListener;

fn no_cuda() -> CudaInfo {
    CudaInfo::default()
}

fn cuda_12_6() -> CudaInfo {
    CudaInfo {
        available: true,
        version: Some("12.6".into()),
        vram_mb: Some(8192),
    }
}

fn cuda_11() -> CudaInfo {
    CudaInfo {
        available: true,
        version: Some("11.8".into()),
        vram_mb: Some(4096),
    }
}

#[test]
fn asset_filename_macos_arm64() {
    let d =
        ServerBinaryDownloader::new(DetectedOs::MacOs, DetectedArch::Aarch64, no_cuda(), "/tmp");
    assert_eq!(
        d.build_asset_filename("b5117").unwrap(),
        "llama-b5117-bin-macos-arm64.tar.gz"
    );
}

#[test]
fn asset_filename_macos_x64() {
    let d = ServerBinaryDownloader::new(DetectedOs::MacOs, DetectedArch::X86_64, no_cuda(), "/tmp");
    assert_eq!(
        d.build_asset_filename("b5117").unwrap(),
        "llama-b5117-bin-macos-x64.tar.gz"
    );
}

#[test]
fn asset_filename_linux_x64() {
    let d = ServerBinaryDownloader::new(DetectedOs::Linux, DetectedArch::X86_64, no_cuda(), "/tmp");
    assert_eq!(
        d.build_asset_filename("b5117").unwrap(),
        "llama-b5117-bin-ubuntu-x64.tar.gz"
    );
}

#[test]
fn asset_filename_linux_arm64() {
    let d =
        ServerBinaryDownloader::new(DetectedOs::Linux, DetectedArch::Aarch64, no_cuda(), "/tmp");
    assert_eq!(
        d.build_asset_filename("b5117").unwrap(),
        "llama-b5117-bin-ubuntu-arm64.tar.gz"
    );
}

#[test]
fn asset_filename_linux_ignores_cuda() {
    let d =
        ServerBinaryDownloader::new(DetectedOs::Linux, DetectedArch::X86_64, cuda_12_6(), "/tmp");
    assert_eq!(
        d.build_asset_filename("b5117").unwrap(),
        "llama-b5117-bin-ubuntu-x64.tar.gz"
    );
}

#[test]
fn asset_filename_windows_cpu_x64() {
    let d =
        ServerBinaryDownloader::new(DetectedOs::Windows, DetectedArch::X86_64, no_cuda(), "/tmp");
    assert_eq!(
        d.build_asset_filename("b5117").unwrap(),
        "llama-b5117-bin-win-cpu-x64.zip"
    );
}

#[test]
fn asset_filename_windows_cuda12_x64() {
    let d = ServerBinaryDownloader::new(
        DetectedOs::Windows,
        DetectedArch::X86_64,
        cuda_12_6(),
        "/tmp",
    );
    assert_eq!(
        d.build_asset_filename("b5117").unwrap(),
        "llama-b5117-bin-win-cuda-12.4-x64.zip"
    );
}

#[test]
fn asset_filename_windows_cuda13_x64() {
    let cuda13 = CudaInfo {
        available: true,
        version: Some("13.0".into()),
        vram_mb: Some(16384),
    };
    let d = ServerBinaryDownloader::new(DetectedOs::Windows, DetectedArch::X86_64, cuda13, "/tmp");
    assert_eq!(
        d.build_asset_filename("b5117").unwrap(),
        "llama-b5117-bin-win-cuda-13.1-x64.zip"
    );
}

#[test]
fn asset_filename_rejects_windows_cuda_arm64() {
    let d = ServerBinaryDownloader::new(
        DetectedOs::Windows,
        DetectedArch::Aarch64,
        cuda_12_6(),
        "/tmp",
    );
    assert!(d.build_asset_filename("b5117").is_err());
}

#[test]
fn asset_filename_windows_cuda11_falls_back_to_cpu() {
    let d =
        ServerBinaryDownloader::new(DetectedOs::Windows, DetectedArch::X86_64, cuda_11(), "/tmp");
    assert_eq!(
        d.build_asset_filename("b5117").unwrap(),
        "llama-b5117-bin-win-cpu-x64.zip"
    );
}

#[test]
fn asset_filename_rejects_unknown_os() {
    let d =
        ServerBinaryDownloader::new(DetectedOs::Unknown, DetectedArch::X86_64, no_cuda(), "/tmp");
    assert!(d.build_asset_filename("b5117").is_err());
}

#[tokio::test]
async fn downloads_and_extracts_mock_release_archive() {
    let tmp = TempDir::new().unwrap();

    let archive_src = tmp.path().join("archive-src");
    let top_level = archive_src.join("llama-b9999-bin-macos-arm64");
    std::fs::create_dir_all(&top_level).unwrap();

    let fake_binary = top_level.join("llama-server");
    {
        let mut f = std::fs::File::create(&fake_binary).unwrap();
        f.write_all(b"#!/bin/sh\necho ok\n").unwrap();
    }

    let archive_name = "llama-b9999-bin-macos-arm64.tar.gz";
    let archive_path = tmp.path().join(archive_name);
    let status = std::process::Command::new("tar")
        .arg("czf")
        .arg(&archive_path)
        .arg("-C")
        .arg(&archive_src)
        .arg("llama-b9999-bin-macos-arm64")
        .status()
        .unwrap();
    assert!(status.success(), "tar creation failed");

    let archive_bytes = std::fs::read(&archive_path).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => break,
            };
            let bytes = archive_bytes.clone();
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = vec![0u8; 4096];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);

                if req.contains("GET /api/latest") {
                    let body = r#"{"tag_name":"b9999"}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\n\
                         Content-Type: application/json\r\n\
                         Content-Length: {}\r\n\
                         Connection: close\r\n\r\n{}",
                        body.len(),
                        body,
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                } else if req.contains("GET /download/") {
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\n\
                         Content-Type: application/octet-stream\r\n\
                         Content-Length: {}\r\n\
                         Connection: close\r\n\r\n",
                        bytes.len(),
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                    let _ = stream.write_all(&bytes).await;
                } else {
                    let _ = stream
                        .write_all(b"HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n")
                        .await;
                }
                let _ = stream.shutdown().await;
            });
        }
    });

    let target_dir = tmp.path().join("models");
    std::fs::create_dir_all(&target_dir).unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::channel::<IpcMessage>(64);
    let downloader = ServerBinaryDownloader::new(
        DetectedOs::MacOs,
        DetectedArch::Aarch64,
        no_cuda(),
        &target_dir,
    )
    .with_api_url(format!("http://127.0.0.1:{port}/api/latest"))
    .with_release_base_url(format!("http://127.0.0.1:{port}/download"));

    let binary_path = downloader.download(Some(tx), None).await.unwrap();
    assert!(binary_path.exists(), "binary should exist");
    assert!(
        binary_path.ends_with("server/llama-server"),
        "unexpected path: {}",
        binary_path.display()
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&binary_path)
            .unwrap()
            .permissions()
            .mode();
        assert!(mode & 0o111 != 0, "binary should be executable");
    }

    let mut messages = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        messages.push(msg);
    }
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, IpcMessage::Status(s) if s.status == DownloadStatus::Started)),
        "should have received Started status"
    );
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, IpcMessage::Progress(_))),
        "should have received progress updates"
    );
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, IpcMessage::Status(s) if s.status == DownloadStatus::Completed)),
        "should have received Completed status"
    );
}

#[tokio::test]
async fn server_binary_404_download_is_terminal_and_cleans_staging_dir() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => break,
            };
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = vec![0u8; 4096];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);

                if req.contains("GET /api/latest") {
                    let body = r#"{"tag_name":"b404"}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\n\
                         Content-Type: application/json\r\n\
                         Content-Length: {}\r\n\
                         Connection: close\r\n\r\n{}",
                        body.len(),
                        body,
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                } else {
                    let _ = stream
                        .write_all(b"HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n")
                        .await;
                }
                let _ = stream.shutdown().await;
            });
        }
    });

    let tmp = TempDir::new().unwrap();
    let target_dir = tmp.path().join("models");
    std::fs::create_dir_all(&target_dir).unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::channel::<IpcMessage>(64);
    let downloader = ServerBinaryDownloader::new(
        DetectedOs::MacOs,
        DetectedArch::Aarch64,
        no_cuda(),
        &target_dir,
    )
    .with_api_url(format!("http://127.0.0.1:{port}/api/latest"))
    .with_release_base_url(format!("http://127.0.0.1:{port}/download"));

    let err = downloader
        .download(Some(tx), None)
        .await
        .expect_err("missing llama.cpp package should fail");
    assert!(err.to_string().contains("HTTP 404"));

    let mut messages = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        messages.push(msg);
    }
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, IpcMessage::Status(s) if s.status == DownloadStatus::Started)),
        "server-binary download should emit Started before terminal package errors"
    );
    let error = messages
        .iter()
        .find_map(|msg| match msg {
            IpcMessage::Error(err) => Some(err),
            _ => None,
        })
        .expect("server-binary download should emit an Error message");
    assert!(error.error.contains("HTTP 404"));
    assert_eq!(
        error.retryable,
        Some(false),
        "package not found is terminal, not a retryable network failure"
    );

    let leftovers = std::fs::read_dir(&target_dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert!(
        leftovers
            .iter()
            .all(|name| !name.starts_with(".server-staging-")),
        "failed package download should remove staging dirs, left: {leftovers:?}"
    );
}

#[tokio::test]
async fn server_binary_bad_archive_preserves_existing_installation() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => break,
            };
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = vec![0u8; 4096];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);

                if req.contains("GET /api/latest") {
                    let body = r#"{"tag_name":"bbad"}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\n\
                         Content-Type: application/json\r\n\
                         Content-Length: {}\r\n\
                         Connection: close\r\n\r\n{}",
                        body.len(),
                        body,
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                } else if req.contains("GET /download/") {
                    let body = b"not a tarball";
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\n\
                         Content-Type: application/octet-stream\r\n\
                         Content-Length: {}\r\n\
                         Connection: close\r\n\r\n",
                        body.len(),
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                    let _ = stream.write_all(body).await;
                } else {
                    let _ = stream
                        .write_all(b"HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n")
                        .await;
                }
                let _ = stream.shutdown().await;
            });
        }
    });

    let tmp = TempDir::new().unwrap();
    let target_dir = tmp.path().join("models");
    let server_dir = target_dir.join("server");
    std::fs::create_dir_all(&server_dir).unwrap();
    let binary_path = server_dir.join("llama-server");
    std::fs::write(&binary_path, b"old-server").unwrap();
    std::fs::write(server_dir.join("version.txt"), "bold").unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::channel::<IpcMessage>(64);
    let downloader = ServerBinaryDownloader::new(
        DetectedOs::MacOs,
        DetectedArch::Aarch64,
        no_cuda(),
        &target_dir,
    )
    .with_api_url(format!("http://127.0.0.1:{port}/api/latest"))
    .with_release_base_url(format!("http://127.0.0.1:{port}/download"));

    let err = downloader
        .download(Some(tx), None)
        .await
        .expect_err("bad archive should fail before replacing installation");
    assert!(err.to_string().contains("extraction failed"));

    assert_eq!(
        std::fs::read(&binary_path).unwrap(),
        b"old-server",
        "failed extraction must not replace the existing server binary"
    );
    assert_eq!(
        std::fs::read_to_string(server_dir.join("version.txt")).unwrap(),
        "bold",
        "failed extraction must not advance the installed version marker"
    );

    let mut messages = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        messages.push(msg);
    }
    let error = messages
        .iter()
        .find_map(|msg| match msg {
            IpcMessage::Error(err) => Some(err),
            _ => None,
        })
        .expect("bad archive should emit an Error message");
    assert_eq!(
        error.retryable,
        Some(false),
        "archive extraction failures require a corrected package, not blind retry"
    );
}

#[tokio::test]
async fn downloads_and_extracts_mock_windows_zip_archive() {
    let tmp = TempDir::new().unwrap();

    let archive_src = tmp.path().join("zip-src");
    let top_level = archive_src.join("llama-b8888-bin-win-cpu-x64");
    std::fs::create_dir_all(&top_level).unwrap();

    let fake_binary = top_level.join("llama-server.exe");
    {
        let mut f = std::fs::File::create(&fake_binary).unwrap();
        f.write_all(b"@echo off\necho ok\n").unwrap();
    }

    let archive_name = "llama-b8888-bin-win-cpu-x64.zip";
    let archive_path = tmp.path().join(archive_name);
    let status = std::process::Command::new("zip")
        .arg("-qr")
        .arg(&archive_path)
        .arg("llama-b8888-bin-win-cpu-x64")
        .current_dir(&archive_src)
        .status()
        .unwrap();
    assert!(status.success(), "zip creation failed");

    let archive_bytes = std::fs::read(&archive_path).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => break,
            };
            let bytes = archive_bytes.clone();
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = vec![0u8; 4096];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);

                if req.contains("GET /api/latest") {
                    let body = r#"{"tag_name":"b8888"}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\n\
                         Content-Type: application/json\r\n\
                         Content-Length: {}\r\n\
                         Connection: close\r\n\r\n{}",
                        body.len(),
                        body,
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                } else if req.contains("GET /download/") {
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\n\
                         Content-Type: application/octet-stream\r\n\
                         Content-Length: {}\r\n\
                         Connection: close\r\n\r\n",
                        bytes.len(),
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                    let _ = stream.write_all(&bytes).await;
                } else {
                    let _ = stream
                        .write_all(b"HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n")
                        .await;
                }
                let _ = stream.shutdown().await;
            });
        }
    });

    let target_dir = tmp.path().join("models");
    std::fs::create_dir_all(&target_dir).unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::channel::<IpcMessage>(64);
    let downloader = ServerBinaryDownloader::new(
        DetectedOs::Windows,
        DetectedArch::X86_64,
        no_cuda(),
        &target_dir,
    )
    .with_api_url(format!("http://127.0.0.1:{port}/api/latest"))
    .with_release_base_url(format!("http://127.0.0.1:{port}/download"));

    let binary_path = downloader.download(Some(tx), None).await.unwrap();

    assert!(binary_path.exists(), "binary should exist");
    assert!(
        binary_path.ends_with("server/llama-server.exe"),
        "unexpected path: {}",
        binary_path.display()
    );
    assert_eq!(
        std::fs::read_to_string(target_dir.join("server/version.txt")).unwrap(),
        "b8888"
    );

    let mut messages = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        messages.push(msg);
    }
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, IpcMessage::Status(s) if s.status == DownloadStatus::Started)),
        "should have received Started status"
    );
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, IpcMessage::Status(s) if s.status == DownloadStatus::Completed)),
        "should have received Completed status"
    );
}
