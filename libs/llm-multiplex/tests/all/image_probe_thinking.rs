use std::sync::atomic::{AtomicUsize, Ordering};

use agent_types::{
    ChatRequest, ChatResponse, ContentPart, FinishReason, ModelDialogueItem, Role, TokenUsage,
};
use async_trait::async_trait;
use llm_multiplex::error::LlmError;
use llm_multiplex::probing::{MultimodalProber, ProbeConfig};
use llm_multiplex::traits::{ChatModel, EventStream, ModelInfo};

struct ThinkingStubModel {
    responses: Vec<Vec<ContentPart>>,
    call_idx: AtomicUsize,
    fail_request: bool,
}

impl ThinkingStubModel {
    fn new(responses: Vec<Vec<ContentPart>>) -> Self {
        Self {
            responses,
            call_idx: AtomicUsize::new(0),
            fail_request: false,
        }
    }
}

#[async_trait]
impl ChatModel for ThinkingStubModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        if self.fail_request {
            return Err(LlmError::Unknown {
                provider: "stub".into(),
                message: "upstream rejected image request".into(),
            });
        }
        let idx = self.call_idx.fetch_add(1, Ordering::SeqCst);
        let content = self.responses.get(idx).cloned().unwrap_or_else(|| {
            vec![ContentPart::Text {
                text: "unknown".into(),
            }]
        });
        Ok(ChatResponse {
            message: ModelDialogueItem {
                id: None,
                role: Role::Assistant,
                content,
                name: None,
                created_at: None,
                metadata: serde_json::Map::new(),
            },
            usage: Some(TokenUsage::new(10, 5)),
            model: Some("stub-thinking".into()),
            finish_reason: FinishReason::Stop,
            metadata: serde_json::Map::new(),
            extra: Default::default(),
        })
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        unimplemented!()
    }

    fn model_info(&self) -> &ModelInfo {
        unimplemented!()
    }
}

fn probe_config() -> ProbeConfig {
    ProbeConfig {
        timeout: std::time::Duration::from_secs(5),
        max_tokens: 200,
    }
}

#[tokio::test]
async fn probe_stops_after_two_image_checks() {
    let model = ThinkingStubModel::new(vec![
        vec![ContentPart::Text { text: "red".into() }],
        vec![ContentPart::Text {
            text: "blue".into(),
        }],
        vec![ContentPart::Text {
            text: "unexpected third call".into(),
        }],
    ]);

    let result = MultimodalProber::with_defaults().probe(&model).await;

    assert!(result.supports_image);
    assert!(!result.supports_video);
    assert_eq!(model.call_idx.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn thinking_only_red_passes_stage1() {
    let model = ThinkingStubModel::new(vec![
        vec![ContentPart::Thinking {
            text: "The image is clearly red".into(),
            signature: None,
        }],
        vec![ContentPart::Text {
            text: "blue".into(),
        }],
    ]);
    let prober = MultimodalProber::new(probe_config());
    let result = prober.probe(&model).await;

    assert!(result.stage1_passed);
    assert!(result.stage2_passed);
    assert!(result.supports_image);
}

#[tokio::test]
async fn chinese_red_keyword_passes() {
    let model = ThinkingStubModel::new(vec![
        vec![ContentPart::Text {
            text: "红色".into(),
        }],
        vec![ContentPart::Text {
            text: "blue".into(),
        }],
    ]);
    let prober = MultimodalProber::new(probe_config());
    let result = prober.probe(&model).await;

    assert!(result.stage1_passed);
    assert!(result.stage2_passed);
    assert!(result.supports_image);
}

#[tokio::test]
async fn chinese_blue_keyword_passes() {
    let model = ThinkingStubModel::new(vec![
        vec![ContentPart::Text { text: "red".into() }],
        vec![ContentPart::Text {
            text: "蓝色".into(),
        }],
    ]);
    let prober = MultimodalProber::new(probe_config());
    let result = prober.probe(&model).await;

    assert!(result.stage1_passed);
    assert!(result.stage2_passed);
    assert!(result.supports_image);
}

#[tokio::test]
async fn thinking_block_with_crimson_passes() {
    let model = ThinkingStubModel::new(vec![
        vec![
            ContentPart::Thinking {
                text: "This is a crimson colored square".into(),
                signature: None,
            },
            ContentPart::Text { text: "".into() },
        ],
        vec![
            ContentPart::Thinking {
                text: "I see an azure colored image".into(),
                signature: None,
            },
            ContentPart::Text { text: "".into() },
        ],
    ]);
    let prober = MultimodalProber::new(probe_config());
    let result = prober.probe(&model).await;

    assert!(result.stage1_passed);
    assert!(result.stage2_passed);
    assert!(result.supports_image);
}

#[tokio::test]
async fn default_max_tokens_is_200() {
    let config = ProbeConfig::default();
    assert_eq!(config.max_tokens, 200);
}

#[tokio::test]
async fn upstream_error_is_not_an_unsupported_result() {
    let mut model = ThinkingStubModel::new(vec![]);
    model.fail_request = true;
    let result = MultimodalProber::with_defaults().probe(&model).await;
    assert!(result
        .error
        .unwrap()
        .contains("upstream rejected image request"));
}

#[tokio::test]
async fn inconclusive_answer_is_an_error() {
    let model = ThinkingStubModel::new(vec![vec![ContentPart::Text {
        text: "I cannot determine the color".into(),
    }]]);
    let result = MultimodalProber::with_defaults().probe(&model).await;
    assert!(result.error.unwrap().contains("inconclusive"));
}

#[tokio::test]
async fn failed_probe_preserves_existing_capabilities() {
    let mut model = ThinkingStubModel::new(vec![]);
    model.fail_request = true;
    let mut info: ModelInfo = serde_json::from_value(serde_json::json!({
        "id": "stub", "name": "stub"
    }))
    .unwrap();
    info.supports_image = Some(true);
    info.supports_video = Some(true);
    info.supports_multimodal = Some(true);
    info.probe_source = Some("configured".into());
    MultimodalProber::with_defaults()
        .probe_and_update(&model, &mut info)
        .await;
    assert_eq!(info.supports_image, Some(true));
    assert_eq!(info.supports_video, Some(true));
    assert_eq!(info.supports_multimodal, Some(true));
    assert_eq!(info.probe_source.as_deref(), Some("configured"));
}

#[tokio::test]
async fn thinking_mentions_no_media_but_correct_answer_passes() {
    let model = ThinkingStubModel::new(vec![
        vec![
            ContentPart::Thinking {
                text: "If there were no image I would reply NO_MEDIA, but this is red".into(),
                signature: None,
            },
            ContentPart::Text { text: "red".into() },
        ],
        vec![ContentPart::Text {
            text: "blue".into(),
        }],
    ]);
    let result = MultimodalProber::with_defaults().probe(&model).await;
    assert!(result.supports_image);
    assert!(result.error.is_none());
}

#[tokio::test]
async fn explicit_no_media_is_a_negative_result() {
    let model = ThinkingStubModel::new(vec![vec![ContentPart::Text {
        text: "NO_MEDIA".into(),
    }]]);
    let result = MultimodalProber::with_defaults().probe(&model).await;
    assert!(!result.supports_image);
    assert!(result.error.is_none());
}

#[tokio::test]
async fn second_image_inconclusive_is_an_error() {
    let model = ThinkingStubModel::new(vec![
        vec![ContentPart::Text { text: "red".into() }],
        vec![ContentPart::Text {
            text: String::new(),
        }],
    ]);
    let result = MultimodalProber::with_defaults().probe(&model).await;
    assert!(result.stage1_passed);
    assert!(!result.stage2_passed);
    assert!(result.error.unwrap().contains("expected blue"));
}
