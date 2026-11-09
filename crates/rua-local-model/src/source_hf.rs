use std::path::Path;

use async_trait::async_trait;
use reqwest::Client;
use rua_core::{Result, RuaError};
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tracing::debug;

use crate::download_ipc::{DownloadStatus, ErrorUpdate, IpcMessage, ProgressUpdate, StatusUpdate};
use crate::model_source::{ModelSource, ProgressTx};

const HF_API_BASE: &str = "https://huggingface.co/api/models";

#[derive(Debug, Clone, Deserialize)]
pub struct HfFileEntry {
    #[serde(rename = "rfilename")]
    pub filename: String,
    pub size: Option<u64>,
}

pub struct HfSource {
    pub repo_id: String,
    pub filename: Option<String>,
    client: Client,
    token: Option<String>,
}

impl HfSource {
    pub fn new(repo_id: impl Into<String>, filename: Option<String>) -> Self {
        let token = std::env::var("HF_TOKEN").ok();
        Self {
            repo_id: repo_id.into(),
            filename,
            client: rua_http::default_client_or_default("rua-local-model"),
            token,
        }
    }

    pub async fn list_gguf_files(&self) -> Result<Vec<HfFileEntry>> {
        let url = format!("{}/{}/tree/main", HF_API_BASE, self.repo_id);
        let mut req = self.client.get(&url);
        if let Some(ref token) = self.token {
            req = req.bearer_auth(token);
        }

        let resp = req.send().await.map_err(|e| RuaError::Io {
            message: format!("HF API request failed: {e}"),
        })?;

        if !resp.status().is_success() {
            return Err(RuaError::Io {
                message: format!("HF API returned {}", resp.status()),
            });
        }

        let files: Vec<HfFileEntry> = resp.json().await.map_err(|e| RuaError::Io {
            message: format!("HF API parse error: {e}"),
        })?;

        Ok(files
            .into_iter()
            .filter(|f| f.filename.ends_with(".gguf"))
            .collect())
    }

    async fn resolve_filename(&self) -> Result<String> {
        if let Some(ref name) = self.filename {
            return Ok(name.clone());
        }
        let files = self.list_gguf_files().await?;
        files
            .first()
            .map(|f| f.filename.clone())
            .ok_or_else(|| RuaError::NotFound {
                resource: "gguf file".into(),
                id: self.repo_id.clone(),
            })
    }

    pub fn download_url(&self, filename: &str) -> String {
        format!(
            "https://huggingface.co/{}/resolve/main/{}",
            self.repo_id, filename
        )
    }
}

#[async_trait]
impl ModelSource for HfSource {
    fn name(&self) -> &str {
        "huggingface"
    }

    async fn fetch_to(&self, dest_path: &Path, progress_tx: ProgressTx) -> Result<()> {
        let id = self.repo_id.clone();

        progress_tx
            .send(IpcMessage::Status(StatusUpdate {
                id: id.clone(),
                status: DownloadStatus::Started,
                detail: Some("resolving filename".into()),
            }))
            .ok();

        let filename = self.resolve_filename().await.inspect_err(|e| {
            progress_tx
                .send(IpcMessage::Error(ErrorUpdate {
                    id: id.clone(),
                    error: e.to_string(),
                    retryable: Some(false),
                }))
                .ok();
        })?;

        let url = self.download_url(&filename);
        debug!(repo = %self.repo_id, file = %filename, "downloading from HF");

        let mut req = self.client.get(&url);
        if let Some(ref token) = self.token {
            req = req.bearer_auth(token);
        }

        let resp = req.send().await.map_err(|e| {
            let err = RuaError::Io {
                message: format!("HF download request failed: {e}"),
            };
            progress_tx
                .send(IpcMessage::Error(ErrorUpdate {
                    id: id.clone(),
                    error: err.to_string(),
                    retryable: Some(true),
                }))
                .ok();
            err
        })?;

        if !resp.status().is_success() {
            let err = RuaError::Io {
                message: format!("HF download returned {}", resp.status()),
            };
            progress_tx
                .send(IpcMessage::Error(ErrorUpdate {
                    id: id.clone(),
                    error: err.to_string(),
                    retryable: Some(false),
                }))
                .ok();
            return Err(err);
        }

        let total = resp.content_length().unwrap_or(0);
        let mut stream = resp.bytes_stream();
        let mut file = tokio::fs::File::create(dest_path)
            .await
            .map_err(|e| RuaError::Io {
                message: format!("failed to create dest file: {e}"),
            })?;

        let mut downloaded: u64 = 0;
        use futures_util::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| RuaError::Io {
                message: format!("HF download stream error: {e}"),
            })?;
            file.write_all(&chunk).await.map_err(|e| RuaError::Io {
                message: format!("failed to write model file: {e}"),
            })?;
            downloaded += chunk.len() as u64;
            progress_tx
                .send(IpcMessage::Progress(ProgressUpdate {
                    id: id.clone(),
                    bytes_downloaded: downloaded,
                    bytes_total: total,
                    eta_secs: None,
                }))
                .ok();
        }

        file.flush().await.map_err(|e| RuaError::Io {
            message: format!("failed to flush model file: {e}"),
        })?;

        progress_tx
            .send(IpcMessage::Status(StatusUpdate {
                id: id.clone(),
                status: DownloadStatus::Completed,
                detail: Some(filename),
            }))
            .ok();

        Ok(())
    }
}
