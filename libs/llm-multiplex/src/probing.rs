use std::time::Duration;

use tracing::{debug, warn};

use agent_types::{ChatRequest, ContentPart, ModelDialogueItem, Role};

use crate::error::LlmError;
use crate::traits::{ChatModel, ModelInfo};

const RED_PNG_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAACAAAAAgCAIAAAD8GO2jAAAAJ0lEQVR42u3NsQkAAAjAsP7/tF7hIASyp6lTCQQCgUAgEAgEgi/BAjLD/C5w/SM9AAAAAElFTkSuQmCC";

const BLUE_PNG_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAACAAAAAgCAIAAAD8GO2jAAAAJklEQVR4nO3NsQkAAAjAsP7/tF7hIASyp5pjAoFAIBAIBAKB4EmwOkv8Lm7+zY4AAAAASUVORK5CYII=";

const RED_KEYWORDS: &[&str] = &["red", "红", "红色", "crimson", "scarlet"];
const BLUE_KEYWORDS: &[&str] = &[
    "blue", "蓝", "蓝色", "navy", "azure", "cobalt", "cyan", "indigo",
];

#[derive(Debug, Clone)]
pub struct ProbeConfig {
    pub timeout: Duration,

    pub max_tokens: u32,
}

impl Default for ProbeConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(15),
            max_tokens: 200,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProbeResult {
    pub supports_image: bool,
    pub supports_video: bool,
    pub supports_multimodal: bool,
    pub image_message: String,
    #[serde(default)]
    pub stage1_passed: bool,
    #[serde(default)]
    pub stage2_passed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub struct MultimodalProber {
    config: ProbeConfig,
}

impl MultimodalProber {
    pub fn new(config: ProbeConfig) -> Self {
        Self { config }
    }

    pub fn with_defaults() -> Self {
        Self::new(ProbeConfig::default())
    }

    pub async fn probe(&self, model: &dyn ChatModel) -> ProbeResult {
        let stage1 = self.probe_single_image(model, RED_PNG_BASE64, "red").await;
        let (stage1_passed, stage1_err) = match stage1 {
            Ok(passed) => (passed, None),
            Err(e) => {
                debug!(error = %e, "probe stage 1 failed");
                (false, Some(e.to_string()))
            }
        };

        if !stage1_passed {
            let image_message = if let Some(ref err) = stage1_err {
                err.clone()
            } else {
                "Model replied NO_MEDIA to the image probe".into()
            };
            return ProbeResult {
                supports_image: false,
                supports_video: false,
                supports_multimodal: false,
                image_message,
                stage1_passed: false,
                stage2_passed: false,
                error: stage1_err,
            };
        }

        let stage2 = self
            .probe_single_image(model, BLUE_PNG_BASE64, "blue")
            .await;
        let (stage2_passed, stage2_err) = match stage2 {
            Ok(passed) => (passed, None),
            Err(e) => {
                debug!(error = %e, "probe stage 2 failed");
                (false, Some(e.to_string()))
            }
        };

        let supports_image = stage1_passed && stage2_passed;
        let image_message = if let Some(ref err) = stage2_err {
            err.clone()
        } else if supports_image {
            "Multimodal supported".into()
        } else {
            "Model replied NO_MEDIA to the image probe".into()
        };

        ProbeResult {
            supports_image,
            supports_video: false,
            supports_multimodal: supports_image,
            image_message,
            stage1_passed,
            stage2_passed,
            error: stage2_err,
        }
    }

    pub async fn probe_and_update(&self, model: &dyn ChatModel, info: &mut ModelInfo) {
        let result = self.probe(model).await;
        if result.error.is_some() {
            return;
        }
        info.supports_image = Some(result.supports_image);
        info.supports_video = Some(result.supports_video);
        info.supports_multimodal = Some(result.supports_multimodal);
        if result.supports_image || result.supports_video {
            info.probe_source = Some("probed".into());
        } else {
            info.probe_source = Some("probed-no-multimodal".into());
        }
    }

    async fn probe_single_image(
        &self,
        model: &dyn ChatModel,
        image_base64: &str,
        expected_color: &str,
    ) -> Result<bool, LlmError> {
        let data_url = format!("data:image/png;base64,{image_base64}");

        let message = ModelDialogueItem {
            id: None,
            role: Role::User,
            content: vec![
                ContentPart::Image {
                    url: data_url,
                    media_type: Some("image/png".into()),
                    alt: None,
                },
                ContentPart::Text {
                    text: "What is the dominant color of this image? Reply with ONLY one English color word. If no image is visible to you, reply with exactly: NO_MEDIA".into(),
                },
            ],
            name: None,
            created_at: None,
            metadata: serde_json::Map::new(),
        };

        let request = ChatRequest::new(vec![message]).with_max_tokens(self.config.max_tokens);

        let response = tokio::time::timeout(self.config.timeout, model.chat(request)).await;

        match response {
            Ok(Ok(resp)) => {
                let text = resp.message.text_content().to_lowercase();
                let thinking = resp.message.thinking_content().to_lowercase();
                let combined = format!("{text} {thinking}");
                let answer = if text.trim().is_empty() {
                    &thinking
                } else {
                    &text
                };
                let has_no_media = answer.trim().eq_ignore_ascii_case("NO_MEDIA");
                let keywords = match expected_color {
                    "red" => RED_KEYWORDS,
                    "blue" => BLUE_KEYWORDS,
                    _ => &[expected_color],
                };
                let passed = !has_no_media && keywords.iter().any(|kw| combined.contains(kw));
                debug!(
                    expected = expected_color,
                    response = %text,
                    has_thinking = !thinking.is_empty(),
                    has_no_media,
                    passed,
                    "probe stage complete"
                );
                if passed || has_no_media {
                    Ok(passed)
                } else {
                    Err(LlmError::InvalidResponse {
                        provider: "probe".into(),
                        message: format!(
                            "Image probe inconclusive: expected {expected_color} or NO_MEDIA, finish reason: {}",
                            resp.finish_reason
                        ),
                    })
                }
            }
            Ok(Err(e)) => Err(e),
            Err(_elapsed) => {
                warn!(timeout = ?self.config.timeout, "probe timed out");
                Err(LlmError::Timeout {
                    provider: "probe".into(),
                    timeout: self.config.timeout,
                })
            }
        }
    }
}

pub async fn probe_model_multimodal(model: &dyn ChatModel) -> ProbeResult {
    MultimodalProber::with_defaults().probe(model).await
}
