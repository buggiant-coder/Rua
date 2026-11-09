use std::path::Path;

use async_trait::async_trait;
use reqwest::Client;
use rua_core::{Result, RuaError};
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tracing::debug;

use crate::download_ipc::{DownloadStatus, ErrorUpdate, IpcMessage, ProgressUpdate, StatusUpdate};
use crate::model_source::{ModelSource, ProgressTx};

const OLLAMA_REGISTRY: &str = "https://registry.ollama.ai";

#[derive(Debug, Clone, Deserialize)]
pub struct OllamaLayer {
    pub digest: String,
    pub size: u64,
    #[serde(rename = "mediaType")]
    pub media_type: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OllamaManifest {
    pub layers: Vec<OllamaLayer>,
}

impl OllamaManifest {
    pub fn model_layer(&self) -> Option<&OllamaLayer> {
        self.layers
            .iter()
            .filter(|l| l.media_type.contains("model"))
            .max_by_key(|l| l.size)
    }
}

pub struct OllamaSource {
    pub model_name: String,
    pub tag: String,
    client: Client,
}

impl OllamaSource {
    pub fn new(model_name: impl Into<String>, tag: Option<String>) -> Self {
        Self {
            model_name: model_name.into(),
            tag: tag.unwrap_or_else(|| "latest".into()),
            client: rua_http::default_client_or_default("rua-local-model"),
        }
    }

    pub async fn fetch_manifest(&self) -> Result<OllamaManifest> {
        let url = format!(
            "{}/v2/library/{}/manifests/{}",
            OLLAMA_REGISTRY, self.model_name, self.tag
        );
        debug!(model = %self.model_name, tag = %self.tag, "fetching Ollama manifest");

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| RuaError::Io {
                message: format!("Ollama manifest request failed: {e}"),
            })?;

        if !resp.status().is_success() {
            return Err(RuaError::Io {
                message: format!("Ollama registry returned {}", resp.status()),
            });
        }

        resp.json().await.map_err(|e| RuaError::Io {
            message: format!("Ollama manifest parse error: {e}"),
        })
    }

    pub fn blob_url(&self, digest: &str) -> String {
        format!(
            "{}/v2/library/{}/blobs/{}",
            OLLAMA_REGISTRY, self.model_name, digest
        )
    }
}

#[async_trait]
impl ModelSource for OllamaSource {
    fn name(&self) -> &str {
        "ollama"
    }

    async fn fetch_to(&self, dest_path: &Path, progress_tx: ProgressTx) -> Result<()> {
        let id = format!("{}:{}", self.model_name, self.tag);

        progress_tx
            .send(IpcMessage::Status(StatusUpdate {
                id: id.clone(),
                status: DownloadStatus::Started,
                detail: Some("fetching manifest".into()),
            }))
            .ok();

        let manifest = self.fetch_manifest().await.inspect_err(|e| {
            progress_tx
                .send(IpcMessage::Error(ErrorUpdate {
                    id: id.clone(),
                    error: e.to_string(),
                    retryable: Some(true),
                }))
                .ok();
        })?;

        let layer = manifest.model_layer().ok_or_else(|| {
            let err = RuaError::NotFound {
                resource: "model layer".into(),
                id: id.clone(),
            };
            progress_tx
                .send(IpcMessage::Error(ErrorUpdate {
                    id: id.clone(),
                    error: err.to_string(),
                    retryable: Some(false),
                }))
                .ok();
            err
        })?;

        let total = layer.size;
        let url = self.blob_url(&layer.digest);
        debug!(model = %self.model_name, digest = %layer.digest, size = total, "downloading blob");

        let resp = self.client.get(&url).send().await.map_err(|e| {
            let err = RuaError::Io {
                message: format!("Ollama blob download failed: {e}"),
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
                message: format!("Ollama blob returned {}", resp.status()),
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
                message: format!("Ollama blob stream error: {e}"),
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
                detail: Some(layer.digest.clone()),
            }))
            .ok();

        Ok(())
    }
}
