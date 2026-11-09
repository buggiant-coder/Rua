use std::path::{Path, PathBuf};

use async_trait::async_trait;
use rua_core::Result;
use rua_local_model::model_source::{ModelSource, ProgressTx};
use rua_local_model::{CustomUrlSource, DownloadStatus, IpcMessage, ProgressUpdate, StatusUpdate};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

struct MockSource;

#[async_trait]
impl ModelSource for MockSource {
    fn name(&self) -> &str {
        "mock"
    }

    async fn fetch_to(&self, _dest_path: &Path, progress_tx: ProgressTx) -> Result<()> {
        progress_tx
            .send(IpcMessage::Status(StatusUpdate {
                id: "test".into(),
                status: DownloadStatus::Started,
                detail: None,
            }))
            .ok();
        progress_tx
            .send(IpcMessage::Progress(ProgressUpdate {
                id: "test".into(),
                bytes_downloaded: 100,
                bytes_total: 100,
                eta_secs: None,
            }))
            .ok();
        progress_tx
            .send(IpcMessage::Status(StatusUpdate {
                id: "test".into(),
                status: DownloadStatus::Completed,
                detail: None,
            }))
            .ok();
        Ok(())
    }
}

#[tokio::test]
async fn model_source_trait_object_dispatches_progress_messages() {
    let source: Box<dyn ModelSource> = Box::new(MockSource);
    assert_eq!(source.name(), "mock");

    let (tx, mut rx) = mpsc::unbounded_channel();
    let dest = PathBuf::from("/tmp/test-model.gguf");
    source.fetch_to(&dest, tx).await.unwrap();

    let mut msgs = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        msgs.push(msg);
    }
    assert_eq!(msgs.len(), 3);
    assert!(matches!(&msgs[0], IpcMessage::Status(s) if s.status == DownloadStatus::Started));
    assert!(matches!(&msgs[2], IpcMessage::Status(s) if s.status == DownloadStatus::Completed));
}

#[test]
fn custom_url_source_exposes_stable_source_metadata() {
    let src = CustomUrlSource::new("https://example.com/model.gguf", "custom-model");

    assert_eq!(src.name(), "custom_url");
    assert_eq!(src.url, "https://example.com/model.gguf");
    assert_eq!(src.model_id, "custom-model");
}

async fn start_status_server(status: u16) -> (u16, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move {
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(stream) => stream,
                Err(_) => break,
            };
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = [0; 1024];
                let _ = stream.read(&mut buf).await;
                let reason = if status == 404 { "Not Found" } else { "Error" };
                let response = format!("HTTP/1.1 {status} {reason}\r\nConnection: close\r\n\r\n");
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    (port, handle)
}

#[tokio::test]
async fn custom_url_source_reports_non_success_as_non_retryable_error() {
    let (port, _handle) = start_status_server(404).await;
    let source = CustomUrlSource::new(format!("http://127.0.0.1:{port}/missing.gguf"), "missing");
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("missing.gguf");
    let (tx, mut rx) = mpsc::unbounded_channel();

    let err = source
        .fetch_to(&dest, tx)
        .await
        .expect_err("404 downloads should fail");

    assert!(err.to_string().contains("404"));
    assert!(
        !dest.exists(),
        "failed custom URL downloads must not leave a completed model file"
    );

    let first = rx.recv().await.expect("started status");
    assert!(matches!(
        first,
        IpcMessage::Status(StatusUpdate {
            status: DownloadStatus::Started,
            ..
        })
    ));
    let second = rx.recv().await.expect("error update");
    match second {
        IpcMessage::Error(error) => {
            assert_eq!(error.id, "missing");
            assert!(error.error.contains("404"));
            assert_eq!(error.retryable, Some(false));
        }
        other => panic!("expected Error update, got {other:?}"),
    }
}
