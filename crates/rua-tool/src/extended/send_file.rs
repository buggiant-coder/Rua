use std::path::Path;

use async_trait::async_trait;
use serde::Deserialize;

use rua_contract::Tool;
use rua_core::tool::{ToolContext, ToolResult, ToolSchema};
use rua_core::ContentPart;

use crate::annotations::{ApprovalPolicy, ExtendedToolAnnotations};
use crate::schema::ToolSpec;

const MAX_SEND_SIZE: u64 = 100 * 1024 * 1024;

pub fn mime_from_extension(path: &Path) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "txt" | "log" | "md" | "csv" | "tsv" => "text/plain".into(),
        "json" => "application/json".into(),
        "xml" => "application/xml".into(),
        "html" | "htm" => "text/html".into(),
        "css" => "text/css".into(),
        "js" => "application/javascript".into(),
        "pdf" => "application/pdf".into(),
        "zip" => "application/zip".into(),
        "tar" => "application/x-tar".into(),
        "gz" | "gzip" => "application/gzip".into(),
        "jpg" | "jpeg" => "image/jpeg".into(),
        "png" => "image/png".into(),
        "gif" => "image/gif".into(),
        "webp" => "image/webp".into(),
        "svg" => "image/svg+xml".into(),
        "mp4" | "m4v" => "video/mp4".into(),
        "mp3" => "audio/mpeg".into(),
        "wav" => "audio/wav".into(),
        "doc" | "docx" => "application/msword".into(),
        "xls" | "xlsx" => "application/vnd.ms-excel".into(),
        "ppt" | "pptx" => "application/vnd.ms-powerpoint".into(),
        _ => "application/octet-stream".into(),
    }
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct SendFileArgs {
    file_path: String,
    #[serde(default)]
    channel: Option<String>,
}

pub struct SendFileTool;

impl SendFileTool {
    pub fn new() -> Self {
        Self
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "send_file".to_string(),
            description: "Send a file to the user through the active channel. \
                          The file will be delivered as an attachment."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Path to the file to send"
                    },
                    "channel": {
                        "type": "string",
                        "description": "Target channel (default: active channel)"
                    }
                },
                "required": ["file_path"]
            }),
            annotations: ExtendedToolAnnotations {
                read_only: true,
                destructive: false,
                idempotent: true,
                requires_approval: ApprovalPolicy::UnlessTrusted,
                allowed_on_mobile: true,
                ..Default::default()
            },
            namespace: "builtin".into(),
        }
    }
}

impl Default for SendFileTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for SendFileTool {
    fn name(&self) -> &str {
        "send_file"
    }

    fn schema(&self) -> ToolSchema {
        let spec = Self::spec();
        ToolSchema {
            name: spec.name,
            description: spec.description,
            parameters: spec.parameters,
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
    ) -> rua_core::Result<ToolResult> {
        let args: SendFileArgs =
            serde_json::from_value(args).map_err(|e| rua_core::RuaError::Tool {
                tool: "send_file".into(),
                message: e.to_string(),
            })?;

        let path = ctx.working_dir.join(&args.file_path);

        if !path.exists() {
            return Ok(ToolResult::error(
                "send_file",
                format!("File not found: {}", args.file_path),
            ));
        }

        if path.is_dir() {
            return Ok(ToolResult::error(
                "send_file",
                format!("'{}' is a directory, not a file", args.file_path),
            ));
        }

        let metadata = std::fs::metadata(&path).map_err(|e| rua_core::RuaError::Tool {
            tool: "send_file".into(),
            message: format!("Failed to read file metadata: {e}"),
        })?;

        let file_size = metadata.len();

        if file_size > MAX_SEND_SIZE {
            return Ok(ToolResult::error(
                "send_file",
                format!(
                    "File too large: {:.1} MB (max {:.0} MB)",
                    file_size as f64 / (1024.0 * 1024.0),
                    MAX_SEND_SIZE as f64 / (1024.0 * 1024.0),
                ),
            ));
        }

        let mime = mime_from_extension(&path);
        let file_name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let abs_path = path.canonicalize().unwrap_or(path);
        let file_url = format!("file://{}", abs_path.display());

        Ok(ToolResult {
            call_id: "send_file".into(),
            name: None,
            content: vec![
                ContentPart::File {
                    url: file_url,
                    name: file_name,
                    media_type: Some(mime),
                    size: Some(file_size),
                },
                ContentPart::Text {
                    text: "File sent successfully.".into(),
                },
            ],
            is_error: false,
            duration_ms: None,
            attachments: None,
            truncation: None,
            metadata: serde_json::Map::new(),
        })
    }
}
