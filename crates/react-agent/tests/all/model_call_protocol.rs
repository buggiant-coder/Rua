use std::pin::Pin;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_types::{AgentEvent, ChatRequest, ChatResponse, FinishReason, ModelDialogueItem, Role};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use llm_multiplex::traits::{ChatModel, EventStream, ModelInfo};
use llm_multiplex::LlmError;
use react_agent::hooks::{NoopHook, RecordingHook};
use react_agent::toolkit::AgentToolkit;
use react_agent::{
    AgentContext, AgentError, ModelCallContext, ModelCallSection, ModelCallSectionKind,
    ModelInputTokenEstimate, ModelInputTokenEstimateMethod, ReactAgent, ReactConfig,
    MODEL_CALL_CONTEXT_SECTION_METADATA_KEY, MODEL_INPUT_TOKEN_ESTIMATE_METADATA_KEY,
};

fn model_info(id: &str) -> ModelInfo {
    ModelInfo {
        id: id.into(),
        name: id.into(),
        context_window: None,
        max_output_tokens: None,
        supports_multimodal: None,
        supports_image: None,
        supports_video: None,
        probe_source: None,
        generate_kwargs: serde_json::Map::new(),
    }
}

struct EndThenPendingModel {
    info: ModelInfo,
}

#[async_trait]
impl ChatModel for EndThenPendingModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        unreachable!("stream-only test model")
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        let stream = futures::stream::iter(vec![Ok(AgentEvent::End {
            finish_reason: FinishReason::Stop,
            usage: None,
            metadata: serde_json::Value::Null,
        })])
        .chain(futures::stream::pending());
        let stream: Pin<Box<dyn Stream<Item = Result<AgentEvent, LlmError>> + Send>> =
            Box::pin(stream);
        stream
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

struct EofBeforeEndModel {
    info: ModelInfo,
}

#[async_trait]
impl ChatModel for EofBeforeEndModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        unreachable!("stream-only test model")
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        Box::pin(futures::stream::iter(vec![Ok(AgentEvent::TextDelta {
            delta: "partial".into(),
            metadata: serde_json::Value::Null,
        })]))
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

struct PendingModel {
    info: ModelInfo,
}

struct TokenErrorModel {
    info: ModelInfo,
    calls: AtomicU32,
}

#[async_trait]
impl ChatModel for TokenErrorModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        unreachable!("stream-only test model")
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(futures::stream::iter(vec![Ok(AgentEvent::Error {
            code: Some("rate_limit_exceeded".into()),
            message: "maximum token quota exceeded".into(),
            retryable: false,
            metadata: serde_json::Value::Null,
        })]))
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

struct TimeoutThenSuccessModel {
    info: ModelInfo,
    calls: Arc<AtomicU32>,
    failures: u32,
    partial_before_timeout: bool,
}

struct PartialThenConnectionErrorModel {
    info: ModelInfo,
    calls: AtomicU32,
}

struct PartialThenContextOverflowModel {
    info: ModelInfo,
    calls: AtomicU32,
}

struct ConnectionErrorBeforeContentModel {
    info: ModelInfo,
    calls: AtomicU32,
}

#[derive(Default)]
struct RecoveringOverflowProjector {
    recoveries: AtomicU32,
}

#[async_trait]
impl react_agent::ModelCallProjector for RecoveringOverflowProjector {
    async fn project(
        &self,
        request: ChatRequest,
        _event_tx: &tokio::sync::mpsc::Sender<AgentEvent>,
    ) -> Result<ChatRequest, AgentError> {
        Ok(request)
    }

    async fn recover_context_overflow(
        &self,
        messages: &[ModelDialogueItem],
        _error_message: &str,
    ) -> Result<Vec<ModelDialogueItem>, AgentError> {
        self.recoveries.fetch_add(1, Ordering::SeqCst);
        Ok(messages.to_vec())
    }
}

#[async_trait]
impl ChatModel for ConnectionErrorBeforeContentModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        unreachable!("stream-only test model")
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(futures::stream::iter(vec![Err(LlmError::Connection {
            provider: "test".into(),
            message: "connection failed before content".into(),
        })]))
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

#[async_trait]
impl ChatModel for PartialThenConnectionErrorModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        unreachable!("stream-only test model")
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            Box::pin(futures::stream::iter(vec![
                Ok(AgentEvent::TextDelta {
                    delta: "partial".into(),
                    metadata: serde_json::Value::Null,
                }),
                Err(LlmError::Connection {
                    provider: "test".into(),
                    message: "stream interrupted".into(),
                }),
            ]))
        } else {
            Box::pin(futures::stream::iter(vec![
                Ok(AgentEvent::TextDelta {
                    delta: "complete".into(),
                    metadata: serde_json::Value::Null,
                }),
                Ok(AgentEvent::End {
                    finish_reason: FinishReason::Stop,
                    usage: None,
                    metadata: serde_json::Value::Null,
                }),
            ]))
        }
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

#[async_trait]
impl ChatModel for PartialThenContextOverflowModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        unreachable!("stream-only test model")
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            Box::pin(futures::stream::iter(vec![
                Ok(AgentEvent::TextDelta {
                    delta: "partial".into(),
                    metadata: serde_json::Value::Null,
                }),
                Ok(AgentEvent::Error {
                    code: Some("context_length_exceeded".into()),
                    message: "maximum context length exceeded".into(),
                    retryable: false,
                    metadata: serde_json::Value::Null,
                }),
            ]))
        } else {
            Box::pin(futures::stream::iter(vec![
                Ok(AgentEvent::TextDelta {
                    delta: "complete".into(),
                    metadata: serde_json::Value::Null,
                }),
                Ok(AgentEvent::End {
                    finish_reason: FinishReason::Stop,
                    usage: None,
                    metadata: serde_json::Value::Null,
                }),
            ]))
        }
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

#[async_trait]
impl ChatModel for PendingModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        unreachable!("stream-only test model")
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        Box::pin(futures::stream::pending())
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

#[async_trait]
impl ChatModel for TimeoutThenSuccessModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        unreachable!("stream-only test model")
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call < self.failures {
            let partial = self.partial_before_timeout;
            Box::pin(async_stream::stream! {
                if partial {
                    yield Ok(AgentEvent::TextDelta {
                        delta: "partial".into(),
                        metadata: serde_json::Value::Null,
                    });
                }
                std::future::pending::<()>().await;
            })
        } else {
            Box::pin(futures::stream::iter(vec![
                Ok(AgentEvent::TextDelta {
                    delta: "recovered".into(),
                    metadata: serde_json::Value::Null,
                }),
                Ok(AgentEvent::End {
                    finish_reason: FinishReason::Stop,
                    usage: None,
                    metadata: serde_json::Value::Null,
                }),
            ]))
        }
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

struct DelayedServerErrorModel {
    info: ModelInfo,
    calls: Arc<AtomicU32>,
    failures: u32,
}

#[async_trait]
impl ChatModel for DelayedServerErrorModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        unreachable!("stream-only test model")
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let fail = call < self.failures;
        Box::pin(async_stream::stream! {
            yield Ok(AgentEvent::Start {
                run_id: format!("attempt-{call}"),
                model: Some("delayed-server".into()),
                metadata: serde_json::Value::Null,
            });
            tokio::time::sleep(Duration::from_millis(700)).await;
            if fail {
                yield Err(LlmError::Server {
                    provider: "delayed-server".into(),
                    status: Some(502),
                    message: "upstream unavailable".into(),
                });
            } else {
                yield Ok(AgentEvent::TextDelta {
                    delta: "recovered".into(),
                    metadata: serde_json::Value::Null,
                });
                yield Ok(AgentEvent::End {
                    finish_reason: FinishReason::Stop,
                    usage: None,
                    metadata: serde_json::Value::Null,
                });
            }
        })
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

fn retry_policy(max_retries: u32) -> rua_provider::RetryPolicy {
    rua_provider::RetryPolicy {
        max_retries,
        initial_delay: Duration::from_secs(2),
        max_delay: Duration::from_secs(2),
        backoff_factor: 1.0,
        max_total_time: None,
    }
}

fn delayed_retried_model(failures: u32, max_retries: u32) -> (Arc<dyn ChatModel>, Arc<AtomicU32>) {
    let calls = Arc::new(AtomicU32::new(0));
    let model = rua_provider::ModelFactory::new()
        .with_retry_policy(retry_policy(max_retries))
        .build(
            Box::new(DelayedServerErrorModel {
                info: model_info("delayed-server"),
                calls: calls.clone(),
                failures,
            }),
            "delayed-server",
        );
    (Arc::from(model), calls)
}

#[tokio::test(start_paused = true)]
async fn provider_retry_gets_full_model_timeout_after_backoff() {
    let (model, calls) = delayed_retried_model(1, 1);
    let ctx = context(
        model,
        ReactConfig {
            model_timeout_secs: 1,
            ..Default::default()
        },
    );
    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    let counter = AtomicU32::new(0);
    let result = react_agent::react_loop::run_react_loop(
        &ctx,
        vec![ModelDialogueItem::text(Role::User, "hi")],
        tx,
        &counter,
    )
    .await;
    assert!(
        result.is_ok(),
        "the second HTTP attempt should recover: {result:?}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::ModelCallReset { .. })),
        "provider backoff must not be interrupted by the outer model timeout: {events:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn provider_retry_exhaustion_preserves_last_error_without_restarting_attempts() {
    let (model, calls) = delayed_retried_model(u32::MAX, 2);
    let ctx = context(
        model,
        ReactConfig {
            model_timeout_secs: 1,
            ..Default::default()
        },
    );
    let (tx, _rx) = tokio::sync::mpsc::channel(32);
    let counter = AtomicU32::new(0);
    let result = react_agent::react_loop::run_react_loop(
        &ctx,
        vec![ModelDialogueItem::text(Role::User, "hi")],
        tx,
        &counter,
    )
    .await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "one initial attempt and two configured retries"
    );
    assert!(
        matches!(
            result,
            Err(AgentError::Model(LlmError::Server {
                status: Some(502),
                ..
            }))
        ),
        "the actual upstream failure must survive exhaustion: {result:?}"
    );
}

struct ProductiveSlowModel {
    info: ModelInfo,
}

struct ModeMarkerModel {
    info: ModelInfo,
}

#[async_trait]
impl ChatModel for ModeMarkerModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        unreachable!("stream-only test model")
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        Box::pin(futures::stream::iter(vec![
            Ok(AgentEvent::TextDelta {
                delta: "[MODE:Plan] ordinary assistant text".into(),
                metadata: serde_json::Value::Null,
            }),
            Ok(AgentEvent::End {
                finish_reason: FinishReason::Stop,
                usage: None,
                metadata: serde_json::Value::Null,
            }),
        ]))
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

#[async_trait]
impl ChatModel for ProductiveSlowModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        unreachable!("stream-only test model")
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        Box::pin(async_stream::stream! {
            tokio::time::sleep(Duration::from_millis(700)).await;
            yield Ok(AgentEvent::TextDelta {
                delta: "still working".into(),
                metadata: serde_json::Value::Null,
            });
            tokio::time::sleep(Duration::from_millis(700)).await;
            yield Ok(AgentEvent::End {
                finish_reason: FinishReason::Stop,
                usage: None,
                metadata: serde_json::Value::Null,
            });
        })
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

fn context(model: Arc<dyn ChatModel>, config: ReactConfig) -> AgentContext {
    AgentContext::new(
        model,
        Arc::new(AgentToolkit::new()),
        config,
        Arc::new(NoopHook),
        rua_core::tool::ToolContext::default(),
    )
}

#[tokio::test]
async fn typed_model_call_sections_have_fixed_roles_and_order() {
    let ctx = context(
        Arc::new(EofBeforeEndModel {
            info: model_info("typed-sections"),
        }),
        ReactConfig::default(),
    )
    .with_model_call_sections(vec![
        ModelCallSection::new(ModelCallSectionKind::SkillCatalog, "skills"),
        ModelCallSection::new(ModelCallSectionKind::ProjectInstructions, "project"),
        ModelCallSection::new(ModelCallSectionKind::AgentPromptFiles, "prompt files"),
        ModelCallSection::new(ModelCallSectionKind::ExecutionEnvironment, "environment"),
        ModelCallSection::new(ModelCallSectionKind::AgentPromptContext, "agent prompt"),
    ]);
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
    let request = ModelCallContext::capture(
        &ctx,
        vec![ModelDialogueItem::text(Role::User, "current input")],
        0,
        false,
    )
    .project(&ctx, &event_tx)
    .await
    .expect("project typed sections");

    let sections = request
        .messages
        .iter()
        .filter_map(|message| {
            message
                .metadata
                .get(MODEL_CALL_CONTEXT_SECTION_METADATA_KEY)
                .and_then(serde_json::Value::as_str)
                .map(|section| (section, message.role, message.text_content()))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        sections,
        vec![
            (
                "agent_prompt_files",
                Role::Developer,
                "prompt files".to_string(),
            ),
            (
                "agent_prompt_context",
                Role::Developer,
                "agent prompt".to_string(),
            ),
            (
                "project_instructions",
                Role::Developer,
                "project".to_string(),
            ),
            (
                "execution_environment",
                Role::Developer,
                "environment".to_string(),
            ),
            ("skill_catalog", Role::Developer, "skills".to_string()),
        ]
    );
    assert_eq!(
        request.messages.last().map(ModelDialogueItem::text_content),
        Some("current input".to_string())
    );
}

#[tokio::test]
async fn model_call_token_diagnostics_ignore_message_metadata() {
    let ctx = context(
        Arc::new(EofBeforeEndModel {
            info: model_info("metadata-token-estimate"),
        }),
        ReactConfig::default(),
    );
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
    let plain = ModelDialogueItem::text(Role::User, "same model-visible input");
    let mut with_internal_metadata = plain.clone();
    with_internal_metadata.metadata.insert(
        "turn_agent_assignment".to_string(),
        serde_json::Value::String("x".repeat(20_000)),
    );

    let plain_request = ModelCallContext::capture(&ctx, vec![plain], 0, false)
        .project(&ctx, &event_tx)
        .await
        .expect("project plain request");
    let metadata_request = ModelCallContext::capture(&ctx, vec![with_internal_metadata], 0, false)
        .project(&ctx, &event_tx)
        .await
        .expect("project metadata request");

    let plain_diagnostics = plain_request
        .metadata
        .get("rua_model_call_context")
        .expect("plain Model Call Context diagnostic");
    let metadata_diagnostics = metadata_request
        .metadata
        .get("rua_model_call_context")
        .expect("metadata Model Call Context diagnostic");
    assert_eq!(
        plain_diagnostics["input_estimate_tokens"],
        metadata_diagnostics["input_estimate_tokens"]
    );
    assert_eq!(
        plain_diagnostics["messages"][0]["estimated_tokens"],
        metadata_diagnostics["messages"][0]["estimated_tokens"]
    );
}

#[test]
fn assistant_message_retains_the_model_input_token_estimate() {
    let estimate = ModelInputTokenEstimate {
        provider_id: Some("test-provider".to_string()),
        model: Some("test-model".to_string()),
        fast_estimate_divisor: 4.0,
        fast_estimate_tokens: 800,
        calibrated_estimate_tokens: 1_000,
        conversation_estimate_tokens: 500,
        method: ModelInputTokenEstimateMethod::ProviderDelta,
        calibration_source_provider_id: Some("previous-provider".to_string()),
        calibration_source_model: Some("previous-model".to_string()),
        previous_fast_estimate_tokens: Some(700),
        previous_provider_prompt_tokens: Some(900),
    };
    let result = react_agent::react_loop::IterationResult {
        model_call_id: "call-1".to_string(),
        input_token_estimate: Some(estimate.clone()),
        text: "done".to_string(),
        text_messages: Vec::new(),
        thinking: String::new(),
        thinking_signature: None,
        tool_calls: Vec::new(),
        finish_reason: FinishReason::Stop,
        usage: None,
        response_metadata: serde_json::json!({
            "rua_responses_session": {
                "response_id": "response-1"
            }
        }),
        events: Vec::new(),
        parse_errors: Vec::new(),
    };

    let assistant = react_agent::react_loop::build_assistant_message(&result);
    let persisted: ModelInputTokenEstimate =
        serde_json::from_value(assistant.metadata[MODEL_INPUT_TOKEN_ESTIMATE_METADATA_KEY].clone())
            .expect("assistant estimate metadata");

    assert_eq!(persisted, estimate);
    assert_eq!(
        assistant.metadata["rua_responses_session"]["response_id"],
        "response-1"
    );
}

#[test]
fn default_config_does_not_impose_wall_clock_deadlines() {
    let config = ReactConfig::default();

    assert_eq!(config.total_timeout_secs, 0);
    assert_eq!(config.tool_timeout_secs, 0);
}

#[tokio::test]
async fn debug_capture_records_preparation_and_artifact_timing_at_the_call_boundary() {
    use llm_multiplex::debug_capture::{self, ActiveCapture, CHAT_ADDRESS_METADATA_KEY};
    use serde_json::{json, Value};

    let root = tempfile::tempdir().unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let address = rua_core::ChatAddress::project(format!("project-{id}"), format!("chat-{id}"));
    debug_capture::register_capture(
        address.clone(),
        ActiveCapture {
            capture_id: id,
            capture_path: root.path().to_path_buf(),
        },
    );
    let mut ctx = context(
        Arc::new(EndThenPendingModel {
            info: model_info("timing-test"),
        }),
        ReactConfig::default(),
    );
    ctx.tool_ctx
        .metadata
        .insert(CHAT_ADDRESS_METADATA_KEY.into(), json!(address));
    let (tx, _rx) = tokio::sync::mpsc::channel(32);
    let counter = AtomicU32::new(0);
    let result = react_agent::react_loop::run_react_loop(
        &ctx,
        vec![ModelDialogueItem::text(
            Role::User,
            "complete original input",
        )],
        tx,
        &counter,
    )
    .await;
    debug_capture::unregister_capture(&address);
    debug_capture::flush_capture(root.path(), std::time::Duration::from_secs(5)).unwrap();
    result.unwrap();

    let calls: Vec<_> = std::fs::read_dir(root.path().join("calls"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(calls.len(), 1);
    let call = &calls[0];
    let artifacts: Vec<_> = std::fs::read_dir(call)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    for stage in ["preparation", "capture"] {
        let path = artifacts
            .iter()
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(&format!("{stage}-timing-"))
            })
            .unwrap();
        let trace: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(
            trace["model_call_id"],
            call.file_name().unwrap().to_str().unwrap()
        );
        assert_eq!(trace["outcome"], "completed");
        if stage == "preparation" {
            for phase in [
                "before_model_hook_started",
                "before_model_hook_finished",
                "projection_finished",
                "token_estimate_finished",
                "diagnostics_finished",
                "prompt_inspection_finished",
            ] {
                assert!(trace["milestones_ms"][phase].is_number(), "missing {phase}");
            }
        } else {
            assert!(trace["data"]["context_capture_ms"].is_number());
            assert!(trace["milestones_ms"]["logical_request_written"].is_number());
        }
    }
    let request: ChatRequest =
        serde_json::from_slice(&std::fs::read(call.join("logical-request.json")).unwrap()).unwrap();
    assert_eq!(
        request.messages.last().unwrap().text_content(),
        "complete original input"
    );
    assert!(!request.metadata.contains_key("milestones_ms"));
}

#[tokio::test]
async fn end_event_finishes_model_call_without_stream_eof() {
    let ctx = context(
        Arc::new(EndThenPendingModel {
            info: model_info("end-then-pending"),
        }),
        ReactConfig::default(),
    );
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let counter = AtomicU32::new(0);

    let result = tokio::time::timeout(
        Duration::from_millis(300),
        react_agent::react_loop::run_react_loop(
            &ctx,
            vec![ModelDialogueItem::text(Role::User, "hi")],
            tx,
            &counter,
        ),
    )
    .await
    .expect("End must finish the model call without transport EOF");

    assert!(result.is_ok());
}

#[tokio::test]
async fn eof_before_terminal_event_fails_the_model_call() {
    let ctx = context(
        Arc::new(EofBeforeEndModel {
            info: model_info("eof-before-end"),
        }),
        ReactConfig::default(),
    );
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let counter = AtomicU32::new(0);

    let result = react_agent::react_loop::run_react_loop(
        &ctx,
        vec![ModelDialogueItem::text(Role::User, "hi")],
        tx,
        &counter,
    )
    .await;

    assert!(matches!(result, Err(AgentError::Model(_))));
}

#[tokio::test]
async fn retryable_midstream_error_resets_partial_output_and_retries_model_call() {
    let model = Arc::new(PartialThenConnectionErrorModel {
        info: model_info("partial-then-connection-error"),
        calls: AtomicU32::new(0),
    });
    let ctx = context(model.clone(), ReactConfig::default());
    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    let counter = AtomicU32::new(0);

    let result = react_agent::react_loop::run_react_loop(
        &ctx,
        vec![ModelDialogueItem::text(Role::User, "hi")],
        tx,
        &counter,
    )
    .await;
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }

    assert!(
        result.is_ok(),
        "retryable stream failure should recover: {result:?}"
    );
    assert_eq!(model.calls.load(Ordering::SeqCst), 2);
    let partial_index = events
        .iter()
        .position(
            |event| matches!(event, AgentEvent::TextDelta { delta, .. } if delta == "partial"),
        )
        .expect("first attempt should emit partial text");
    let reset_index = events
        .iter()
        .position(|event| matches!(event, AgentEvent::ModelCallReset { .. }))
        .expect("failed attempt should be reset");
    let complete_index = events
        .iter()
        .position(
            |event| matches!(event, AgentEvent::TextDelta { delta, .. } if delta == "complete"),
        )
        .expect("second attempt should emit complete text");
    assert!(
        partial_index < reset_index && reset_index < complete_index,
        "reset must separate failed and successful attempt output: {events:#?}"
    );
}

#[tokio::test]
async fn retryable_error_before_content_is_not_retried_by_react_loop() {
    let model = Arc::new(ConnectionErrorBeforeContentModel {
        info: model_info("connection-error-before-content"),
        calls: AtomicU32::new(0),
    });
    let ctx = context(model.clone(), ReactConfig::default());
    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    let counter = AtomicU32::new(0);

    let result = react_agent::react_loop::run_react_loop(
        &ctx,
        vec![ModelDialogueItem::text(Role::User, "hi")],
        tx,
        &counter,
    )
    .await;
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }

    assert!(matches!(result, Err(AgentError::Model(_))));
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
    assert!(!events
        .iter()
        .any(|event| matches!(event, AgentEvent::ModelCallReset { .. })));
}

#[tokio::test]
async fn context_overflow_resets_partial_output_before_compaction_retry() {
    let model = Arc::new(PartialThenContextOverflowModel {
        info: model_info("partial-then-context-overflow"),
        calls: AtomicU32::new(0),
    });
    let projector = Arc::new(RecoveringOverflowProjector::default());
    let ctx =
        context(model.clone(), ReactConfig::default()).with_model_call_projector(projector.clone());
    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    let counter = AtomicU32::new(0);

    let result = react_agent::react_loop::run_react_loop(
        &ctx,
        vec![ModelDialogueItem::text(Role::User, "hi")],
        tx,
        &counter,
    )
    .await;
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }

    assert!(
        result.is_ok(),
        "overflow compaction should recover: {result:?}"
    );
    assert_eq!(model.calls.load(Ordering::SeqCst), 2);
    assert_eq!(projector.recoveries.load(Ordering::SeqCst), 1);
    let partial_index = events
        .iter()
        .position(
            |event| matches!(event, AgentEvent::TextDelta { delta, .. } if delta == "partial"),
        )
        .expect("first attempt should emit partial text");
    let reset_index = events
        .iter()
        .position(|event| matches!(event, AgentEvent::ModelCallReset { .. }))
        .expect("failed overflow attempt should be reset");
    let complete_index = events
        .iter()
        .position(
            |event| matches!(event, AgentEvent::TextDelta { delta, .. } if delta == "complete"),
        )
        .expect("second attempt should emit complete text");
    assert!(
        partial_index < reset_index && reset_index < complete_index,
        "reset must remove failed output before the compacted retry: {events:#?}"
    );
}

#[tokio::test]
async fn token_quota_error_does_not_compact_or_retry_history() {
    let model = Arc::new(TokenErrorModel {
        info: model_info("token-quota-error"),
        calls: AtomicU32::new(0),
    });
    let projector = Arc::new(RecoveringOverflowProjector::default());
    let ctx =
        context(model.clone(), ReactConfig::default()).with_model_call_projector(projector.clone());
    let (tx, _rx) = tokio::sync::mpsc::channel(32);
    let counter = AtomicU32::new(0);
    let result = react_agent::react_loop::run_react_loop(
        &ctx,
        vec![ModelDialogueItem::text(Role::User, "hi")],
        tx,
        &counter,
    )
    .await;
    assert!(matches!(result, Err(AgentError::Internal(_))), "{result:?}");
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
    assert_eq!(projector.recoveries.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn model_timeout_interrupts_a_pending_model_call() {
    let config = ReactConfig {
        model_timeout_secs: 1,
        total_timeout_secs: 10,
        ..Default::default()
    };
    let ctx = context(
        Arc::new(PendingModel {
            info: model_info("model-timeout"),
        }),
        config,
    );
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let counter = AtomicU32::new(0);

    let result = tokio::time::timeout(
        Duration::from_secs(3),
        react_agent::react_loop::run_react_loop(
            &ctx,
            vec![ModelDialogueItem::text(Role::User, "hi")],
            tx,
            &counter,
        ),
    )
    .await
    .expect("model timeout must bound the pending stream");

    assert!(matches!(
        result,
        Err(AgentError::Model(LlmError::Timeout { timeout, .. })) if timeout == Duration::from_secs(1)
    ));
}

#[tokio::test(start_paused = true)]
async fn model_timeout_retries_before_failing_the_agent_turn() {
    let calls = Arc::new(AtomicU32::new(0));
    let model = Arc::new(rua_provider::RetryChatModel::new(
        Box::new(TimeoutThenSuccessModel {
            info: model_info("timeout-then-success"),
            calls: calls.clone(),
            failures: 1,
            partial_before_timeout: false,
        }),
        retry_policy(1),
    ));
    let config = ReactConfig {
        model_timeout_secs: 1,
        total_timeout_secs: 5,
        ..Default::default()
    };
    let ctx = context(model.clone(), config);
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let counter = AtomicU32::new(0);

    let result = react_agent::react_loop::run_react_loop(
        &ctx,
        vec![ModelDialogueItem::text(Role::User, "hi")],
        tx,
        &counter,
    )
    .await;
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }

    assert!(result.is_ok(), "timeout retry should recover: {result:?}");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(!events
        .iter()
        .any(|event| matches!(event, AgentEvent::ModelCallReset { .. })));
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::TextDelta { delta, .. } if delta == "recovered")));
}

#[tokio::test(start_paused = true)]
async fn productive_stream_can_exceed_model_timeout() {
    let config = ReactConfig {
        model_timeout_secs: 1,
        total_timeout_secs: 5,
        ..Default::default()
    };
    let ctx = context(
        Arc::new(ProductiveSlowModel {
            info: model_info("productive-slow"),
        }),
        config,
    );
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let counter = AtomicU32::new(0);

    let result = react_agent::react_loop::run_react_loop(
        &ctx,
        vec![ModelDialogueItem::text(Role::User, "hi")],
        tx,
        &counter,
    )
    .await;

    assert!(
        result.is_ok(),
        "productive stream should not time out: {result:?}"
    );
}

#[tokio::test]
async fn assistant_mode_marker_text_cannot_change_runtime_control_state() {
    let hook = Arc::new(RecordingHook::new());
    let ctx = AgentContext::new(
        Arc::new(ModeMarkerModel {
            info: model_info("mode-marker"),
        }),
        Arc::new(AgentToolkit::new()),
        ReactConfig::default(),
        hook.clone(),
        rua_core::tool::ToolContext::default(),
    );
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let counter = AtomicU32::new(0);

    react_agent::react_loop::run_react_loop(
        &ctx,
        vec![ModelDialogueItem::text(Role::User, "hi")],
        tx,
        &counter,
    )
    .await
    .expect("model call");

    let events = hook.events().await;
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, react_agent::HookEvent::BeforeModelCall { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn total_timeout_terminates_the_agent_stream() {
    let config = ReactConfig {
        model_timeout_secs: 60,
        total_timeout_secs: 1,
        ..Default::default()
    };
    let agent = ReactAgent::builder(Arc::new(PendingModel {
        info: model_info("total-timeout"),
    }))
    .config(config)
    .build();
    let mut stream = agent.run_stream(vec![ModelDialogueItem::text(Role::User, "hi")]);

    let events = tokio::time::timeout(Duration::from_secs(2), async {
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            let terminal = event.is_terminal();
            events.push(event);
            if terminal {
                break;
            }
        }
        events
    })
    .await
    .expect("total timeout must terminate the agent stream");

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::Error { message, .. } if message.contains("timed out after 1s")
    )));
}

#[tokio::test(start_paused = true)]
async fn model_timeout_honors_provider_retry_count_including_disabled_retries() {
    for max_retries in [0, 2] {
        let calls = Arc::new(AtomicU32::new(0));
        let model = rua_provider::RetryChatModel::new(
            Box::new(TimeoutThenSuccessModel {
                info: model_info("always-timeout"),
                calls: calls.clone(),
                failures: u32::MAX,
                partial_before_timeout: false,
            }),
            retry_policy(max_retries),
        );
        let ctx = context(
            Arc::new(model),
            ReactConfig {
                model_timeout_secs: 1,
                ..Default::default()
            },
        );
        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        let counter = AtomicU32::new(0);
        let result = react_agent::react_loop::run_react_loop(
            &ctx,
            vec![ModelDialogueItem::text(Role::User, "hi")],
            tx,
            &counter,
        )
        .await;
        assert!(
            matches!(result, Err(AgentError::Model(LlmError::Timeout { .. }))),
            "{result:?}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), max_retries + 1);
        while let Ok(event) = rx.try_recv() {
            assert!(!matches!(event, AgentEvent::ModelCallReset { .. }));
        }
    }
}

#[tokio::test(start_paused = true)]
async fn midstream_timeout_resets_partial_output_before_retrying() {
    let calls = Arc::new(AtomicU32::new(0));
    let model = rua_provider::RetryChatModel::new(
        Box::new(TimeoutThenSuccessModel {
            info: model_info("partial-timeout"),
            calls: calls.clone(),
            failures: 1,
            partial_before_timeout: true,
        }),
        retry_policy(2),
    );
    let ctx = context(
        Arc::new(model),
        ReactConfig {
            model_timeout_secs: 1,
            ..Default::default()
        },
    );
    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    let counter = AtomicU32::new(0);
    react_agent::react_loop::run_react_loop(
        &ctx,
        vec![ModelDialogueItem::text(Role::User, "hi")],
        tx,
        &counter,
    )
    .await
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let mut visible = Vec::new();
    while let Ok(event) = rx.try_recv() {
        match event {
            AgentEvent::TextDelta { delta, .. } => visible.push(delta),
            AgentEvent::ModelCallReset { .. } => visible.push("RESET".into()),
            _ => {}
        }
    }
    assert_eq!(visible, ["partial", "RESET", "recovered"]);
}

#[tokio::test(start_paused = true)]
async fn provider_retry_backoff_stops_on_cancel_or_interjection() {
    for interject in [false, true] {
        let (model, calls) = delayed_retried_model(u32::MAX, 3);
        let (control_tx, control_rx) = tokio::sync::watch::channel(rua_core::RunControl::Running);
        let ctx = context(
            model,
            ReactConfig {
                model_timeout_secs: 1,
                ..Default::default()
            },
        )
        .with_run_control(Some(control_rx));
        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        let counter = AtomicU32::new(0);
        let started = tokio::time::Instant::now();
        let stop = async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if interject {
                control_tx.send(rua_core::RunControl::Interject).unwrap();
            } else {
                ctx.cancel.cancel();
            }
        };
        let (result, ()) = tokio::join!(
            react_agent::react_loop::run_react_loop(
                &ctx,
                vec![ModelDialogueItem::text(Role::User, "hi")],
                tx,
                &counter,
            ),
            stop,
        );
        assert_eq!(started.elapsed(), Duration::from_secs(1));
        if interject {
            result.unwrap();
            let mut interjected = false;
            while let Ok(event) = rx.try_recv() {
                interjected |= matches!(event, AgentEvent::End {
                    finish_reason: FinishReason::Unknown(reason), ..
                } if reason == "interjected");
            }
            assert!(interjected);
        } else {
            assert!(matches!(result, Err(AgentError::Cancelled)), "{result:?}");
        }
        tokio::time::advance(Duration::from_secs(5)).await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "stopping backoff must prevent another request"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn total_timeout_stops_provider_retry_backoff() {
    let (model, calls) = delayed_retried_model(u32::MAX, 3);
    let agent = ReactAgent::builder(model)
        .config(ReactConfig {
            model_timeout_secs: 1,
            total_timeout_secs: 1,
            ..Default::default()
        })
        .build();
    let mut stream = agent.run_stream(vec![ModelDialogueItem::text(Role::User, "hi")]);
    let started = tokio::time::Instant::now();
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        let terminal = event.is_terminal();
        events.push(event);
        if terminal {
            break;
        }
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(started.elapsed(), Duration::from_secs(1));
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::Error { message, .. }
        if message.contains("agent execution timed out after 1s"))));
}

#[tokio::test(start_paused = true)]
async fn provider_retry_deadline_cannot_restart_a_partial_model_call() {
    let calls = Arc::new(AtomicU32::new(0));
    let model = rua_provider::RetryChatModel::new(
        Box::new(TimeoutThenSuccessModel {
            info: model_info("partial-deadline"),
            calls: calls.clone(),
            failures: u32::MAX,
            partial_before_timeout: true,
        }),
        rua_provider::RetryPolicy {
            max_total_time: Some(Duration::from_millis(500)),
            ..retry_policy(2)
        },
    );
    let ctx = context(
        Arc::new(model),
        ReactConfig {
            model_timeout_secs: 1,
            ..Default::default()
        },
    );
    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    let counter = AtomicU32::new(0);
    let started = tokio::time::Instant::now();
    let result = react_agent::react_loop::run_react_loop(
        &ctx,
        vec![ModelDialogueItem::text(Role::User, "hi")],
        tx,
        &counter,
    )
    .await;
    assert!(
        matches!(
            result,
            Err(AgentError::Model(LlmError::RetryDeadlineExceeded { .. }))
        ),
        "{result:?}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(started.elapsed(), Duration::from_millis(500));
    while let Ok(event) = rx.try_recv() {
        assert!(!matches!(event, AgentEvent::ModelCallReset { .. }));
    }
}

#[tokio::test(start_paused = true)]
async fn zero_model_timeout_allows_a_delayed_provider_response() {
    let (model, calls) = delayed_retried_model(0, 0);
    let ctx = context(
        model,
        ReactConfig {
            model_timeout_secs: 0,
            ..Default::default()
        },
    );
    let (tx, _rx) = tokio::sync::mpsc::channel(32);
    let counter = AtomicU32::new(0);
    react_agent::react_loop::run_react_loop(
        &ctx,
        vec![ModelDialogueItem::text(Role::User, "hi")],
        tx,
        &counter,
    )
    .await
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[derive(Default)]
struct HiddenOutputHook {
    persisted: tokio::sync::Mutex<Vec<ModelDialogueItem>>,
}

#[async_trait]
impl react_agent::hooks::AgentHook for HiddenOutputHook {
    async fn after_model_event(&self, _event: &AgentEvent) -> bool {
        false
    }

    async fn after_model_call(
        &self,
        messages: &[ModelDialogueItem],
        _iteration: u32,
        _total_usage: &agent_types::TokenUsage,
        _last_call_usage: Option<&agent_types::TokenUsage>,
        _terminal: bool,
    ) -> Result<(), AgentError> {
        self.persisted.lock().await.extend_from_slice(messages);
        Ok(())
    }
}

#[tokio::test(start_paused = true)]
async fn timeout_preserves_partial_response_even_when_output_was_hidden() {
    let calls = Arc::new(AtomicU32::new(0));
    let model = rua_provider::RetryChatModel::new(
        Box::new(TimeoutThenSuccessModel {
            info: model_info("hidden-timeout"),
            calls: calls.clone(),
            failures: u32::MAX,
            partial_before_timeout: true,
        }),
        retry_policy(2),
    );
    let hook = Arc::new(HiddenOutputHook::default());
    let ctx = AgentContext::new(
        Arc::new(model),
        Arc::new(AgentToolkit::new()),
        ReactConfig {
            model_timeout_secs: 1,
            ..Default::default()
        },
        hook.clone(),
        rua_core::tool::ToolContext::default(),
    );
    let (tx, _rx) = tokio::sync::mpsc::channel(32);
    let counter = AtomicU32::new(0);
    let result = react_agent::react_loop::run_react_loop(
        &ctx,
        vec![ModelDialogueItem::text(Role::User, "hi")],
        tx,
        &counter,
    )
    .await;
    assert!(matches!(
        result,
        Err(AgentError::Model(LlmError::Timeout { .. }))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let persisted = hook.persisted.lock().await;
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].text_content(), "partial");
}
