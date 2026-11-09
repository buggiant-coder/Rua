use rua_local_model::robustness::{compute_sha256, resume_download};
use rua_local_model::IpcMessage;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

async fn start_range_server(payload: Vec<u8>) -> (u16, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let handle = tokio::spawn(async move {
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => break,
            };
            let bytes = payload.clone();
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = vec![0u8; 4096];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);

                let range_start = parse_range_header(&req);

                if let Some(start) = range_start {
                    if start < bytes.len() as u64 {
                        let slice = &bytes[start as usize..];
                        let resp = format!(
                            "HTTP/1.1 206 Partial Content\r\n\
                             Content-Length: {}\r\n\
                             Content-Range: bytes {}-{}/{}\r\n\
                             Connection: close\r\n\r\n",
                            slice.len(),
                            start,
                            bytes.len() - 1,
                            bytes.len(),
                        );
                        let _ = stream.write_all(resp.as_bytes()).await;
                        let _ = stream.write_all(slice).await;
                    } else {
                        let _ = stream
                            .write_all(
                                b"HTTP/1.1 416 Range Not Satisfiable\r\n\
                                  Connection: close\r\n\r\n",
                            )
                            .await;
                    }
                } else {
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\n\
                         Content-Length: {}\r\n\
                         Accept-Ranges: bytes\r\n\
                         Connection: close\r\n\r\n",
                        bytes.len(),
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                    let _ = stream.write_all(&bytes).await;
                }
                let _ = stream.shutdown().await;
            });
        }
    });

    (port, handle)
}

async fn start_no_range_server(payload: Vec<u8>) -> (u16, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let handle = tokio::spawn(async move {
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => break,
            };
            let bytes = payload.clone();
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = vec![0u8; 4096];
                let _ = stream.read(&mut buf).await;

                let resp = format!(
                    "HTTP/1.1 200 OK\r\n\
                     Content-Length: {}\r\n\
                     Connection: close\r\n\r\n",
                    bytes.len(),
                );
                let _ = stream.write_all(resp.as_bytes()).await;
                let _ = stream.write_all(&bytes).await;
                let _ = stream.shutdown().await;
            });
        }
    });

    (port, handle)
}

fn parse_range_header(req: &str) -> Option<u64> {
    for line in req.lines() {
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("range: bytes=") {
            let val = line.split('=').nth(1)?;
            let start_str = val.split('-').next()?;
            return start_str.parse().ok();
        }
    }
    None
}

#[tokio::test]
async fn sha256_match_succeeds() {
    let tmp = TempDir::new().unwrap();
    let payload = b"hello world test payload for sha256";
    let expected = {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("tmp");
        std::fs::write(&p, payload).unwrap();
        compute_sha256(&p).unwrap()
    };

    let (port, _handle) = start_range_server(payload.to_vec()).await;
    let dest = tmp.path().join("download.bin");
    let client = reqwest::Client::new();

    let result = resume_download(
        &client,
        &format!("http://127.0.0.1:{port}/file"),
        &dest,
        Some(&expected),
        "test",
        &None,
    )
    .await;

    assert!(result.is_ok(), "sha256 match should succeed: {result:?}");
    assert!(dest.exists());
    assert_eq!(std::fs::read(&dest).unwrap(), payload);
}

#[tokio::test]
async fn sha256_mismatch_deletes_file() {
    let tmp = TempDir::new().unwrap();
    let payload = b"some data";

    let (port, _handle) = start_range_server(payload.to_vec()).await;
    let dest = tmp.path().join("download.bin");
    let client = reqwest::Client::new();

    let result = resume_download(
        &client,
        &format!("http://127.0.0.1:{port}/file"),
        &dest,
        Some("0000000000000000000000000000000000000000000000000000000000000000"),
        "test",
        &None,
    )
    .await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("SHA256 mismatch"), "error: {err}");
    assert!(!dest.exists(), "file should be deleted on mismatch");
}

#[tokio::test]
async fn range_resume_appends_from_offset() {
    let tmp = TempDir::new().unwrap();
    let full_payload = b"AAAAABBBBB";
    let partial = &full_payload[..5];

    let (port, _handle) = start_range_server(full_payload.to_vec()).await;
    let dest = tmp.path().join("download.bin");

    std::fs::write(&dest, partial).unwrap();
    assert_eq!(std::fs::metadata(&dest).unwrap().len(), 5);

    let client = reqwest::Client::new();
    let (tx, mut rx) = mpsc::channel::<IpcMessage>(64);

    let result = resume_download(
        &client,
        &format!("http://127.0.0.1:{port}/file"),
        &dest,
        None,
        "test",
        &Some(tx),
    )
    .await;

    assert!(result.is_ok(), "resume should succeed: {result:?}");
    let content = std::fs::read(&dest).unwrap();
    assert_eq!(
        content, full_payload,
        "file should contain full payload after resume"
    );

    let mut msgs = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        msgs.push(msg);
    }
    assert!(!msgs.is_empty(), "should have progress updates");
}

#[tokio::test]
async fn range_not_supported_falls_back_to_full_download() {
    let tmp = TempDir::new().unwrap();
    let full_payload = b"FULL_CONTENT_HERE";
    let partial = b"OLD_P";

    let (port, _handle) = start_no_range_server(full_payload.to_vec()).await;
    let dest = tmp.path().join("download.bin");

    std::fs::write(&dest, partial).unwrap();

    let client = reqwest::Client::new();

    let result = resume_download(
        &client,
        &format!("http://127.0.0.1:{port}/file"),
        &dest,
        None,
        "test",
        &None,
    )
    .await;

    assert!(
        result.is_ok(),
        "fallback full download should succeed: {result:?}"
    );
    let content = std::fs::read(&dest).unwrap();
    assert_eq!(
        content, full_payload,
        "file should contain full payload (not appended)"
    );
}

#[tokio::test]
async fn fresh_download_no_sha256() {
    let tmp = TempDir::new().unwrap();
    let payload = b"brand new file content";

    let (port, _handle) = start_range_server(payload.to_vec()).await;
    let dest = tmp.path().join("download.bin");
    let client = reqwest::Client::new();

    let result = resume_download(
        &client,
        &format!("http://127.0.0.1:{port}/file"),
        &dest,
        None,
        "test",
        &None,
    )
    .await;

    assert!(result.is_ok());
    assert_eq!(std::fs::read(&dest).unwrap(), payload);
}
