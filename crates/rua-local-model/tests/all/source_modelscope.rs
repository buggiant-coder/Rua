use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use rua_core::Result;
use rua_local_model::manager::{LocalModelManager, ModelSourceConfig};
use rua_local_model::model_source::{ModelSource, ProgressTx};
use rua_local_model::source_modelscope::ModelScopeSource;
use rua_local_model::{DownloadStatus, IpcMessage, ProgressUpdate, StatusUpdate};
use tempfile::TempDir;
use tokio::net::TcpListener;

#[test]
fn source_name() {
    let src = ModelScopeSource::new("damo/nlp_bert_backbone_base_std", None);
    assert_eq!(src.name(), "modelscope");
    assert_eq!(src.repo_id, "damo/nlp_bert_backbone_base_std");
}

#[test]
fn download_url_format() {
    let src = ModelScopeSource::new("damo/nlp_bert_backbone_base_std", None);
    let url = src.download_url("model-q4.gguf");
    assert_eq!(
        url,
        "https://www.modelscope.cn/models/damo/nlp_bert_backbone_base_std/resolve/master/model-q4.gguf"
    );
}

#[test]
fn parse_ms_file_entry() {
    let json = r#"{"Name":"model-q4.gguf","Size":4096}"#;
    let entry: rua_local_model::source_modelscope::MsFileEntry =
        serde_json::from_str(json).unwrap();
    assert_eq!(entry.name, "model-q4.gguf");
    assert_eq!(entry.size, Some(4096));
}

#[test]
fn infer_ms_prefix() {
    let config = test_infer("ms:damo/Rua-Qwen2.5-7B-GGUF");
    match config {
        ModelSourceConfig::ModelScope { repo_id, filename } => {
            assert_eq!(repo_id, "damo/Rua-Qwen2.5-7B-GGUF");
            assert!(filename.is_none());
        }
        other => panic!("expected ModelScope, got {:?}", other),
    }
}

#[test]
fn infer_modelscope_prefix() {
    let config = test_infer("modelscope:AgentScope/agentscope-7b-gguf");
    match config {
        ModelSourceConfig::ModelScope { repo_id, filename } => {
            assert_eq!(repo_id, "AgentScope/agentscope-7b-gguf");
            assert!(filename.is_none());
        }
        other => panic!("expected ModelScope, got {:?}", other),
    }
}

#[test]
fn infer_ollama_still_works() {
    let config = test_infer("llama3:8b");
    assert!(matches!(config, ModelSourceConfig::Ollama { .. }));
}

#[test]
fn infer_hf_still_works() {
    let config = test_infer("lmstudio-community/Meta-Llama-3.1-8B-GGUF");
    assert!(matches!(config, ModelSourceConfig::HuggingFace { .. }));
}

#[test]
fn infer_custom_url_still_works() {
    let config = test_infer("https://example.com/model.gguf");
    assert!(matches!(config, ModelSourceConfig::CustomUrl { .. }));
}

fn test_infer(model_id: &str) -> ModelSourceConfig {
    if model_id.starts_with("http://") || model_id.starts_with("https://") {
        ModelSourceConfig::CustomUrl {
            url: model_id.to_string(),
        }
    } else if let Some(rest) = model_id
        .strip_prefix("ms:")
        .or_else(|| model_id.strip_prefix("modelscope:"))
    {
        ModelSourceConfig::ModelScope {
            repo_id: rest.to_string(),
            filename: None,
        }
    } else if model_id.contains(':') {
        let parts: Vec<&str> = model_id.splitn(2, ':').collect();
        ModelSourceConfig::Ollama {
            model_name: parts[0].to_string(),
            tag: Some(parts[1].to_string()),
        }
    } else {
        ModelSourceConfig::HuggingFace {
            repo_id: model_id.to_string(),
            filename: None,
        }
    }
}

struct MockModelScopeSource {
    download_url: String,
    model_id: String,
}

#[async_trait]
impl ModelSource for MockModelScopeSource {
    fn name(&self) -> &str {
        "mock_modelscope"
    }

    async fn fetch_to(&self, dest_path: &Path, progress_tx: ProgressTx) -> Result<()> {
        let id = self.model_id.clone();
        let client = reqwest::Client::new();

        progress_tx
            .send(IpcMessage::Status(StatusUpdate {
                id: id.clone(),
                status: DownloadStatus::Started,
                detail: Some("resolving from mock ModelScope".into()),
            }))
            .ok();

        let resp =
            client
                .get(&self.download_url)
                .send()
                .await
                .map_err(|e| rua_core::RuaError::Io {
                    message: format!("mock ModelScope download failed: {e}"),
                })?;

        let total = resp.content_length().unwrap_or(0);
        let bytes = resp.bytes().await.map_err(|e| rua_core::RuaError::Io {
            message: format!("mock ModelScope stream error: {e}"),
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
                detail: Some("model-q4.gguf".into()),
            }))
            .ok();

        Ok(())
    }
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

#[tokio::test]
async fn download_model_modelscope_source_happy_path() {
    let model_data = b"fake-modelscope-gguf-content";
    let (port, _handle) = start_mock_model_server(model_data.to_vec()).await;

    let tmp = TempDir::new().unwrap();
    let mgr = LocalModelManager::new_for_test(tmp.path());

    let source: Box<dyn ModelSource> = Box::new(MockModelScopeSource {
        download_url: format!("http://127.0.0.1:{port}/models/damo/test/resolve/master/model.gguf"),
        model_id: "ms:damo/test-model".into(),
    });

    let progress = mgr
        .download_model_from_source("ms:damo/test-model".into(), source)
        .await;
    assert_eq!(progress.status, "downloading");

    tokio::time::sleep(Duration::from_secs(3)).await;

    let snapshot = mgr.model_download_progress().await.unwrap();
    assert_eq!(snapshot.status, "completed");
    assert_eq!(snapshot.progress, Some(1.0));

    let dest = tmp.path().join("models").join("ms_damo_test-model.gguf");
    assert!(
        dest.exists(),
        "ModelScope model should be at {}",
        dest.display()
    );
    assert_eq!(std::fs::read(&dest).unwrap(), model_data);
}

#[tokio::test]
async fn download_model_with_modelscope_config() {
    let tmp = TempDir::new().unwrap();
    let mgr = LocalModelManager::new_for_test(tmp.path());

    let model_data = b"ms-config-test-content";
    let (port, _handle) = start_mock_model_server(model_data.to_vec()).await;

    let source: Box<dyn ModelSource> = Box::new(MockModelScopeSource {
        download_url: format!("http://127.0.0.1:{port}/model.gguf"),
        model_id: "ms-config-test".into(),
    });

    let progress = mgr
        .download_model_from_source("ms-config-test".into(), source)
        .await;
    assert_eq!(progress.status, "downloading");

    tokio::time::sleep(Duration::from_secs(3)).await;

    let snapshot = mgr.model_download_progress().await.unwrap();
    assert_eq!(snapshot.status, "completed");
}
