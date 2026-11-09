use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use rua_core::Result;
use rua_local_model::manager::LocalModelManager;
use rua_local_model::model_source::{ModelSource, ProgressTx};
use rua_local_model::{DownloadProgress, DownloadStatus, IpcMessage, ProgressUpdate, StatusUpdate};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::Notify;

async fn start_mock_github_server(archive_bytes: Vec<u8>) -> (u16, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let handle = tokio::spawn(async move {
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
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body,
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                } else if req.contains("GET /download/") {
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n",
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

    (port, handle)
}

fn build_fake_server_archive(tmp: &TempDir) -> Vec<u8> {
    let archive_src = tmp.path().join("archive-src");
    let top_level = archive_src.join("llama-b9999-bin-macos-arm64");
    std::fs::create_dir_all(&top_level).unwrap();

    let fake_binary = top_level.join("llama-server");
    {
        let mut f = std::fs::File::create(&fake_binary).unwrap();
        f.write_all(b"#!/bin/sh\necho ok\n").unwrap();
    }

    let archive_path = tmp.path().join("llama-b9999-bin-macos-arm64.tar.gz");
    let status = std::process::Command::new("tar")
        .arg("czf")
        .arg(&archive_path)
        .arg("-C")
        .arg(&archive_src)
        .arg("llama-b9999-bin-macos-arm64")
        .status()
        .unwrap();
    assert!(status.success(), "tar creation failed");

    std::fs::read(&archive_path).unwrap()
}

async fn start_mock_model_server(model_bytes: Vec<u8>) -> (u16, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let handle = tokio::spawn(async move {
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => break,
            };
            let bytes = model_bytes.clone();
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = vec![0u8; 4096];
                let _ = stream.read(&mut buf).await;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n",
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

async fn start_hanging_http_server() -> (u16, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let handle = tokio::spawn(async move {
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => break,
            };
            tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                let mut buf = vec![0u8; 4096];
                let _ = stream.read(&mut buf).await;
                std::future::pending::<()>().await;
            });
        }
    });

    (port, handle)
}

async fn wait_for_server_snapshot(
    mgr: &LocalModelManager,
    predicate: impl Fn(&DownloadProgress) -> bool,
) -> DownloadProgress {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let snapshot = mgr.server_download_progress().await.unwrap();
        if predicate(&snapshot) {
            return snapshot;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "server download snapshot did not satisfy predicate: {snapshot:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

struct MockHfSource {
    api_url: String,
    download_url: String,
    model_id: String,
}

#[async_trait]
impl ModelSource for MockHfSource {
    fn name(&self) -> &str {
        "mock_huggingface"
    }

    async fn fetch_to(&self, dest_path: &Path, progress_tx: ProgressTx) -> Result<()> {
        let id = self.model_id.clone();
        let client = reqwest::Client::new();

        progress_tx
            .send(IpcMessage::Status(StatusUpdate {
                id: id.clone(),
                status: DownloadStatus::Started,
                detail: Some("resolving from mock HF API".into()),
            }))
            .ok();

        let resp = client
            .get(&self.api_url)
            .send()
            .await
            .map_err(|e| rua_core::RuaError::Io {
                message: format!("mock HF API failed: {e}"),
            })?;
        let _body = resp.text().await.unwrap_or_default();

        let resp =
            client
                .get(&self.download_url)
                .send()
                .await
                .map_err(|e| rua_core::RuaError::Io {
                    message: format!("mock HF download failed: {e}"),
                })?;

        let total = resp.content_length().unwrap_or(0);
        let bytes = resp.bytes().await.map_err(|e| rua_core::RuaError::Io {
            message: format!("mock HF stream error: {e}"),
        })?;

        tokio::fs::write(dest_path, &bytes)
            .await
            .map_err(|e| rua_core::RuaError::Io {
                message: format!("write failed: {e}"),
            })?;

        progress_tx
            .send(IpcMessage::Progress(ProgressUpdate {
                id: id.clone(),
                bytes_downloaded: bytes.len() as u64,
                bytes_total: total,
                eta_secs: None,
            }))
            .ok();

        progress_tx
            .send(IpcMessage::Status(StatusUpdate {
                id,
                status: DownloadStatus::Completed,
                detail: Some("mock-model.gguf".into()),
            }))
            .ok();

        Ok(())
    }
}

struct MockOllamaSource {
    manifest_url: String,
    blob_url: String,
    model_id: String,
}

#[async_trait]
impl ModelSource for MockOllamaSource {
    fn name(&self) -> &str {
        "mock_ollama"
    }

    async fn fetch_to(&self, dest_path: &Path, progress_tx: ProgressTx) -> Result<()> {
        let id = self.model_id.clone();
        let client = reqwest::Client::new();

        progress_tx
            .send(IpcMessage::Status(StatusUpdate {
                id: id.clone(),
                status: DownloadStatus::Started,
                detail: Some("fetching mock Ollama manifest".into()),
            }))
            .ok();

        let resp =
            client
                .get(&self.manifest_url)
                .send()
                .await
                .map_err(|e| rua_core::RuaError::Io {
                    message: format!("mock Ollama manifest failed: {e}"),
                })?;
        let _manifest = resp.text().await.unwrap_or_default();

        let resp = client
            .get(&self.blob_url)
            .send()
            .await
            .map_err(|e| rua_core::RuaError::Io {
                message: format!("mock Ollama blob failed: {e}"),
            })?;

        let total = resp.content_length().unwrap_or(0);
        let bytes = resp.bytes().await.map_err(|e| rua_core::RuaError::Io {
            message: format!("mock Ollama stream error: {e}"),
        })?;

        tokio::fs::write(dest_path, &bytes)
            .await
            .map_err(|e| rua_core::RuaError::Io {
                message: format!("write failed: {e}"),
            })?;

        progress_tx
            .send(IpcMessage::Progress(ProgressUpdate {
                id: id.clone(),
                bytes_downloaded: bytes.len() as u64,
                bytes_total: total,
                eta_secs: None,
            }))
            .ok();

        progress_tx
            .send(IpcMessage::Status(StatusUpdate {
                id,
                status: DownloadStatus::Completed,
                detail: Some("sha256:mock-digest".into()),
            }))
            .ok();

        Ok(())
    }
}

struct FailingSource {
    model_id: String,
    error_msg: String,
}

#[async_trait]
impl ModelSource for FailingSource {
    fn name(&self) -> &str {
        "failing"
    }

    async fn fetch_to(&self, _dest_path: &Path, progress_tx: ProgressTx) -> Result<()> {
        progress_tx
            .send(IpcMessage::Status(StatusUpdate {
                id: self.model_id.clone(),
                status: DownloadStatus::Started,
                detail: None,
            }))
            .ok();

        progress_tx
            .send(IpcMessage::Error(rua_local_model::ErrorUpdate {
                id: self.model_id.clone(),
                error: self.error_msg.clone(),
                retryable: Some(false),
            }))
            .ok();

        Err(rua_core::RuaError::Io {
            message: self.error_msg.clone(),
        })
    }
}

struct RecordingSource {
    bytes: Vec<u8>,
    observed_dest: Arc<Mutex<Option<PathBuf>>>,
}

#[async_trait]
impl ModelSource for RecordingSource {
    fn name(&self) -> &str {
        "recording"
    }

    async fn fetch_to(&self, dest_path: &Path, progress_tx: ProgressTx) -> Result<()> {
        *self.observed_dest.lock().unwrap() = Some(dest_path.to_path_buf());
        tokio::fs::write(dest_path, &self.bytes)
            .await
            .map_err(|e| rua_core::RuaError::Io {
                message: format!("write failed: {e}"),
            })?;

        progress_tx
            .send(IpcMessage::Status(StatusUpdate {
                id: "recording".into(),
                status: DownloadStatus::Completed,
                detail: None,
            }))
            .ok();

        Ok(())
    }
}

struct PartialThenFailSource {
    bytes: Vec<u8>,
    error_msg: String,
}

#[async_trait]
impl ModelSource for PartialThenFailSource {
    fn name(&self) -> &str {
        "partial_then_fail"
    }

    async fn fetch_to(&self, dest_path: &Path, progress_tx: ProgressTx) -> Result<()> {
        tokio::fs::write(dest_path, &self.bytes)
            .await
            .map_err(|e| rua_core::RuaError::Io {
                message: format!("partial write failed: {e}"),
            })?;

        progress_tx
            .send(IpcMessage::Error(rua_local_model::ErrorUpdate {
                id: "partial".into(),
                error: self.error_msg.clone(),
                retryable: Some(true),
            }))
            .ok();

        Err(rua_core::RuaError::Io {
            message: self.error_msg.clone(),
        })
    }
}

struct BlockingSource {
    started: Arc<Notify>,
}

#[async_trait]
impl ModelSource for BlockingSource {
    fn name(&self) -> &str {
        "blocking"
    }

    async fn fetch_to(&self, _dest_path: &Path, progress_tx: ProgressTx) -> Result<()> {
        progress_tx
            .send(IpcMessage::Status(StatusUpdate {
                id: "blocking".into(),
                status: DownloadStatus::Started,
                detail: None,
            }))
            .ok();
        self.started.notify_waiters();
        std::future::pending::<()>().await;
        #[allow(unreachable_code)]
        Ok(())
    }
}

#[tokio::test]
async fn download_server_happy_path_completed_and_file_on_disk() {
    let tmp = TempDir::new().unwrap();
    let archive_bytes = build_fake_server_archive(&tmp);

    let (port, _server_handle) = start_mock_github_server(archive_bytes).await;

    let working_dir = tmp.path().join("working");
    std::fs::create_dir_all(&working_dir).unwrap();

    let mgr = LocalModelManager::new_for_test(&working_dir)
        .with_server_api_url(format!("http://127.0.0.1:{port}/api/latest"))
        .with_server_release_base_url(format!("http://127.0.0.1:{port}/download"));

    let progress = mgr.download_server("test-server".into()).await;
    assert_eq!(progress.model_id, "test-server");
    assert_eq!(progress.status, "downloading");

    let mut rx = mgr.subscribe_download("test-server").await.unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut got_progress = false;
    let mut got_completed = false;
    loop {
        tokio::select! {
            msg = rx.recv() => {
                match msg {
                    Ok(IpcMessage::Progress(_)) => got_progress = true,
                    Ok(IpcMessage::Status(s)) if s.status == DownloadStatus::Completed => {
                        got_completed = true;
                        break;
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }

    assert!(got_progress, "should receive progress IPC via broadcast");
    assert!(
        got_completed,
        "should receive Completed status via broadcast"
    );

    let snapshot = wait_for_server_snapshot(&mgr, |snapshot| snapshot.local_path.is_some()).await;
    assert_eq!(snapshot.status, "completed");
    assert_eq!(snapshot.progress, Some(1.0));

    let server_dir = working_dir.join("server");
    let binary_path = server_dir.join("llama-server");
    assert!(
        binary_path.exists(),
        "llama-server binary should exist at {}",
        binary_path.display()
    );
    assert_eq!(
        snapshot.local_path.as_deref(),
        Some(binary_path.to_string_lossy().as_ref())
    );
    assert_eq!(snapshot.model_name.as_deref(), Some("test-server"));
    assert!(snapshot.downloaded_bytes > 0);
    assert_eq!(snapshot.total_bytes, Some(snapshot.downloaded_bytes));
    assert_eq!(snapshot.source.as_deref(), Some("auto"));
}

#[tokio::test]
async fn download_model_hf_source_happy_path() {
    let model_data = b"fake-hf-model-gguf-content";
    let (port, _handle) = start_mock_model_server(model_data.to_vec()).await;

    let tmp = TempDir::new().unwrap();
    let mgr = LocalModelManager::new_for_test(tmp.path());

    let source: Box<dyn ModelSource> = Box::new(MockHfSource {
        api_url: format!("http://127.0.0.1:{port}/api/models/org/repo/tree/main"),
        download_url: format!("http://127.0.0.1:{port}/org/repo/resolve/main/model.gguf"),
        model_id: "org/test-hf-model".into(),
    });

    let progress = mgr
        .download_model_from_source("org/test-hf-model".into(), source)
        .await;
    assert_eq!(progress.status, "downloading");

    tokio::time::sleep(Duration::from_secs(3)).await;

    let snapshot = mgr.model_download_progress().await.unwrap();
    assert_eq!(snapshot.status, "completed");
    assert_eq!(snapshot.progress, Some(1.0));
    assert_eq!(snapshot.model_name.as_deref(), Some("org/test-hf-model"));
    assert_eq!(snapshot.downloaded_bytes, model_data.len() as u64);
    assert_eq!(snapshot.total_bytes, Some(model_data.len() as u64));
    assert_eq!(snapshot.speed_bytes_per_sec, 0);
    assert_eq!(snapshot.source.as_deref(), Some("auto"));

    let dest = tmp.path().join("models").join("org_test-hf-model.gguf");
    assert!(dest.exists(), "HF model should be at {}", dest.display());
    assert_eq!(
        snapshot.local_path.as_deref(),
        Some(dest.to_string_lossy().as_ref())
    );
    assert_eq!(std::fs::read(&dest).unwrap(), model_data);
}

#[tokio::test]
async fn download_model_ollama_source_happy_path() {
    let model_data = b"fake-ollama-blob-content-4gb";
    let (port, _handle) = start_mock_model_server(model_data.to_vec()).await;

    let tmp = TempDir::new().unwrap();
    let mgr = LocalModelManager::new_for_test(tmp.path());

    let source: Box<dyn ModelSource> = Box::new(MockOllamaSource {
        manifest_url: format!("http://127.0.0.1:{port}/v2/library/llama3/manifests/8b"),
        blob_url: format!("http://127.0.0.1:{port}/v2/library/llama3/blobs/sha256:abc"),
        model_id: "llama3:8b".into(),
    });

    let progress = mgr
        .download_model_from_source("llama3:8b".into(), source)
        .await;
    assert_eq!(progress.status, "downloading");

    tokio::time::sleep(Duration::from_secs(3)).await;

    let snapshot = mgr.model_download_progress().await.unwrap();
    assert_eq!(snapshot.status, "completed");
    assert_eq!(snapshot.progress, Some(1.0));

    let dest = tmp.path().join("models").join("llama3_8b.gguf");
    assert!(
        dest.exists(),
        "Ollama model should be at {}",
        dest.display()
    );
    assert_eq!(std::fs::read(&dest).unwrap(), model_data);
}

#[tokio::test]
async fn download_model_custom_url_happy_path() {
    let model_data = b"fake-custom-url-model-content";
    let (port, _handle) = start_mock_model_server(model_data.to_vec()).await;

    let tmp = TempDir::new().unwrap();
    let mgr = LocalModelManager::new_for_test(tmp.path());

    let url = format!("http://127.0.0.1:{port}/model.gguf");
    let progress = mgr
        .download_model_with_source(
            "test-custom".into(),
            rua_local_model::manager::ModelSourceConfig::CustomUrl { url },
        )
        .await;
    assert_eq!(progress.status, "downloading");

    tokio::time::sleep(Duration::from_secs(3)).await;

    let snapshot = mgr.model_download_progress().await.unwrap();
    assert_eq!(snapshot.status, "completed");

    let dest = tmp.path().join("models").join("test-custom.gguf");
    assert!(dest.exists());
    assert_eq!(std::fs::read(&dest).unwrap(), model_data);
}

#[tokio::test]
async fn download_model_sanitizes_model_id_into_single_models_file() {
    let tmp = TempDir::new().unwrap();
    let mgr = LocalModelManager::new_for_test(tmp.path());
    let observed_dest = Arc::new(Mutex::new(None));

    let source: Box<dyn ModelSource> = Box::new(RecordingSource {
        bytes: b"path-safe-model".to_vec(),
        observed_dest: observed_dest.clone(),
    });

    let progress = mgr
        .download_model_from_source("../outside:tag".into(), source)
        .await;
    assert_eq!(progress.status, "downloading");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if mgr
            .model_download_progress()
            .await
            .is_ok_and(|p| p.status == "completed")
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "model download did not complete"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let dest = observed_dest
        .lock()
        .unwrap()
        .clone()
        .expect("source should see the resolved destination");
    let models_dir = tmp.path().join("models");
    assert_eq!(dest.parent(), Some(models_dir.as_path()));
    assert_eq!(
        dest.file_name().and_then(|s| s.to_str()),
        Some("..._outside_tag.gguf.downloading"),
        "sources should write to the Rust staging file, not the final GGUF"
    );
    let final_path = models_dir.join(".._outside_tag.gguf");
    assert_eq!(std::fs::read(&final_path).unwrap(), b"path-safe-model");
    assert!(
        !tmp.path().join("outside_tag.gguf").exists(),
        "sanitized ids must not escape the models directory"
    );
    assert!(
        !dest.exists(),
        "successful downloads should atomically move the staging file into the final model path"
    );
}

#[tokio::test]
async fn completed_download_is_listed_and_delete_removes_managed_file() {
    let tmp = TempDir::new().unwrap();
    let mgr = LocalModelManager::new_for_test(tmp.path());

    let source: Box<dyn ModelSource> = Box::new(RecordingSource {
        bytes: b"downloaded-model".to_vec(),
        observed_dest: Arc::new(Mutex::new(None)),
    });
    let model_id = "AgentScope/demo-model";

    mgr.download_model_from_source(model_id.into(), source)
        .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if mgr
            .model_download_progress()
            .await
            .is_ok_and(|progress| progress.status == "completed")
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "model download did not complete"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let models = mgr.list_models().await;
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, model_id);
    assert_eq!(models[0].name, model_id);
    assert_eq!(models[0].format.as_deref(), Some("gguf"));
    assert_eq!(models[0].size, Some("downloaded-model".len() as u64));
    let model_path = PathBuf::from(&models[0].path);
    assert_eq!(
        model_path.file_name().and_then(|name| name.to_str()),
        Some("AgentScope_demo-model.gguf")
    );
    assert_eq!(std::fs::read(&model_path).unwrap(), b"downloaded-model");

    mgr.delete_model(model_id).await.unwrap();

    assert!(
        mgr.list_models().await.is_empty(),
        "deleted downloaded models should disappear from the public model list"
    );
    assert!(
        !model_path.exists(),
        "deleting a managed downloaded model should remove the local GGUF file"
    );
}

#[tokio::test]
async fn download_model_source_failure_sets_failed_state_and_closes_stream() {
    let tmp = TempDir::new().unwrap();
    let mgr = LocalModelManager::new_for_test(tmp.path());

    let source: Box<dyn ModelSource> = Box::new(FailingSource {
        model_id: "fail-model".into(),
        error_msg: "subprocess died: signal 9".into(),
    });

    let progress = mgr
        .download_model_from_source("fail-model".into(), source)
        .await;
    assert_eq!(progress.status, "downloading");

    tokio::time::sleep(Duration::from_secs(2)).await;

    let snapshot = mgr.model_download_progress().await.unwrap();
    assert_eq!(snapshot.status, "failed");
    assert!(snapshot.error.as_ref().unwrap().contains("subprocess died"));
    assert!(
        mgr.subscribe_download("fail-model").await.is_err(),
        "terminal failure should remove the download stream from the subscription table"
    );
}

#[tokio::test]
async fn failed_model_download_keeps_existing_completed_file_and_removes_staging_file() {
    let tmp = TempDir::new().unwrap();
    let mgr = LocalModelManager::new_for_test(tmp.path());
    let model_id = "org/stable-model";
    let models_dir = tmp.path().join("models");
    std::fs::create_dir_all(&models_dir).unwrap();
    let final_path = models_dir.join("org_stable-model.gguf");
    std::fs::write(&final_path, b"previous-good-model").unwrap();

    mgr.download_model_from_source(
        model_id.into(),
        Box::new(PartialThenFailSource {
            bytes: b"corrupt-partial".to_vec(),
            error_msg: "network dropped after partial body".into(),
        }),
    )
    .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = mgr.model_download_progress().await.unwrap();
        if snapshot.status == "failed" {
            assert!(snapshot
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("network dropped"));
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "partial failure did not surface; last snapshot: {snapshot:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    assert_eq!(
        std::fs::read(&final_path).unwrap(),
        b"previous-good-model",
        "failed downloads must not truncate or replace the existing completed model"
    );
    assert!(
        !models_dir
            .join(".org_stable-model.gguf.downloading")
            .exists(),
        "failed downloads should clean the staging artifact"
    );
}

#[tokio::test]
async fn cancel_server_download_cleans_state_and_stream() {
    let tmp = TempDir::new().unwrap();
    let mgr = LocalModelManager::new_for_test(tmp.path());

    mgr.download_server("cancel-test".into()).await;

    assert!(mgr.server_download_progress().await.is_ok());
    assert!(mgr.subscribe_download("cancel-test").await.is_ok());

    mgr.cancel_server_download().await.unwrap();

    assert!(mgr.server_download_progress().await.is_err());
    assert!(mgr.subscribe_download("cancel-test").await.is_err());
    assert!(mgr.cancel_server_download().await.is_err());
}

#[tokio::test]
async fn starting_new_server_download_replaces_previous_state_and_stream() {
    let (port, _handle) = start_hanging_http_server().await;
    let tmp = TempDir::new().unwrap();
    let mgr = LocalModelManager::new_for_test(tmp.path())
        .with_server_api_url(format!("http://127.0.0.1:{port}/api/latest"));

    mgr.download_server("first-server-download".into()).await;
    assert!(
        mgr.subscribe_download("first-server-download")
            .await
            .is_ok(),
        "first server download should expose a stream while it is active"
    );

    mgr.download_server("second-server-download".into()).await;

    assert!(
        mgr.subscribe_download("first-server-download")
            .await
            .is_err(),
        "replacing an active server download must clean the stale stream entry"
    );
    assert!(
        mgr.subscribe_download("second-server-download")
            .await
            .is_ok(),
        "replacement server download should own the active stream"
    );
    let snapshot = mgr.server_download_progress().await.unwrap();
    assert_eq!(snapshot.model_id, "second-server-download");
    assert_eq!(snapshot.status, "downloading");

    mgr.cancel_server_download()
        .await
        .expect("replacement download should be cancellable");
    assert!(mgr
        .subscribe_download("second-server-download")
        .await
        .is_err());
}

#[tokio::test]
async fn cancel_model_download_cleans_state_and_stream() {
    let tmp = TempDir::new().unwrap();
    let mgr = LocalModelManager::new_for_test(tmp.path());

    mgr.download_model("cancel-model".into()).await;

    assert!(mgr.model_download_progress().await.is_ok());
    assert!(mgr.subscribe_download("cancel-model").await.is_ok());

    mgr.cancel_model_download().await.unwrap();

    assert!(mgr.model_download_progress().await.is_err());
    assert!(mgr.subscribe_download("cancel-model").await.is_err());
}

#[tokio::test]
async fn starting_new_model_download_replaces_previous_state_and_stream() {
    let tmp = TempDir::new().unwrap();
    let mgr = LocalModelManager::new_for_test(tmp.path());
    let started = Arc::new(Notify::new());
    let started_wait = started.notified();

    mgr.download_model_from_source(
        "first-model".into(),
        Box::new(BlockingSource {
            started: started.clone(),
        }),
    )
    .await;

    tokio::time::timeout(Duration::from_secs(2), started_wait)
        .await
        .expect("blocking source should start");
    assert!(
        mgr.subscribe_download("first-model").await.is_ok(),
        "first download should expose a stream while it is active"
    );

    let observed_dest = Arc::new(Mutex::new(None));
    mgr.download_model_from_source(
        "second-model".into(),
        Box::new(RecordingSource {
            bytes: b"second-model-content".to_vec(),
            observed_dest: observed_dest.clone(),
        }),
    )
    .await;

    assert!(
        mgr.subscribe_download("first-model").await.is_err(),
        "replacing an active download must clean the stale stream entry"
    );
    let snapshot = mgr.model_download_progress().await.unwrap();
    assert_eq!(snapshot.model_id, "second-model");
    assert_eq!(snapshot.status, "downloading");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = mgr.model_download_progress().await.unwrap();
        if snapshot.status == "completed" {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "replacement download did not complete; last snapshot: {snapshot:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let dest = observed_dest
        .lock()
        .unwrap()
        .clone()
        .expect("replacement source should receive a destination");
    assert_eq!(
        dest.file_name().and_then(|s| s.to_str()),
        Some(".second-model.gguf.downloading")
    );
    assert!(
        !dest.exists(),
        "replacement staging file should be moved away after success"
    );
    assert_eq!(
        std::fs::read(tmp.path().join("models").join("second-model.gguf")).unwrap(),
        b"second-model-content"
    );
}

#[tokio::test]
async fn download_model_broadcast_receives_all_lifecycle_messages() {
    let model_data = b"broadcast-lifecycle-test";
    let (port, _handle) = start_mock_model_server(model_data.to_vec()).await;

    let tmp = TempDir::new().unwrap();
    let mgr = LocalModelManager::new_for_test(tmp.path());

    let source: Box<dyn ModelSource> = Box::new(MockHfSource {
        api_url: format!("http://127.0.0.1:{port}/api"),
        download_url: format!("http://127.0.0.1:{port}/file.gguf"),
        model_id: "broadcast-test".into(),
    });

    mgr.download_model_from_source("broadcast-test".into(), source)
        .await;

    let mut rx = mgr.subscribe_download("broadcast-test").await.unwrap();

    let mut got_started = false;
    let mut got_progress = false;
    let mut got_completed = false;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        tokio::select! {
            msg = rx.recv() => {
                match msg {
                    Ok(IpcMessage::Status(s)) => match s.status {
                        DownloadStatus::Started => got_started = true,
                        DownloadStatus::Completed => { got_completed = true; break; }
                        _ => {}
                    },
                    Ok(IpcMessage::Progress(_)) => got_progress = true,
                    Ok(IpcMessage::Error(_)) => panic!("unexpected error"),
                    Err(_) => break,
                }
            }
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }

    assert!(got_started, "broadcast should have Started");
    assert!(got_progress, "broadcast should have Progress");
    assert!(got_completed, "broadcast should have Completed");
}
