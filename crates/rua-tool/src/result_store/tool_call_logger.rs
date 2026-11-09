use std::path::{Path, PathBuf};

use async_trait::async_trait;
use md5::{Digest, Md5};
use tokio::fs;
use tokio::io::{AsyncWriteExt, BufWriter};

use super::{CleanupPolicy, CleanupReport, ToolResultBlob, ToolResultStore};
use crate::error::ToolError;
use crate::executor::RawCommandCapture;

const EXCLUDED_TOOLS: &[&str] = &["recall_tool_result"];

pub fn recall_key(tool_use_id: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(tool_use_id.as_bytes());
    let result = hasher.finalize();
    format!("{:x}", result)[..16].to_string()
}

pub struct ToolCallArchive {
    base_dir: PathBuf,
}

impl ToolCallArchive {
    pub fn new(base_dir: impl AsRef<Path>) -> Self {
        Self {
            base_dir: base_dir.as_ref().to_path_buf(),
        }
    }

    fn file_for_key(&self, key: &str) -> PathBuf {
        self.base_dir.join(format!("{key}.json"))
    }

    fn file_for_id(&self, tool_use_id: &str) -> PathBuf {
        self.file_for_key(&recall_key(tool_use_id))
    }

    fn output_file_for_key(&self, key: &str) -> PathBuf {
        self.base_dir.join(format!("{key}.output"))
    }

    pub async fn has_entry(&self, tool_use_id: &str) -> bool {
        let path = self.file_for_id(tool_use_id);
        fs::metadata(&path).await.is_ok()
    }

    async fn ensure_dir(&self) -> Result<(), ToolError> {
        fs::create_dir_all(&self.base_dir)
            .await
            .map_err(|e| ToolError::ResultStore {
                message: format!("failed to create tool_call_logs dir: {e}"),
            })
    }

    pub async fn save_call(
        &self,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &serde_json::Value,
        tool_output: &str,
    ) -> Result<(), ToolError> {
        if tool_use_id.is_empty() {
            return Ok(());
        }
        if EXCLUDED_TOOLS.contains(&tool_name) {
            return Ok(());
        }

        self.ensure_dir().await?;

        let data = serde_json::json!({
            "tool_use_id": tool_use_id,
            "tool_name": tool_name,
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "input": tool_input,
            "output": tool_output,
        });

        let path = self.file_for_id(tool_use_id);
        let json_str = serde_json::to_string_pretty(&data).map_err(|e| ToolError::ResultStore {
            message: format!("failed to serialize tool call log: {e}"),
        })?;

        fs::write(&path, json_str)
            .await
            .map_err(|e| ToolError::ResultStore {
                message: format!("failed to write tool call log: {e}"),
            })?;

        tracing::debug!(
            tool_name = tool_name,
            key = recall_key(tool_use_id),
            "Saved tool call log"
        );

        Ok(())
    }

    pub async fn save_captured_call(
        &self,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &serde_json::Value,
        capture: &RawCommandCapture,
    ) -> Result<(), ToolError> {
        if tool_use_id.is_empty() || EXCLUDED_TOOLS.contains(&tool_name) {
            return Ok(());
        }
        self.ensure_dir().await?;
        let key = recall_key(tool_use_id);
        let output_path = self.output_file_for_key(&key);
        let temp_path = output_path.with_extension(format!("output.tmp.{}", std::process::id()));
        let file = fs::File::create(&temp_path)
            .await
            .map_err(result_store_error("create captured Tool output"))?;
        let mut writer = BufWriter::new(file);
        append_capture_file(&mut writer, "[stdout]\n", capture.stdout_path.as_deref()).await?;
        append_capture_file(&mut writer, "\n[stderr]\n", capture.stderr_path.as_deref()).await?;
        writer
            .flush()
            .await
            .map_err(result_store_error("flush captured Tool output"))?;
        writer
            .get_ref()
            .sync_all()
            .await
            .map_err(result_store_error("sync captured Tool output"))?;
        drop(writer);
        fs::rename(&temp_path, &output_path)
            .await
            .map_err(result_store_error("publish captured Tool output"))?;

        let data = serde_json::json!({
            "tool_use_id": tool_use_id,
            "tool_name": tool_name,
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "input": tool_input,
            "output_file": output_path.file_name().and_then(|name| name.to_str()),
        });
        let json_str =
            serde_json::to_string_pretty(&data).map_err(|error| ToolError::ResultStore {
                message: format!("failed to serialize captured tool call log: {error}"),
            })?;
        fs::write(self.file_for_key(&key), json_str)
            .await
            .map_err(result_store_error("write captured Tool metadata"))?;
        Ok(())
    }
}

#[async_trait]
impl ToolResultStore for ToolCallArchive {
    async fn save(&self, blob: ToolResultBlob) -> Result<(), ToolError> {
        self.ensure_dir().await?;

        let key = blob.id.clone();
        let input_json = blob.input.clone().unwrap_or(serde_json::json!({}));
        let data = serde_json::json!({
            "tool_use_id": blob.id,
            "tool_name": blob.tool_name,
            "timestamp": blob.created_at.to_rfc3339(),
            "input": input_json,
            "output": blob.content,
        });

        let path = self.file_for_key(&key);
        let json_str = serde_json::to_string_pretty(&data).map_err(|e| ToolError::ResultStore {
            message: format!("failed to serialize tool call log: {e}"),
        })?;

        fs::write(&path, json_str)
            .await
            .map_err(|e| ToolError::ResultStore {
                message: format!("failed to write tool call log: {e}"),
            })?;

        Ok(())
    }

    async fn read(&self, key: &str) -> Result<ToolResultBlob, ToolError> {
        let path = self.file_for_key(key);
        let json_str = fs::read_to_string(&path)
            .await
            .map_err(|_| ToolError::ResultStore {
                message: format!("tool call log not found for key: {key}"),
            })?;

        let data: serde_json::Value =
            serde_json::from_str(&json_str).map_err(|e| ToolError::ResultStore {
                message: format!("invalid tool call log JSON: {e}"),
            })?;

        let tool_name = data["tool_name"].as_str().unwrap_or("").to_string();

        let output = match data.get("output_file").and_then(serde_json::Value::as_str) {
            Some(file_name) => fs::read_to_string(self.base_dir.join(file_name))
                .await
                .map_err(result_store_error("read captured Tool output"))?,
            None => data["output"].as_str().unwrap_or("").to_string(),
        };
        let input_value = data.get("input").cloned();

        Ok(ToolResultBlob {
            id: key.to_string(),
            tool_name,
            content: output,
            mime_type: "text/plain".to_string(),
            created_at: chrono::Utc::now(),
            input: input_value,
        })
    }

    async fn cleanup(&self, policy: CleanupPolicy) -> Result<CleanupReport, ToolError> {
        let (mut files_removed, mut bytes_freed) =
            remove_stale_files(&self.base_dir.join(".raw-capture"), policy.max_age).await?;

        let mut entries = match fs::read_dir(&self.base_dir).await {
            Ok(e) => e,
            Err(_) => {
                return Ok(CleanupReport {
                    files_removed: 0,
                    bytes_freed: 0,
                })
            }
        };

        let now = std::time::SystemTime::now();

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| ToolError::ResultStore {
                message: format!("failed to read dir entry: {e}"),
            })?
        {
            let metadata = entry.metadata().await.map_err(|e| ToolError::ResultStore {
                message: format!("failed to read file metadata: {e}"),
            })?;

            if let Ok(modified) = metadata.modified() {
                if let Ok(age) = now.duration_since(modified) {
                    if age > policy.max_age {
                        let size = metadata.len();
                        if entry.path().extension().and_then(|value| value.to_str()) == Some("json")
                        {
                            if let Ok(json) = fs::read_to_string(entry.path()).await {
                                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&json)
                                {
                                    if let Some(file_name) =
                                        value.get("output_file").and_then(serde_json::Value::as_str)
                                    {
                                        let _ =
                                            fs::remove_file(self.base_dir.join(file_name)).await;
                                    }
                                }
                            }
                        }
                        if fs::remove_file(entry.path()).await.is_ok() {
                            files_removed += 1;
                            bytes_freed += size;
                        }
                    }
                }
            }
        }

        Ok(CleanupReport {
            files_removed,
            bytes_freed,
        })
    }
}

async fn remove_stale_files(
    directory: &Path,
    max_age: std::time::Duration,
) -> Result<(usize, u64), ToolError> {
    let mut entries = match fs::read_dir(directory).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((0, 0)),
        Err(error) => {
            return Err(ToolError::ResultStore {
                message: format!("failed to read stale capture directory: {error}"),
            })
        }
    };
    let now = std::time::SystemTime::now();
    let mut files_removed = 0;
    let mut bytes_freed = 0;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(result_store_error("read stale capture entry"))?
    {
        let metadata = entry
            .metadata()
            .await
            .map_err(result_store_error("read stale capture metadata"))?;
        if !metadata.is_file() {
            continue;
        }
        let is_stale = metadata
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > max_age);
        if is_stale && fs::remove_file(entry.path()).await.is_ok() {
            files_removed += 1;
            bytes_freed += metadata.len();
        }
    }
    Ok((files_removed, bytes_freed))
}

async fn append_capture_file(
    writer: &mut BufWriter<fs::File>,
    heading: &str,
    path: Option<&Path>,
) -> Result<(), ToolError> {
    writer
        .write_all(heading.as_bytes())
        .await
        .map_err(result_store_error("write captured Tool heading"))?;
    let Some(path) = path else {
        return Ok(());
    };
    let mut source = fs::File::open(path)
        .await
        .map_err(result_store_error("open captured Tool stream"))?;
    tokio::io::copy(&mut source, writer)
        .await
        .map_err(result_store_error("copy captured Tool stream"))?;
    Ok(())
}

fn result_store_error(operation: &'static str) -> impl FnOnce(std::io::Error) -> ToolError {
    move |error| ToolError::ResultStore {
        message: format!("{operation}: {error}"),
    }
}
