use std::path::{Component, Path, PathBuf};

use llm_multiplex::ModelInfo;

use crate::error::AgentError;

pub(crate) fn resolve_projected_media(
    messages: &mut [agent_types::ModelDialogueItem],
    storage_root: &Path,
) {
    for message in messages {
        for part in &mut message.content {
            let url = match part {
                agent_types::ContentPart::Image { url, .. }
                | agent_types::ContentPart::Audio { url, .. }
                | agent_types::ContentPart::Video { url, .. }
                | agent_types::ContentPart::File { url, .. } => url,
                _ => continue,
            };
            if agent_types::content::is_managed_media_ref(url) {
                *url = storage_root
                    .join(url.as_str())
                    .to_string_lossy()
                    .into_owned();
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaType {
    Text,

    ImageUrl,

    ImageLocal,

    Audio,

    Video,

    Unknown,
}

impl MediaType {
    pub fn from_mime(mime: &str) -> Self {
        let lower = mime.to_lowercase();
        if lower.starts_with("text/") {
            Self::Text
        } else if lower.starts_with("image/") {
            Self::ImageUrl
        } else if lower.starts_with("audio/") {
            Self::Audio
        } else if lower.starts_with("video/") {
            Self::Video
        } else {
            Self::Unknown
        }
    }

    pub fn from_extension(ext: &str) -> Self {
        match ext.to_lowercase().as_str() {
            "txt" | "md" | "json" | "yaml" | "yml" | "toml" | "rs" | "py" | "js" | "ts"
            | "html" | "css" | "xml" | "csv" | "log" => Self::Text,
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "svg" | "bmp" | "ico" => Self::ImageLocal,
            "mp3" | "wav" | "ogg" | "flac" | "m4a" | "aac" => Self::Audio,
            "mp4" | "avi" | "mov" | "webm" | "mkv" => Self::Video,
            _ => Self::Unknown,
        }
    }

    pub fn is_supported(&self) -> bool {
        matches!(self, Self::Text | Self::ImageUrl | Self::ImageLocal)
    }
}

#[derive(Debug, Clone)]
pub struct MediaLimits {
    pub max_base64_size: usize,

    pub max_local_file_size: u64,

    pub audio_enabled: bool,

    pub video_enabled: bool,
}

impl Default for MediaLimits {
    fn default() -> Self {
        Self {
            max_base64_size: 10 * 1024 * 1024,
            max_local_file_size: 50 * 1024 * 1024,
            audio_enabled: false,
            video_enabled: false,
        }
    }
}

pub struct MediaProcessor {
    limits: MediaLimits,
}

impl MediaProcessor {
    pub fn new() -> Self {
        Self {
            limits: MediaLimits::default(),
        }
    }

    pub fn with_limits(limits: MediaLimits) -> Self {
        Self { limits }
    }

    pub fn validate_workspace_path(
        &self,
        file_path: &Path,
        workspace: &Path,
    ) -> Result<PathBuf, AgentError> {
        let workspace = normalize_path(workspace);

        let candidate = if file_path.is_absolute() {
            file_path.to_path_buf()
        } else {
            workspace.join(file_path)
        };
        let canonical = normalize_path(&candidate);

        if !canonical.starts_with(&workspace) {
            return Err(AgentError::media(format!(
                "File path '{}' is outside workspace '{}'",
                file_path.display(),
                workspace.display()
            )));
        }

        Ok(canonical)
    }

    pub fn validate_base64_size(&self, base64_data: &str) -> Result<(), AgentError> {
        let estimated_size = base64_data.len() * 3 / 4;
        if estimated_size > self.limits.max_base64_size {
            return Err(AgentError::media(format!(
                "Base64 data too large: ~{} bytes (limit: {} bytes)",
                estimated_size, self.limits.max_base64_size
            )));
        }
        Ok(())
    }

    pub fn validate_media_type(&self, media_type: MediaType) -> Result<(), AgentError> {
        match media_type {
            MediaType::Text | MediaType::ImageUrl | MediaType::ImageLocal => Ok(()),
            MediaType::Audio if self.limits.audio_enabled => Ok(()),
            MediaType::Video if self.limits.video_enabled => Ok(()),
            MediaType::Audio => Err(AgentError::media(
                "Audio processing is not enabled. Enable it in the agent configuration.",
            )),
            MediaType::Video => Err(AgentError::media(
                "Video processing is not supported in P0. Use an external tool instead.",
            )),
            MediaType::Unknown => Err(AgentError::media(
                "Unknown media type. Supported types: text, image, audio (if enabled).",
            )),
        }
    }

    pub fn validate_image_url(&self, url: &str) -> Result<(), AgentError> {
        if url.is_empty() {
            return Err(AgentError::media("Image URL cannot be empty"));
        }
        if let Some(rest) = url.strip_prefix("data:") {
            let media_type = rest.split_once(';').map_or(rest, |(mime, _)| mime);
            if media_type.to_lowercase().starts_with("image/") {
                return Ok(());
            }
            return Err(AgentError::media(format!(
                "Invalid image data URL media type: {media_type}"
            )));
        }

        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(AgentError::media(format!(
                "Invalid image URL scheme: {url}"
            )));
        }
        Ok(())
    }

    pub fn limits(&self) -> &MediaLimits {
        &self.limits
    }
}

impl Default for MediaProcessor {
    fn default() -> Self {
        Self::new()
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

#[derive(Debug, Clone)]
pub struct FileBlockSupportFormatter {
    provider_id: String,
    supports_image: bool,
    supports_video: bool,
}

impl FileBlockSupportFormatter {
    pub fn for_provider(provider_id: &str, model_info: &ModelInfo) -> Self {
        Self {
            provider_id: provider_id.to_string(),
            supports_image: model_info.supports_image.unwrap_or(false),
            supports_video: model_info.supports_video.unwrap_or(false),
        }
    }

    pub fn text_only(provider_id: &str) -> Self {
        Self {
            provider_id: provider_id.to_string(),
            supports_image: false,
            supports_video: false,
        }
    }

    pub fn provider_id(&self) -> &str {
        &self.provider_id
    }

    pub fn supports_image(&self) -> bool {
        self.supports_image
    }

    pub fn supports_video(&self) -> bool {
        self.supports_video
    }

    pub fn is_supported(&self, media_type: MediaType) -> bool {
        match media_type {
            MediaType::Text => true,
            MediaType::ImageUrl | MediaType::ImageLocal => self.supports_image,
            MediaType::Video => self.supports_video,
            MediaType::Audio => false,
            MediaType::Unknown => false,
        }
    }

    pub fn validate(&self, media_type: MediaType) -> Result<(), AgentError> {
        if self.is_supported(media_type) {
            Ok(())
        } else {
            Err(AgentError::media(format!(
                "Provider '{}' does not support {:?} media",
                self.provider_id, media_type,
            )))
        }
    }
}
