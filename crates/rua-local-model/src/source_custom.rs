use std::path::Path;

use async_trait::async_trait;
use reqwest::Client;
use rua_core::{Result, RuaError};
use tokio::io::AsyncWriteExt;
use tracing::debug;

use crate::download_ipc::{DownloadStatus, ErrorUpdate, IpcMessage, ProgressUpdate, StatusUpdate};
use crate::model_source::{ModelSource, ProgressTx};

pub struct CustomUrlSource {
    pub url: String,
    pub model_id: String,
    client: Client,
}

impl CustomUrlSource {
    pub fn new(url: impl Into<String>, model_id: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            model_id: model_id.into(),
            client: rua_http::default_client_or_default("rua-local-model"),
        }
    }
}

#[async_trait]
impl ModelSource for CustomUrlSource {
    fn name(&self) -> &str {
        "custom_url"
    }

    async fn fetch_to(&self, dest_path: &Path, progress_tx: ProgressTx) -> Result<()> {
        let id = self.model_id.clone();

        progress_tx
            .send(IpcMessage::Status(StatusUpdate {
                id: id.clone(),
                status: DownloadStatus::Started,
                detail: Some(self.url.clone()),
            }))
            .ok();

        debug!(url = %self.url, model_id = %self.model_id, "downloading from custom URL");

        let resp = self.client.get(&self.url).send().await.map_err(|e| {
            let err = RuaError::Io {
                message: format!("custom URL download failed: {e}"),
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
                message: format!("custom URL returned {}", resp.status()),
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
                message: format!("custom URL stream error: {e}"),
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
                detail: None,
            }))
            .ok();

        Ok(())
    }
}
