use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::Deserialize;

use agent_types::ContentPart;
use rua_contract::Tool;
use rua_core::tool::{ToolContext, ToolResult, ToolSchema};

use crate::annotations::{ApprovalPolicy, ExtendedToolAnnotations};
use crate::schema::ToolSpec;

pub const MAX_IMAGE_SIZE: u64 = 20 * 1024 * 1024;

pub const MAX_VIDEO_SIZE: u64 = 100 * 1024 * 1024;

const IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "bmp", "svg", "ico", "tiff", "tif", "heic", "heif", "avif",
];

const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "avi", "mov", "webm", "flv", "wmv", "m4v", "3gp", "ogv",
];

pub fn mime_from_extension(path: &Path) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg".into(),
        "png" => "image/png".into(),
        "gif" => "image/gif".into(),
        "webp" => "image/webp".into(),
        "svg" => "image/svg+xml".into(),
        "bmp" => "image/bmp".into(),
        "ico" => "image/x-icon".into(),
        "tiff" | "tif" => "image/tiff".into(),
        "heic" | "heif" => "image/heif".into(),
        "avif" => "image/avif".into(),
        "mp4" | "m4v" => "video/mp4".into(),
        "mkv" => "video/x-matroska".into(),
        "avi" => "video/x-msvideo".into(),
        "mov" => "video/quicktime".into(),
        "webm" => "video/webm".into(),
        "flv" => "video/x-flv".into(),
        "wmv" => "video/x-ms-wmv".into(),
        "3gp" => "video/3gpp".into(),
        "ogv" => "video/ogg".into(),
        _ => "application/octet-stream".into(),
    }
}

pub fn is_image_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| IMAGE_EXTENSIONS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

pub fn is_video_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| VIDEO_EXTENSIONS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

pub fn base64_encode(input: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        result.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(CHARS[(triple & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

fn is_url(path: &str) -> bool {
    path.starts_with("http://") || path.starts_with("https://")
}

fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    } else if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}

#[derive(Debug, Deserialize)]
struct ViewImageArgs {
    image_path: String,
}

pub struct ViewImageTool;

impl ViewImageTool {
    pub fn new() -> Self {
        Self
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "view_image".to_string(),
            description: "Load an image file into the model context so the model can see it. \
                          The image is base64-encoded and returned as a content block."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "image_path": {
                        "type": "string",
                        "description": "Local path or HTTP(S) URL of the image to view"
                    }
                },
                "required": ["image_path"]
            }),
            annotations: ExtendedToolAnnotations {
                read_only: true,
                idempotent: true,
                requires_approval: ApprovalPolicy::Never,
                allowed_on_mobile: true,
                ..Default::default()
            },
            namespace: "builtin".into(),
        }
    }
}

impl Default for ViewImageTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ViewImageTool {
    fn name(&self) -> &str {
        "view_image"
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
        let args: ViewImageArgs =
            serde_json::from_value(args).map_err(|e| rua_core::RuaError::Tool {
                tool: "view_image".into(),
                message: e.to_string(),
            })?;

        if is_url(&args.image_path) {
            let url_path = args
                .image_path
                .split('?')
                .next()
                .unwrap_or(&args.image_path);
            let p = Path::new(url_path);
            if p.extension().is_some() && !is_image_extension(p) {
                return Ok(ToolResult::error(
                    "view_image",
                    format!(
                        "URL does not point to a supported image format: {}",
                        args.image_path
                    ),
                ));
            }
            return Ok(ToolResult {
                call_id: "view_image".into(),
                name: None,
                content: vec![
                    ContentPart::Image {
                        url: args.image_path.clone(),
                        media_type: None,
                        alt: None,
                    },
                    ContentPart::Text {
                        text: format!("Image loaded from URL: {}", args.image_path),
                    },
                ],
                is_error: false,
                duration_ms: None,
                attachments: None,
                truncation: None,
                metadata: serde_json::Map::new(),
            });
        }

        let expanded = expand_tilde(&args.image_path);
        let path = ctx.working_dir.join(expanded);

        if !path.exists() {
            return Ok(ToolResult::error(
                "view_image",
                format!("File not found: {}", args.image_path),
            ));
        }

        if path.is_dir() {
            return Ok(ToolResult::error(
                "view_image",
                format!("'{}' is a directory, not an image file", args.image_path),
            ));
        }

        if !is_image_extension(&path) {
            return Ok(ToolResult::error(
                "view_image",
                format!(
                    "File '{}' does not appear to be a supported image format. \
                     Supported: {}",
                    args.image_path,
                    IMAGE_EXTENSIONS.join(", ")
                ),
            ));
        }

        let metadata = std::fs::metadata(&path).map_err(|e| rua_core::RuaError::Tool {
            tool: "view_image".into(),
            message: format!("Failed to read file metadata: {e}"),
        })?;

        let file_size = metadata.len();

        if file_size > MAX_IMAGE_SIZE {
            return Ok(ToolResult::error(
                "view_image",
                format!(
                    "Image file is too large ({:.1} MB). Maximum supported size is {:.0} MB.",
                    file_size as f64 / (1024.0 * 1024.0),
                    MAX_IMAGE_SIZE as f64 / (1024.0 * 1024.0),
                ),
            ));
        }

        let mime = mime_from_extension(&path);

        let image_bytes = std::fs::read(&path).map_err(|e| rua_core::RuaError::Tool {
            tool: "view_image".into(),
            message: format!("Failed to read image file: {e}"),
        })?;

        let b64 = base64_encode(&image_bytes);
        let data_url = format!("data:{mime};base64,{b64}");
        let file_name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        Ok(ToolResult {
            call_id: "view_image".into(),
            name: None,
            content: vec![
                ContentPart::Image {
                    url: data_url,
                    media_type: Some(mime),
                    alt: Some(file_name.clone()),
                },
                ContentPart::Text {
                    text: format!("Image loaded: {file_name}"),
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

#[derive(Debug, Deserialize)]
struct ViewVideoArgs {
    video_path: String,
}

pub struct ViewVideoTool;

impl ViewVideoTool {
    pub fn new() -> Self {
        Self
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "view_video".to_string(),
            description: "Load a video file into the model context so the model can see it. \
                          Provides the video as a file reference for analysis."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "video_path": {
                        "type": "string",
                        "description": "Local path or HTTP(S) URL of the video to view"
                    }
                },
                "required": ["video_path"]
            }),
            annotations: ExtendedToolAnnotations {
                read_only: true,
                idempotent: true,
                requires_approval: ApprovalPolicy::Never,
                allowed_on_mobile: true,
                ..Default::default()
            },
            namespace: "builtin".into(),
        }
    }
}

impl Default for ViewVideoTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ViewVideoTool {
    fn name(&self) -> &str {
        "view_video"
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
        let args: ViewVideoArgs =
            serde_json::from_value(args).map_err(|e| rua_core::RuaError::Tool {
                tool: "view_video".into(),
                message: e.to_string(),
            })?;

        if is_url(&args.video_path) {
            let url_path = args
                .video_path
                .split('?')
                .next()
                .unwrap_or(&args.video_path);
            let p = Path::new(url_path);
            if p.extension().is_some() && !is_video_extension(p) {
                return Ok(ToolResult::error(
                    "view_video",
                    format!(
                        "URL does not point to a supported video format: {}",
                        args.video_path
                    ),
                ));
            }
            return Ok(ToolResult {
                call_id: "view_video".into(),
                name: None,
                content: vec![
                    ContentPart::Video {
                        url: args.video_path.clone(),
                        media_type: None,
                    },
                    ContentPart::Text {
                        text: format!("Video loaded from URL: {}", args.video_path),
                    },
                ],
                is_error: false,
                duration_ms: None,
                attachments: None,
                truncation: None,
                metadata: serde_json::Map::new(),
            });
        }

        let expanded = expand_tilde(&args.video_path);
        let path = ctx.working_dir.join(expanded);

        if !path.exists() {
            return Ok(ToolResult::error(
                "view_video",
                format!("File not found: {}", args.video_path),
            ));
        }

        if path.is_dir() {
            return Ok(ToolResult::error(
                "view_video",
                format!("'{}' is a directory, not a video file", args.video_path),
            ));
        }

        if !is_video_extension(&path) {
            return Ok(ToolResult::error(
                "view_video",
                format!(
                    "File '{}' does not appear to be a supported video format. \
                     Supported: {}",
                    args.video_path,
                    VIDEO_EXTENSIONS.join(", ")
                ),
            ));
        }

        let metadata = std::fs::metadata(&path).map_err(|e| rua_core::RuaError::Tool {
            tool: "view_video".into(),
            message: format!("Failed to read file metadata: {e}"),
        })?;

        let file_size = metadata.len();

        if file_size > MAX_VIDEO_SIZE {
            return Ok(ToolResult::error(
                "view_video",
                format!(
                    "Video file is too large ({:.1} MB). Maximum supported size is {:.0} MB.",
                    file_size as f64 / (1024.0 * 1024.0),
                    MAX_VIDEO_SIZE as f64 / (1024.0 * 1024.0),
                ),
            ));
        }

        let mime = mime_from_extension(&path);
        let file_name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let video_bytes = std::fs::read(&path).map_err(|e| rua_core::RuaError::Tool {
            tool: "view_video".into(),
            message: format!("Failed to read video file: {e}"),
        })?;

        let b64 = base64_encode(&video_bytes);
        let data_url = format!("data:{mime};base64,{b64}");

        Ok(ToolResult {
            call_id: "view_video".into(),
            name: None,
            content: vec![
                ContentPart::Video {
                    url: data_url,
                    media_type: Some(mime),
                },
                ContentPart::Text {
                    text: format!("Video loaded: {file_name}"),
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
