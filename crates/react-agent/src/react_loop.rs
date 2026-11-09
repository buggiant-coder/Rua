use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;

use agent_types::conversation_message_identity::{
    reasoning_message_id, text_message_id, tool_result_message_id, tool_use_message_id,
    CANONICAL_MESSAGE_ID_METADATA_KEY, MODEL_DIALOGUE_ITEM_ID_METADATA_KEY,
    REASONING_MESSAGE_ID_METADATA_KEY, TEXT_MESSAGE_ID_METADATA_KEY,
};
use agent_types::model_dialogue_item::{
    AssistantMessagePart, ASSISTANT_MESSAGE_PARTS_METADATA_KEY, ASSISTANT_PHASE_METADATA_KEY,
    PROVIDER_MESSAGE_ID_METADATA_KEY,
};
use agent_types::{
    AgentEvent, ChatRequest, ContentPart, FinishReason, ModelDialogueItem, Role, TokenUsage,
    ToolCall, ToolDefinition, ToolResult,
};
use futures::{stream::FuturesUnordered, StreamExt};
use llm_multiplex::debug_capture::CaptureTrace;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::context::AgentContext;
use crate::error::AgentError;
use crate::hooks::AgentHook;
use crate::hooks::ApprovalResult;
use crate::model_call_context::{ModelInputTokenEstimate, MODEL_INPUT_TOKEN_ESTIMATE_METADATA_KEY};
use crate::tool_accumulator::ToolCallAccumulator;
use crate::tool_disclosure::{
    resolve_bridge_read_call, resolve_bridge_tool_call, BridgeReadResolution,
    BridgeToolCallResolution, ToolDisclosureConfig,
};
use crate::toolkit::AgentToolkit;
use crate::ToolDecision;

#[derive(Debug, Clone)]
pub struct TokenBudget {
    max_tokens: usize,
    threshold_pct: f64,
}

impl TokenBudget {
    pub fn new(max_tokens: usize) -> Self {
        Self {
            max_tokens,
            threshold_pct: 0.85,
        }
    }

    pub fn with_threshold(mut self, pct: f64) -> Self {
        self.threshold_pct = pct.clamp(0.0, 1.0);
        self
    }

    pub fn estimate_message_tokens(msg: &ModelDialogueItem) -> usize {
        let mut tokens = 4;
        for part in &msg.content {
            tokens += match part {
                ContentPart::Text { text } => text.len() / 4 + 1,
                ContentPart::Thinking { text, .. } => text.len() / 4 + 1,
                ContentPart::ToolUse { call } => {
                    call.name.len() / 4 + call.arguments.to_string().len() / 4 + 10
                }
                ContentPart::ToolResult { result } => result.text_content().len() / 4 + 5,
                _ => 10,
            };
        }
        tokens
    }

    pub fn estimate_messages_tokens(messages: &[ModelDialogueItem]) -> usize {
        messages
            .iter()
            .map(Self::estimate_message_tokens)
            .sum::<usize>()
            + 3
    }

    pub fn threshold(&self) -> usize {
        (self.max_tokens as f64 * self.threshold_pct) as usize
    }

    pub fn exceeds_threshold(&self, messages: &[ModelDialogueItem]) -> bool {
        if self.max_tokens == 0 {
            return false;
        }
        Self::estimate_messages_tokens(messages) > self.threshold()
    }

    pub fn utilization_pct(&self, messages: &[ModelDialogueItem]) -> f64 {
        if self.max_tokens == 0 {
            return 0.0;
        }
        Self::estimate_messages_tokens(messages) as f64 / self.max_tokens as f64 * 100.0
    }

    pub fn is_enabled(&self) -> bool {
        self.max_tokens > 0
    }
}

#[derive(Debug)]
pub struct IterationResult {
    pub model_call_id: String,

    pub input_token_estimate: Option<ModelInputTokenEstimate>,

    pub text: String,

    pub text_messages: Vec<ModelDialogueItem>,

    pub thinking: String,

    pub thinking_signature: Option<String>,

    pub tool_calls: Vec<ToolCall>,

    pub finish_reason: FinishReason,

    pub usage: Option<TokenUsage>,

    pub response_metadata: serde_json::Value,

    #[allow(dead_code)]
    pub events: Vec<AgentEvent>,

    pub parse_errors: Vec<(ToolCall, ToolResult)>,
}

pub const AUTO_CONTINUE_HINT: &str = "\
<system-hint>\
You have tools available. If the task requires action, please use \
the appropriate tool instead of just describing what to do. / \
你有可用的工具。如果任务需要操作，请使用合适的工具而不是只描述要做什么。\
</system-hint>";

const SUMMARIZING_HINT: &str = "\
The run reached the configured tool-round limit. No further tools can run in this turn. \
Summarize completed work, remaining work, and the limit that stopped execution. \
Do not claim the goal is complete or promise to execute another action in this turn.";

type RunningToolFuture = std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = std::result::Result<
                    std::result::Result<ToolResult, AgentError>,
                    tokio::time::error::Elapsed,
                >,
            > + Send,
    >,
>;

async fn wait_for_interjection(rx: &mut Option<watch::Receiver<rua_core::RunControl>>) {
    wait_for_interjection_at(rx, false).await;
}

async fn wait_for_model_interjection(rx: &mut Option<watch::Receiver<rua_core::RunControl>>) {
    wait_for_interjection_at(rx, true).await;
}

async fn wait_for_interjection_at(
    rx: &mut Option<watch::Receiver<rua_core::RunControl>>,
    model: bool,
) {
    let interrupts = |control: rua_core::RunControl| {
        control.is_immediate_interject()
            || (model && control == rua_core::RunControl::InterjectAfterTool)
    };
    let Some(rx) = rx.as_mut() else {
        std::future::pending::<()>().await;
        return;
    };
    if interrupts(*rx.borrow()) {
        return;
    }
    while rx.changed().await.is_ok() {
        if interrupts(*rx.borrow()) {
            return;
        }
    }
    std::future::pending::<()>().await;
}

fn interjected_finish_reason() -> FinishReason {
    FinishReason::Unknown("interjected".to_string())
}

fn incomplete_model_stream(ctx: &AgentContext) -> AgentError {
    AgentError::Model(llm_multiplex::LlmError::Connection {
        provider: ctx
            .provider_id
            .clone()
            .unwrap_or_else(|| ctx.model.model_info().id.clone()),
        message: "model stream closed before terminal End/Error event".to_string(),
    })
}

pub async fn run_react_loop(
    ctx: &AgentContext,
    initial_messages: Vec<ModelDialogueItem>,
    event_tx: tokio::sync::mpsc::Sender<AgentEvent>,
    iteration_counter: &AtomicU32,
) -> Result<(), AgentError> {
    let mut messages = initial_messages;
    let mut iteration: u32 = 0;
    let mut tool_rounds: u32 = 0;
    let mut total_usage = TokenUsage::default();
    let mut compaction_retries: u32 = 0;
    let mut auto_continue_text_count: u32 = 0;
    let mut multimodal_retry_done: bool = false;
    let mut next_call_guidance: Option<&'static str> = None;
    let mut previous_trace = CaptureTrace::default();

    loop {
        previous_trace.mark("next_loop_started");
        if ctx.is_cancelled() {
            previous_trace.finish("cancelled");
            return Err(AgentError::Cancelled);
        }

        if ctx.config.max_iterations > 0 && tool_rounds >= ctx.config.max_iterations {
            warn!(
                "Max iterations ({}) reached, performing summarizing call",
                ctx.config.max_iterations
            );
            ctx.hook
                .on_iteration_limit(ctx.config.max_iterations)
                .await?;
            let (summary_result, mut trace) =
                run_summarizing_call(ctx, &messages, iteration, &event_tx, &mut previous_trace)
                    .await?;
            if let Some(ref usage) = summary_result.usage {
                total_usage.accumulate(usage);
            }
            let completed_message_start = messages.len();
            let assistant_msg = build_assistant_message(&summary_result);
            messages.push(assistant_msg);
            let persisted = trace
                .measure_local(
                    "assistant_after_model_call_started",
                    "assistant_after_model_call_finished",
                    ctx.hook.after_model_call(
                        &messages[completed_message_start..],
                        iteration,
                        &total_usage,
                        summary_result.usage.as_ref(),
                        true,
                    ),
                )
                .await;
            if persisted.is_err() {
                trace.finish("error");
            }
            persisted?;
            let _ = event_tx
                .send(AgentEvent::IterationEnd {
                    iteration,
                    has_tool_calls: false,
                })
                .await;
            emit_usage_and_end(
                &event_tx,
                &total_usage,
                if summary_result.finish_reason == interjected_finish_reason() {
                    summary_result.finish_reason
                } else {
                    FinishReason::Unknown("max_iterations".to_string())
                },
            )
            .await;
            trace.finish("max_iterations");
            return Ok(());
        }

        let model_rejects_media = ctx.model.model_info().supports_multimodal == Some(false)
            || ctx
                .provider_id
                .as_ref()
                .map(|pid| {
                    let key =
                        rua_provider::capability_cache::cache_key(pid, &ctx.model.model_info().id);
                    rua_provider::capability_cache::rejects_media(&key)
                })
                .unwrap_or(false);
        if model_rejects_media {
            let count = strip_media_blocks_from_memory(&mut messages);
            if count > 0 {
                warn!(
                    stripped_count = count,
                    "Proactive multimodal strip: model does not support multimodal, stripped {} media blocks",
                    count
                );
            }
        }

        let (request, mut trace) = prepare_model_call_request(
            ctx,
            messages,
            iteration,
            true,
            next_call_guidance.take(),
            &event_tx,
            &mut previous_trace,
        )
        .await?;
        messages = request.messages.clone();
        trace.mark("messages_cloned");
        let current_model_call_id = model_call_id(&request)?;

        let _ = event_tx
            .send(AgentEvent::IterationStart { iteration })
            .await;
        trace.mark("iteration_start_emitted");
        previous_trace.mark("next_model_invocation_started");
        previous_trace.finish("continued");

        let iter_result = match run_model_iteration(
            ctx,
            request,
            iteration,
            &total_usage,
            &event_tx,
            &mut trace,
        )
        .await
        {
            Ok(result) => {
                compaction_retries = 0;
                result
            }
            Err(AgentError::ContextOverflow { ref message }) => {
                trace.finish("context_overflow");
                let _ = event_tx
                    .send(AgentEvent::ModelCallReset {
                        model_call_id: current_model_call_id,
                    })
                    .await;
                if ctx.config.compact_on_overflow
                    && compaction_retries < ctx.config.max_compaction_retries
                {
                    compaction_retries += 1;
                    debug!(
                        "Context overflow, attempting compaction (retry {}/{})",
                        compaction_retries, ctx.config.max_compaction_retries
                    );
                    let Some(projector) = ctx.model_call_projector.as_ref() else {
                        return Err(AgentError::ContextOverflow {
                            message: message.clone(),
                        });
                    };
                    messages = projector
                        .recover_context_overflow(&messages, message)
                        .await?;
                    continue;
                }
                return Err(AgentError::ContextOverflow {
                    message: message.clone(),
                });
            }
            Err(e) => {
                trace.finish(if e.is_cancelled() {
                    "cancelled"
                } else {
                    "error"
                });
                if !multimodal_retry_done && is_multimodal_error(&e) && has_media_blocks(&messages)
                {
                    multimodal_retry_done = true;

                    if let Some(ref pid) = ctx.provider_id {
                        let key = rua_provider::capability_cache::cache_key(
                            pid,
                            &ctx.model.model_info().id,
                        );
                        rua_provider::capability_cache::learn_rejects_media(&key);
                    }
                    let count = strip_media_blocks_from_memory(&mut messages);
                    warn!(
                        stripped_count = count,
                        "Passive multimodal strip: model rejected media input, stripped {} media blocks, retrying",
                        count
                    );
                    continue;
                }
                return Err(e);
            }
        };
        trace.mark("model_iteration_finished");

        if let Some(ref usage) = iter_result.usage {
            total_usage.accumulate(usage);
        }

        let completed_message_start = messages.len();
        let completed_iteration = iteration;
        let assistant_msg = build_assistant_message(&iter_result);
        messages.push(assistant_msg);
        let has_tool_work =
            !iter_result.tool_calls.is_empty() || !iter_result.parse_errors.is_empty();
        trace.set("tool_call_count", iter_result.tool_calls.len());
        trace.set("tool_parse_error_count", iter_result.parse_errors.len());
        let persisted = trace
            .measure_local(
                "assistant_after_model_call_started",
                "assistant_after_model_call_finished",
                ctx.hook.after_model_call(
                    &messages[completed_message_start..],
                    completed_iteration,
                    &total_usage,
                    iter_result.usage.as_ref(),
                    !has_tool_work,
                ),
            )
            .await;
        if persisted.is_err() {
            trace.finish("error");
        }
        persisted?;

        trace.mark("iteration_end_emit_started");
        let _ = event_tx
            .send(AgentEvent::IterationEnd {
                iteration,
                has_tool_calls: !iter_result.tool_calls.is_empty()
                    || !iter_result.parse_errors.is_empty(),
            })
            .await;
        trace.mark("iteration_end_emitted");

        iteration += 1;
        iteration_counter.store(iteration, Ordering::Relaxed);

        if has_tool_work {
            trace.mark("tools_started");
            let mut tool_results = if !iter_result.tool_calls.is_empty() {
                let executed = execute_tools_traced(
                    ctx,
                    &iter_result.tool_calls,
                    iteration,
                    &event_tx,
                    &trace,
                )
                .await;
                if let Err(error) = &executed {
                    trace.finish(if error.is_cancelled() {
                        "cancelled"
                    } else {
                        "error"
                    });
                }
                executed?
            } else {
                Vec::new()
            };
            trace.mark("tools_finished");

            for (_, error_result) in &iter_result.parse_errors {
                tool_results.push(error_result.clone());
            }

            let tool_message_start = messages.len();
            for result in &tool_results {
                messages.push(ModelDialogueItem {
                    id: None,
                    role: Role::Tool,
                    content: vec![ContentPart::ToolResult {
                        result: result.clone(),
                    }],
                    name: result.name.clone(),
                    created_at: None,
                    metadata: serde_json::Map::new(),
                });
            }
            let persisted = trace
                .measure_local(
                    "tool_results_after_model_call_started",
                    "tool_results_after_model_call_finished",
                    ctx.hook.after_model_call(
                        &messages[tool_message_start..],
                        completed_iteration,
                        &total_usage,
                        None,
                        false,
                    ),
                )
                .await;
            if persisted.is_err() {
                trace.finish("error");
            }
            persisted?;
            tool_rounds += 1;

            auto_continue_text_count = 0;

            let ask_user = iter_result.tool_calls.iter().any(|c| c.name == "ask_user");
            let interjected = ctx.is_interjected();
            let should_exit_at_boundary = ctx.should_exit_at_boundary();
            if ask_user {
                emit_usage_and_end(&event_tx, &total_usage, FinishReason::Stop).await;
                trace.finish("waiting_for_user");
                return Ok(());
            }

            if interjected || should_exit_at_boundary {
                let finish_reason = if interjected {
                    interjected_finish_reason()
                } else {
                    FinishReason::Stop
                };
                emit_usage_and_end(&event_tx, &total_usage, finish_reason).await;
                trace.finish("stopped_at_boundary");
                return Ok(());
            }

            previous_trace = trace;
            continue;
        }

        if ctx.config.auto_continue_on_text_only
            && iter_result.finish_reason == FinishReason::Stop
            && iter_result.tool_calls.is_empty()
            && !ctx.current_toolkit().is_empty()
            && auto_continue_text_count < ctx.config.auto_continue_max_retries
        {
            auto_continue_text_count += 1;
            debug!(
                "auto_continue_on_text_only: injecting nudge, attempt {}/{}",
                auto_continue_text_count, ctx.config.auto_continue_max_retries
            );
            next_call_guidance = Some(AUTO_CONTINUE_HINT);
            previous_trace = trace;
            continue;
        }

        emit_usage_and_end(&event_tx, &total_usage, iter_result.finish_reason).await;
        trace.finish("completed");
        return Ok(());
    }
}

async fn prepare_model_call_request(
    ctx: &AgentContext,
    messages: Vec<ModelDialogueItem>,
    iteration: u32,
    include_tools: bool,
    guidance: Option<&str>,
    event_tx: &tokio::sync::mpsc::Sender<AgentEvent>,
    previous_trace: &mut CaptureTrace,
) -> Result<(ChatRequest, CaptureTrace), AgentError> {
    previous_trace.mark("next_preparation_started");
    let capture_started = std::time::Instant::now();
    let context = crate::model_call_context::ModelCallContext::capture_with_guidance(
        ctx,
        messages,
        iteration,
        include_tools,
        guidance,
    );
    let context_capture_ms = capture_started.elapsed().as_secs_f64() * 1000.0;
    previous_trace.set("next_model_call_id", context.model_call_id());
    let mut iteration_trace = context.capture_trace("iteration");
    iteration_trace.set("context_capture_ms", context_capture_ms);
    iteration_trace.mark("preparation_started");
    let projected = context.project(ctx, event_tx).await;
    if projected.is_err() {
        iteration_trace.finish("preparation_error");
        previous_trace.finish("next_preparation_error");
    }
    let mut request = projected?;
    iteration_trace.mark("projection_finished");
    let mut trace = llm_multiplex::debug_capture::CaptureTrace::new(&request.metadata, "capture");
    trace.set("context_capture_ms", context_capture_ms);
    let mut tool_result_truncation = Vec::new();
    for message in &mut request.messages {
        if let Some(value) = message
            .metadata
            .remove("rua_llm_debug_tool_result_truncation")
        {
            if let Some(entries) = value.as_array() {
                tool_result_truncation.extend(entries.iter().cloned());
            }
        }
    }
    let mut projection = request
        .metadata
        .remove("rua_llm_debug_projection_evidence")
        .unwrap_or_else(|| {
            serde_json::json!({
                "messages": request.messages.iter().enumerate().map(|(output_index, message)| {
                    serde_json::json!({
                        "output_index": output_index,
                        "message_id": message.id,
                        "role": message.role,
                        "metadata": message.metadata,
                    })
                }).collect::<Vec<_>>()
            })
        });
    if let Some(object) = projection.as_object_mut() {
        object.insert(
            "tool_result_truncation".into(),
            serde_json::Value::Array(tool_result_truncation),
        );
    }
    llm_multiplex::debug_capture::record_named_json(
        &request.metadata,
        "projection.json",
        &projection,
    );
    trace.mark("projection_written");
    // Writing real evidence first avoids record_logical_request's placeholder
    // followed immediately by a second atomic write to the same projection file.
    llm_multiplex::debug_capture::record_logical_request(&request);
    trace.mark("logical_request_written");
    if let Some(token_estimate) = request
        .metadata
        .get(MODEL_INPUT_TOKEN_ESTIMATE_METADATA_KEY)
    {
        llm_multiplex::debug_capture::record_named_json(
            &request.metadata,
            "token-estimate.json",
            token_estimate,
        );
    }
    trace.finish("completed");
    iteration_trace.mark("request_prepared");
    previous_trace.mark("next_request_prepared");
    Ok((request, iteration_trace))
}

async fn run_summarizing_call(
    ctx: &AgentContext,
    messages: &[ModelDialogueItem],
    iteration: u32,
    event_tx: &tokio::sync::mpsc::Sender<AgentEvent>,
    previous_trace: &mut CaptureTrace,
) -> Result<(IterationResult, CaptureTrace), AgentError> {
    let (request, mut trace) = prepare_model_call_request(
        ctx,
        messages.to_vec(),
        iteration,
        false,
        Some(SUMMARIZING_HINT),
        event_tx,
        previous_trace,
    )
    .await?;
    let model_call_id = model_call_id(&request)?;
    let input_token_estimate = input_token_estimate(&request);
    let _ = event_tx
        .send(AgentEvent::IterationStart { iteration })
        .await;
    previous_trace.mark("next_model_invocation_started");
    previous_trace.finish("continued");

    trace.mark("model_invocation_started");
    let mut stream = ctx
        .model
        .chat_stream_with_idle_timeout(request, ctx.config.model_timeout());
    let mut run_control = ctx.run_control.clone();
    let mut tool_call_ids_by_index = HashMap::new();

    let mut text = String::new();
    let mut text_messages = Vec::new();
    let mut usage: Option<TokenUsage> = None;
    let mut provider_usage_event_seen = false;
    let response_metadata;

    let finish_reason = loop {
        let event_result = tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => return Err(AgentError::Cancelled),
            _ = wait_for_model_interjection(&mut run_control) => {
                trace.mark("model_interjected");
                return Ok((IterationResult {
                    model_call_id,
                    input_token_estimate: input_token_estimate.clone(),
                    text,
                    text_messages,
                    thinking: String::new(),
                    thinking_signature: None,
                    tool_calls: Vec::new(),
                    finish_reason: interjected_finish_reason(),
                    usage,
                    response_metadata: serde_json::Value::Null,
                    events: Vec::new(),
                    parse_errors: Vec::new(),
                }, trace));
            }
            ev = stream.next() => match ev {
                Some(e) => e,
                None => return Err(incomplete_model_stream(ctx)),
            },
        };

        let event = event_result.map_err(AgentError::Model)?;

        let event =
            assign_model_output_identity(event, &model_call_id, &mut tool_call_ids_by_index);
        match &event {
            AgentEvent::TextDelta { delta, metadata } => {
                text.push_str(delta);
                append_text_message(&mut text_messages, delta, metadata);
                if !delta.is_empty() && ctx.hook.after_model_event(&event).await {
                    let _ = event_tx.send(event.clone()).await;
                }
            }
            AgentEvent::Usage { usage: u } => {
                usage = Some(u.clone());
                provider_usage_event_seen = true;
                if ctx.hook.after_model_event(&event).await {
                    let _ = event_tx.send(event.clone()).await;
                }
            }
            AgentEvent::End {
                finish_reason: fr,
                usage: u,
                metadata,
            } => {
                trace.mark("model_end_received");
                if u.is_some() {
                    usage = u.clone();
                }
                response_metadata = metadata.clone();
                break fr.clone();
            }
            AgentEvent::Error { message, code, .. } => {
                if rua_provider::error::is_context_overflow(message, code.as_deref()) {
                    return Err(AgentError::ContextOverflow {
                        message: message.clone(),
                    });
                }
                return Err(AgentError::Internal(message.clone()));
            }
            _ => {}
        }
    };

    forward_captured_usage_if_needed(ctx, event_tx, usage.as_ref(), provider_usage_event_seen)
        .await;

    trace.mark("model_iteration_finished");
    Ok((
        IterationResult {
            model_call_id,
            input_token_estimate,
            text,
            text_messages,
            thinking: String::new(),
            thinking_signature: None,
            tool_calls: Vec::new(),
            finish_reason,
            usage,
            response_metadata,
            events: Vec::new(),
            parse_errors: Vec::new(),
        },
        trace,
    ))
}

async fn run_model_iteration(
    ctx: &AgentContext,
    request: ChatRequest,
    iteration: u32,
    total_usage_before: &TokenUsage,
    event_tx: &tokio::sync::mpsc::Sender<AgentEvent>,
    trace: &mut CaptureTrace,
) -> Result<IterationResult, AgentError> {
    let model_call_id = model_call_id(&request)?;
    let mut emitted_output = false;
    let first_request = request.clone();
    trace.mark("retry_request_cloned");
    // The provider owns retries before content, including per-attempt timeouts.
    // Only retry here when displayed partial output needs a ModelCallReset.
    match run_model_iteration_attempt(
        ctx,
        first_request,
        iteration,
        total_usage_before,
        event_tx,
        false,
        &mut emitted_output,
        trace,
    )
    .await
    {
        Err(error) if matches!(&error, AgentError::Model(error) if emitted_output && error.is_retryable()) =>
        {
            trace.mark("model_retry_started");
            let _ = event_tx
                .send(AgentEvent::ModelCallReset { model_call_id })
                .await;
            let mut retry_emitted_output = false;
            run_model_iteration_attempt(
                ctx,
                request,
                iteration,
                total_usage_before,
                event_tx,
                true,
                &mut retry_emitted_output,
                trace,
            )
            .await
        }
        result => result,
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_model_iteration_attempt(
    ctx: &AgentContext,
    request: ChatRequest,
    iteration: u32,
    total_usage_before: &TokenUsage,
    event_tx: &tokio::sync::mpsc::Sender<AgentEvent>,
    persist_transport_error: bool,
    emitted_output: &mut bool,
    trace: &mut CaptureTrace,
) -> Result<IterationResult, AgentError> {
    let model_call_id = model_call_id(&request)?;
    let input_token_estimate = input_token_estimate(&request);

    {
        use rua_tool::extended::ask_user::AskUserService;
        let svc = AskUserService::global();
        if svc
            .get_pending_by_session(&ctx.tool_ctx.session_id)
            .await
            .is_some()
        {
            debug!(session_id = %ctx.tool_ctx.session_id, "Pending ask_user detected for session, skipping LLM call");
            return Ok(IterationResult {
                model_call_id,
                input_token_estimate: input_token_estimate.clone(),
                text: String::new(),
                text_messages: Vec::new(),
                thinking: String::new(),
                thinking_signature: None,
                tool_calls: Vec::new(),
                finish_reason: FinishReason::Stop,
                usage: None,
                response_metadata: serde_json::Value::Null,
                events: Vec::new(),
                parse_errors: Vec::new(),
            });
        }
    }

    trace.mark("model_invocation_started");
    let mut stream = ctx
        .model
        .chat_stream_with_idle_timeout(request, ctx.config.model_timeout());
    let mut run_control = ctx.run_control.clone();

    let mut text = String::new();
    let mut text_messages = Vec::new();
    let mut thinking = String::new();
    let mut thinking_signature: Option<String> = None;
    let mut tool_accumulator = ToolCallAccumulator::new();
    let mut tool_call_ids_by_index = HashMap::new();
    let mut direct_tool_calls: Vec<ToolCall> = Vec::new();
    let mut usage: Option<TokenUsage> = None;
    let mut provider_usage_event_seen = false;
    let response_metadata;
    let mut _events = Vec::new();

    let finish_reason = loop {
        let event_result = tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => {
                persist_partial_model_response(
                    ctx,
                    iteration,
                    &model_call_id,
                    input_token_estimate.as_ref(),
                    &text,
                    &text_messages,
                    &thinking,
                    thinking_signature.as_ref(),
                    total_usage_before,
                    usage.as_ref(),
                ).await?;
                return Err(AgentError::Cancelled);
            },
            _ = wait_for_model_interjection(&mut run_control) => {
                return Ok(IterationResult {
                    model_call_id: model_call_id.clone(),
                    input_token_estimate: input_token_estimate.clone(),
                    text,
                    text_messages,
                    thinking,
                    thinking_signature,
                    tool_calls: Vec::new(),
                    finish_reason: interjected_finish_reason(),
                    usage,
                    response_metadata: serde_json::Value::Null,
                    events: _events,
                    parse_errors: Vec::new(),
                });
            }
            ev = stream.next() => match ev {
                Some(e) => e,
                None => {
                    persist_partial_model_response(
                        ctx,
                        iteration,
                        &model_call_id,
                        input_token_estimate.as_ref(),
                        &text,
                        &text_messages,
                        &thinking,
                        thinking_signature.as_ref(),
                        total_usage_before,
                        usage.as_ref(),
                    ).await?;
                    return Err(incomplete_model_stream(ctx));
                },
            },
        };

        let event = match event_result {
            Ok(event) => event,
            Err(error) => {
                // With no displayed output the outer loop will not retry, even
                // if the provider produced content that a Hook kept hidden.
                if persist_transport_error || !error.is_retryable() || !*emitted_output {
                    persist_partial_model_response(
                        ctx,
                        iteration,
                        &model_call_id,
                        input_token_estimate.as_ref(),
                        &text,
                        &text_messages,
                        &thinking,
                        thinking_signature.as_ref(),
                        total_usage_before,
                        usage.as_ref(),
                    )
                    .await?;
                }
                return Err(AgentError::Model(error));
            }
        };

        let event =
            assign_model_output_identity(event, &model_call_id, &mut tool_call_ids_by_index);
        match &event {
            AgentEvent::Start { .. } => {}
            AgentEvent::TextDelta { delta, metadata } => {
                text.push_str(delta);
                append_text_message(&mut text_messages, delta, metadata);
                if !delta.is_empty() && ctx.hook.after_model_event(&event).await {
                    *emitted_output = true;
                    let _ = event_tx.send(event.clone()).await;
                }
            }
            AgentEvent::ThinkingDelta {
                delta, signature, ..
            } => {
                thinking.push_str(delta);
                if signature.is_some() {
                    thinking_signature = signature.clone();
                }
                if ctx.config.emit_thinking && ctx.hook.after_model_event(&event).await {
                    *emitted_output = true;
                    let _ = event_tx.send(event.clone()).await;
                }
            }
            AgentEvent::ToolCallDelta { .. } => {
                tool_accumulator.push_event(&event);
                if ctx.config.emit_tool_deltas && ctx.hook.after_model_event(&event).await {
                    *emitted_output = true;
                    let _ = event_tx.send(event.clone()).await;
                }
            }
            AgentEvent::ToolCall { call } => {
                debug!(
                    tool_call_id = %call.id,
                    tool_name = %call.name,
                    arguments = %call.arguments,
                    raw_arguments = ?call.raw_arguments,
                    "react_loop: received direct ToolCall event"
                );
                if ctx.hook.after_model_event(&event).await {
                    *emitted_output = true;
                    let _ = event_tx.send(event.clone()).await;
                }
                direct_tool_calls.push(call.clone());
            }
            AgentEvent::Usage { usage: u } => {
                usage = Some(u.clone());
                provider_usage_event_seen = true;
                if ctx.hook.after_model_event(&event).await {
                    let _ = event_tx.send(event.clone()).await;
                }
            }
            AgentEvent::End {
                finish_reason: fr,
                usage: u,
                metadata,
            } => {
                trace.mark("model_end_received");
                if u.is_some() {
                    usage = u.clone();
                }
                response_metadata = metadata.clone();

                break fr.clone();
            }
            AgentEvent::Error { message, code, .. } => {
                if rua_provider::error::is_context_overflow(message, code.as_deref()) {
                    return Err(AgentError::ContextOverflow {
                        message: message.clone(),
                    });
                }

                persist_partial_model_response(
                    ctx,
                    iteration,
                    &model_call_id,
                    input_token_estimate.as_ref(),
                    &text,
                    &text_messages,
                    &thinking,
                    thinking_signature.as_ref(),
                    total_usage_before,
                    usage.as_ref(),
                )
                .await?;
                return Err(AgentError::Internal(message.clone()));
            }
            _ => {
                if ctx.hook.after_model_event(&event).await {
                    let _ = event_tx.send(event.clone()).await;
                }
            }
        }
    };

    trace.mark("final_usage_emit_started");
    forward_captured_usage_if_needed(ctx, event_tx, usage.as_ref(), provider_usage_event_seen)
        .await;
    trace.mark("final_usage_emitted");

    trace.mark("tool_calls_finalize_started");
    let (tool_calls, parse_errors) = {
        let mut good_calls = Vec::new();
        let mut errors = Vec::new();
        let mut seen_ids = HashSet::new();

        for mut call in direct_tool_calls {
            stamp_tool_call_identity(&mut call);
            if seen_ids.insert(call.id.clone()) {
                good_calls.push(call);
            } else {
                warn!(tool_call_id = %call.id, "ignoring duplicate complete tool call id");
            }
        }

        if tool_accumulator.has_pending() {
            debug!(
                pending = tool_accumulator.pending_count(),
                "react_loop: finalizing streaming tool call deltas"
            );
            let finalize_results = tool_accumulator.finalize();

            for result in finalize_results {
                match result {
                    Ok(mut call) => {
                        stamp_tool_call_identity(&mut call);
                        if !seen_ids.insert(call.id.clone()) {
                            warn!(
                                tool_call_id = %call.id,
                                "ignoring duplicate finalized tool call id"
                            );
                            continue;
                        }
                        debug!(
                            tool_call_id = %call.id,
                            tool_name = %call.name,
                            arguments = %call.arguments,
                            "react_loop: emitting finalized ToolCall"
                        );
                        let tc_event = AgentEvent::ToolCall { call: call.clone() };
                        if ctx.hook.after_model_event(&tc_event).await {
                            let _ = event_tx.send(tc_event).await;
                        }
                        good_calls.push(call);
                    }
                    Err(fe) => {
                        if !seen_ids.insert(fe.id.clone()) {
                            warn!(
                                tool_call_id = %fe.id,
                                "ignoring duplicate malformed tool call id"
                            );
                            continue;
                        }
                        warn!(
                            "Tool '{}' (id={}) JSON parse failed: {}",
                            fe.tool_name, fe.id, fe.error_message
                        );
                        let mut broken_call = ToolCall {
                            id: fe.id.clone(),
                            name: fe.tool_name.clone(),
                            arguments: serde_json::Value::Object(serde_json::Map::new()),
                            raw_arguments: Some(fe.raw_arguments.clone()),
                            provider_call_id: None,
                            metadata: serde_json::Map::new(),
                        };
                        stamp_tool_call_identity(&mut broken_call);
                        let tc_event = AgentEvent::ToolCall {
                            call: broken_call.clone(),
                        };
                        if ctx.hook.after_model_event(&tc_event).await {
                            let _ = event_tx.send(tc_event).await;
                        }
                        let error_result = ToolResult::error(
                            fe.id.clone(),
                            format!(
                                "Failed to parse tool arguments: {}\nRaw arguments: {}",
                                fe.error_message, fe.raw_arguments
                            ),
                        );
                        let _ = event_tx
                            .send(AgentEvent::ToolResult {
                                result: error_result.clone(),
                            })
                            .await;
                        errors.push((broken_call, error_result));
                    }
                }
            }
        }
        (good_calls, errors)
    };

    trace.mark("tool_calls_finalize_finished");
    Ok(IterationResult {
        model_call_id,
        input_token_estimate,
        text,
        text_messages,
        thinking,
        thinking_signature,
        tool_calls,
        finish_reason,
        usage,
        response_metadata,
        events: _events,
        parse_errors,
    })
}

#[allow(clippy::too_many_arguments)]
async fn persist_partial_model_response(
    ctx: &AgentContext,
    iteration: u32,
    model_call_id: &str,
    input_token_estimate: Option<&ModelInputTokenEstimate>,
    text: &str,
    text_messages: &[ModelDialogueItem],
    thinking: &str,
    thinking_signature: Option<&String>,
    total_usage_before: &TokenUsage,
    last_call_usage: Option<&TokenUsage>,
) -> Result<(), AgentError> {
    if text.is_empty() && thinking.is_empty() {
        return Ok(());
    }
    let partial = IterationResult {
        model_call_id: model_call_id.to_string(),
        input_token_estimate: input_token_estimate.cloned(),
        text: text.to_string(),
        text_messages: text_messages.to_vec(),
        thinking: thinking.to_string(),
        thinking_signature: thinking_signature.cloned(),
        tool_calls: Vec::new(),
        finish_reason: FinishReason::Unknown("incomplete".to_string()),
        usage: last_call_usage.cloned(),
        response_metadata: serde_json::Value::Null,
        events: Vec::new(),
        parse_errors: Vec::new(),
    };
    let message = build_assistant_message(&partial);
    let mut total_usage = total_usage_before.clone();
    if let Some(last_call_usage) = last_call_usage {
        total_usage.accumulate(last_call_usage);
    }
    ctx.hook
        .after_model_call(
            std::slice::from_ref(&message),
            iteration,
            &total_usage,
            last_call_usage,
            true,
        )
        .await
}

fn model_call_id(request: &ChatRequest) -> Result<String, AgentError> {
    request
        .metadata
        .get(crate::model_call_context::MODEL_CALL_ID_METADATA_KEY)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AgentError::Internal("model call id missing after projection".to_string()))
}

fn input_token_estimate(request: &ChatRequest) -> Option<ModelInputTokenEstimate> {
    request
        .metadata
        .get(MODEL_INPUT_TOKEN_ESTIMATE_METADATA_KEY)
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
}

pub async fn execute_tools(
    ctx: &AgentContext,
    tool_calls: &[ToolCall],
    iteration: u32,
    event_tx: &tokio::sync::mpsc::Sender<AgentEvent>,
) -> Result<Vec<ToolResult>, AgentError> {
    execute_tools_traced(
        ctx,
        tool_calls,
        iteration,
        event_tx,
        &CaptureTrace::default(),
    )
    .await
}

async fn execute_tools_traced(
    ctx: &AgentContext,
    tool_calls: &[ToolCall],
    iteration: u32,
    event_tx: &tokio::sync::mpsc::Sender<AgentEvent>,
    trace: &CaptureTrace,
) -> Result<Vec<ToolResult>, AgentError> {
    let mut results = Vec::new();
    let toolkit = ctx.current_toolkit();

    let max_parallel_tools = ctx.config.max_parallel_tools.max(1);
    let can_use_parallel = ctx.config.parallel_tool_calls && max_parallel_tools > 1;
    let mut idx = 0;

    while idx < tool_calls.len() {
        if can_use_parallel && tool_call_supports_parallel(&toolkit, &tool_calls[idx]) {
            let start = idx;
            while idx < tool_calls.len() && tool_call_supports_parallel(&toolkit, &tool_calls[idx])
            {
                idx += 1;
            }

            for batch in tool_calls[start..idx].chunks(max_parallel_tools) {
                if batch.len() == 1 {
                    let result = execute_single_tool_with_emit(
                        &batch[0],
                        &toolkit,
                        &ctx.hook,
                        &ctx.tool_ctx,
                        &ctx.config.tool_disclosure,
                        iteration,
                        &ctx.cancel,
                        ctx.run_control.as_ref(),
                        ctx.tool_before_boundary.as_ref(),
                        ctx.tool_observer_boundary.as_ref(),
                        ctx.permission_request_boundary.as_ref(),
                        ctx.config.tool_timeout(),
                        event_tx,
                        true,
                        trace.child("tool"),
                    )
                    .await?;
                    record_memory_tool_result(ctx, &batch[0].name, &result);
                    results.push(result);
                } else {
                    let mut batch_trace = trace.child("tool-batch");
                    batch_trace.set("tool_call_count", batch.len());
                    batch_trace.mark("dispatch_started");
                    let batch_results = execute_tools_parallel_batch(
                        ctx,
                        Arc::clone(&toolkit),
                        batch,
                        iteration,
                        event_tx,
                        trace,
                    )
                    .await?;
                    batch_trace.mark("tools_joined");
                    batch_trace.mark("result_emit_started");
                    for result in &batch_results {
                        emit_tool_result(event_tx, result).await;
                    }
                    batch_trace.mark("result_emitted");
                    for (call, result) in batch.iter().zip(batch_results.iter()) {
                        record_memory_tool_result(ctx, &call.name, result);
                    }
                    results.extend(batch_results);
                    batch_trace.finish("completed");
                }
            }
            continue;
        }

        if ctx.is_cancelled() {
            return Err(AgentError::Cancelled);
        }

        if let Some(result) = reject_memory_write_without_recent_search(ctx, &tool_calls[idx]) {
            let mut tool_trace = trace.child("tool");
            tool_trace.set("tool_call_id", tool_calls[idx].id.clone());
            tool_trace.set("tool_name", tool_calls[idx].name.clone());
            tool_trace.set("executor_invoked", false);
            tool_trace.mark("result_emit_started");
            emit_tool_result(event_tx, &result).await;
            tool_trace.mark("result_emitted");
            tool_trace.mark("after_tool_hook_started");
            ctx.hook
                .after_tool_call(&tool_calls[idx], &result, 0)
                .await?;
            tool_trace.mark("after_tool_hook_finished");
            tool_trace.finish("rejected");
            record_memory_tool_result(ctx, &tool_calls[idx].name, &result);
            results.push(result);
            idx += 1;
            continue;
        }

        let result = execute_single_tool_with_emit(
            &tool_calls[idx],
            &toolkit,
            &ctx.hook,
            &ctx.tool_ctx,
            &ctx.config.tool_disclosure,
            iteration,
            &ctx.cancel,
            ctx.run_control.as_ref(),
            ctx.tool_before_boundary.as_ref(),
            ctx.tool_observer_boundary.as_ref(),
            ctx.permission_request_boundary.as_ref(),
            ctx.config.tool_timeout(),
            event_tx,
            true,
            trace.child("tool"),
        )
        .await?;
        record_memory_tool_result(ctx, &tool_calls[idx].name, &result);
        results.push(result);
        idx += 1;
    }

    Ok(results)
}

const MEMORY_DISCIPLINE_WINDOW: usize = 5;
const MEMORY_SEARCH_TOOL: &str = "memory_search";
const MEMORY_WRITE_TOOL: &str = "memory_write";

fn record_memory_tool_result(ctx: &AgentContext, name: &str, result: &ToolResult) {
    let mut recent = ctx
        .memory_operation_recent_tools
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if recent.len() == MEMORY_DISCIPLINE_WINDOW {
        recent.pop_front();
    }
    recent.push_back((name.to_string(), !result.is_error));
}

fn reject_memory_write_without_recent_search(
    ctx: &AgentContext,
    call: &ToolCall,
) -> Option<ToolResult> {
    if call.name != MEMORY_WRITE_TOOL {
        return None;
    }
    let has_recent_search = ctx
        .memory_operation_recent_tools
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .any(|(name, succeeded)| name == MEMORY_SEARCH_TOOL && *succeeded);
    if has_recent_search {
        return None;
    }

    Some(ToolResult::text(
        call.id.clone(),
        "memory_write rejected: first call memory_search with compact keywords to check for existing related Memory. If a matching record exists, use memory_update instead of creating a duplicate. If no matching record exists, retry memory_write after the search.",
    ))
}

async fn execute_tools_parallel_batch(
    ctx: &AgentContext,
    toolkit: Arc<AgentToolkit>,
    tool_calls: &[ToolCall],
    iteration: u32,
    event_tx: &tokio::sync::mpsc::Sender<AgentEvent>,
    trace: &CaptureTrace,
) -> Result<Vec<ToolResult>, AgentError> {
    let mut pending = FuturesUnordered::new();
    let batch_cancel = ctx.cancel.child_token();

    for (index, call) in tool_calls.iter().enumerate() {
        let call = call.clone();
        let call_for_result = call.clone();
        let toolkit = Arc::clone(&toolkit);
        let hook = ctx.hook.clone();
        let tool_ctx = ctx.tool_ctx.clone();
        let cancel = batch_cancel.clone();
        let run_control = ctx.run_control.clone();
        let tool_before_boundary = ctx.tool_before_boundary.clone();
        let tool_observer_boundary = ctx.tool_observer_boundary.clone();
        let permission_request_boundary = ctx.permission_request_boundary.clone();
        let event_tx = event_tx.clone();
        let config_timeout = ctx.config.tool_timeout();
        let tool_disclosure = ctx.config.tool_disclosure.clone();
        let tool_trace = trace.child("tool");

        let handle = tokio::spawn(async move {
            execute_single_tool_with_emit(
                &call,
                &toolkit,
                &hook,
                &tool_ctx,
                &tool_disclosure,
                iteration,
                &cancel,
                run_control.as_ref(),
                tool_before_boundary.as_ref(),
                tool_observer_boundary.as_ref(),
                permission_request_boundary.as_ref(),
                config_timeout,
                &event_tx,
                false,
                tool_trace,
            )
            .await
        });
        pending.push(async move { (index, call_for_result, handle.await) });
    }

    let mut results = vec![None; tool_calls.len()];
    let mut terminal_error = None;
    while let Some((index, call, joined)) = pending.next().await {
        match joined {
            Ok(Ok(result)) => results[index] = Some(result),
            Ok(Err(e)) if e.is_cancelled() || e.is_fatal_tool_error() => {
                if terminal_error.is_none() {
                    terminal_error = Some(e);
                    batch_cancel.cancel();
                }
            }
            Ok(Err(e)) => {
                warn!("Tool '{}' failed: {}", call.name, e);
                results[index] = Some(ToolResult::error(call.id, e.to_string()));
            }
            Err(join_error) => {
                warn!("Tool '{}' task panicked: {}", call.name, join_error);
                results[index] = Some(ToolResult::error(
                    call.id,
                    format!("Tool '{}' panicked: {}", call.name, join_error),
                ));
            }
        }
    }

    match terminal_error {
        Some(error) => Err(error),
        None => Ok(results
            .into_iter()
            .map(|result| result.expect("every parallel tool task must produce a result"))
            .collect()),
    }
}

fn tool_call_supports_parallel(toolkit: &AgentToolkit, call: &ToolCall) -> bool {
    let Some(definition) = toolkit.get_definition(&call.name) else {
        return false;
    };
    tool_definition_supports_parallel(definition)
}

fn tool_definition_supports_parallel(definition: &ToolDefinition) -> bool {
    if let Some(value) = definition
        .metadata
        .get("supports_parallel_tool_calls")
        .and_then(serde_json::Value::as_bool)
    {
        return value;
    }

    let annotations = &definition.annotations;
    annotations.read_only == Some(true)
        && annotations.idempotent == Some(true)
        && annotations.destructive != Some(true)
        && annotations.requires_approval != Some(true)
}

async fn emit_tool_result(event_tx: &tokio::sync::mpsc::Sender<AgentEvent>, result: &ToolResult) {
    let _ = event_tx
        .send(AgentEvent::ToolResult {
            result: result.clone(),
        })
        .await;
}

#[allow(clippy::too_many_arguments)]
pub async fn execute_single_tool(
    call: &ToolCall,
    toolkit: &AgentToolkit,
    hook: &Arc<dyn AgentHook>,
    tool_ctx: &rua_core::tool::ToolContext,
    tool_disclosure: &ToolDisclosureConfig,
    iteration: u32,
    cancel: &CancellationToken,
    timeout: std::time::Duration,
    event_tx: &tokio::sync::mpsc::Sender<AgentEvent>,
) -> Result<ToolResult, AgentError> {
    execute_single_tool_with_product_boundary(
        call,
        toolkit,
        hook,
        tool_ctx,
        tool_disclosure,
        iteration,
        cancel,
        timeout,
        event_tx,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn execute_single_tool_with_product_boundary(
    call: &ToolCall,
    toolkit: &AgentToolkit,
    hook: &Arc<dyn AgentHook>,
    tool_ctx: &rua_core::tool::ToolContext,
    tool_disclosure: &ToolDisclosureConfig,
    iteration: u32,
    cancel: &CancellationToken,
    timeout: std::time::Duration,
    event_tx: &tokio::sync::mpsc::Sender<AgentEvent>,
    tool_before_boundary: Option<&Arc<dyn crate::ToolBeforeBoundary>>,
) -> Result<ToolResult, AgentError> {
    execute_single_tool_with_product_boundaries(
        call,
        toolkit,
        hook,
        tool_ctx,
        tool_disclosure,
        iteration,
        cancel,
        timeout,
        event_tx,
        tool_before_boundary,
        None,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn execute_single_tool_with_product_boundaries(
    call: &ToolCall,
    toolkit: &AgentToolkit,
    hook: &Arc<dyn AgentHook>,
    tool_ctx: &rua_core::tool::ToolContext,
    tool_disclosure: &ToolDisclosureConfig,
    iteration: u32,
    cancel: &CancellationToken,
    timeout: std::time::Duration,
    event_tx: &tokio::sync::mpsc::Sender<AgentEvent>,
    tool_before_boundary: Option<&Arc<dyn crate::ToolBeforeBoundary>>,
    tool_observer_boundary: Option<&Arc<dyn crate::ToolObserverBoundary>>,
    permission_request_boundary: Option<&Arc<dyn crate::PermissionRequestBoundary>>,
) -> Result<ToolResult, AgentError> {
    execute_single_tool_with_emit(
        call,
        toolkit,
        hook,
        tool_ctx,
        tool_disclosure,
        iteration,
        cancel,
        None,
        tool_before_boundary,
        tool_observer_boundary,
        permission_request_boundary,
        timeout,
        event_tx,
        true,
        CaptureTrace::default(),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn execute_single_tool_with_emit(
    call: &ToolCall,
    toolkit: &AgentToolkit,
    hook: &Arc<dyn AgentHook>,
    tool_ctx: &rua_core::tool::ToolContext,
    tool_disclosure: &ToolDisclosureConfig,
    iteration: u32,
    cancel: &CancellationToken,
    run_control: Option<&watch::Receiver<rua_core::RunControl>>,
    tool_before_boundary: Option<&Arc<dyn crate::ToolBeforeBoundary>>,
    tool_observer_boundary: Option<&Arc<dyn crate::ToolObserverBoundary>>,
    permission_request_boundary: Option<&Arc<dyn crate::PermissionRequestBoundary>>,
    timeout: std::time::Duration,
    event_tx: &tokio::sync::mpsc::Sender<AgentEvent>,
    emit_result_event: bool,
    mut trace: CaptureTrace,
) -> Result<ToolResult, AgentError> {
    trace.set("tool_call_id", call.id.clone());
    trace.set("tool_name", call.name.clone());
    trace.set("iteration", iteration);
    trace.set("executor_invoked", false);
    trace.mark("tool_started");
    let result = execute_single_tool_canonical(
        call,
        toolkit,
        hook,
        tool_ctx,
        tool_disclosure,
        iteration,
        cancel,
        run_control,
        tool_before_boundary,
        permission_request_boundary,
        timeout,
        event_tx,
        emit_result_event,
        &mut trace,
    )
    .await;
    trace.mark("canonical_tool_finished");
    if let Err(error) = &result {
        trace.finish(if error.is_cancelled() {
            "cancelled"
        } else {
            "error"
        });
    }
    let result = result?;
    if let Some(boundary) = tool_observer_boundary {
        let event = if result.is_error {
            crate::ToolObservationEvent::Failed
        } else {
            crate::ToolObservationEvent::After
        };
        if let Err(failure) = trace
            .measure_local(
                "product_observer_started",
                "product_observer_finished",
                boundary.observe(
                    crate::ToolObservationInvocation {
                        event,
                        call: call.clone(),
                        result: result.clone(),
                        agent_id: tool_ctx.agent_id.clone(),
                        session_id: tool_ctx.session_id.clone(),
                        working_directory: tool_ctx.working_dir.clone(),
                        metadata: tool_ctx.metadata.clone(),
                    },
                    cancel.clone(),
                ),
            )
            .await
        {
            warn!(diagnostic = %failure.diagnostic, "product Tool observer failed");
            trace.set("product_observer_failed", true);
        }
    }
    trace.finish(if result.is_error {
        "tool_error"
    } else {
        "completed"
    });
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
async fn execute_single_tool_canonical(
    call: &ToolCall,
    toolkit: &AgentToolkit,
    hook: &Arc<dyn AgentHook>,
    tool_ctx: &rua_core::tool::ToolContext,
    tool_disclosure: &ToolDisclosureConfig,
    iteration: u32,
    cancel: &CancellationToken,
    run_control: Option<&watch::Receiver<rua_core::RunControl>>,
    tool_before_boundary: Option<&Arc<dyn crate::ToolBeforeBoundary>>,
    permission_request_boundary: Option<&Arc<dyn crate::PermissionRequestBoundary>>,
    timeout: std::time::Duration,
    event_tx: &tokio::sync::mpsc::Sender<AgentEvent>,
    emit_result_event: bool,
    trace: &mut CaptureTrace,
) -> Result<ToolResult, AgentError> {
    match resolve_bridge_read_call(call, toolkit, tool_disclosure) {
        BridgeReadResolution::NotBridge => {}
        BridgeReadResolution::Resolved(result) => {
            let result = *result;
            if emit_result_event {
                trace.mark("result_emit_started");
                emit_tool_result(event_tx, &result).await;
                trace.mark("result_emitted");
            }
            return Ok(result);
        }
    }

    let effective_call = match resolve_bridge_tool_call(call, toolkit) {
        BridgeToolCallResolution::NotBridge => call.clone(),
        BridgeToolCallResolution::Unwrapped(unwrapped) => unwrapped,
        BridgeToolCallResolution::Rejected(result) => {
            if emit_result_event {
                trace.mark("result_emit_started");
                emit_tool_result(event_tx, &result).await;
                trace.mark("result_emitted");
            }
            return Ok(result);
        }
    };
    let mut effective_call = effective_call;
    if let Some(canonical) = toolkit.canonical_name(&effective_call.name) {
        if canonical != effective_call.name {
            effective_call.name = canonical.to_string();
        }
    }
    if let Some(boundary) = tool_before_boundary {
        let schema = toolkit
            .get_definition(&effective_call.name)
            .map(|definition| definition.parameters.clone())
            .unwrap_or(serde_json::Value::Null);
        let outcome = trace
            .measure_local(
                "product_before_started",
                "product_before_finished",
                boundary.apply(
                    crate::ToolBeforeInvocation {
                        call: effective_call.clone(),
                        schema,
                        agent_id: tool_ctx.agent_id.clone(),
                        session_id: tool_ctx.session_id.clone(),
                        working_directory: tool_ctx.working_dir.clone(),
                        metadata: tool_ctx.metadata.clone(),
                    },
                    cancel.clone(),
                ),
            )
            .await;
        effective_call = match outcome {
            Ok(crate::ToolBeforeOutcome::Continue(candidate)) => {
                let candidate = *candidate;
                if candidate.id != effective_call.id || candidate.name != effective_call.name {
                    let result = ToolResult::error(
                        effective_call.id.clone(),
                        "tool.before Hook produced an invalid candidate identity",
                    );
                    if emit_result_event {
                        trace.mark("result_emit_started");
                        emit_tool_result(event_tx, &result).await;
                        trace.mark("result_emitted");
                    }
                    return Ok(result);
                }
                let Some(definition) = toolkit.get_definition(&candidate.name) else {
                    let result = ToolResult::error(
                        candidate.id.clone(),
                        format!("Tool '{}' not found", candidate.name),
                    );
                    if emit_result_event {
                        trace.mark("result_emit_started");
                        emit_tool_result(event_tx, &result).await;
                        trace.mark("result_emitted");
                    }
                    return Ok(result);
                };
                if let Err(error) = rua_tool::validate_args(
                    &candidate.name,
                    &candidate.arguments,
                    &definition.parameters,
                ) {
                    let result = ToolResult::error(
                        candidate.id.clone(),
                        format!("tool.before Hook produced invalid Tool arguments: {error}"),
                    );
                    if emit_result_event {
                        trace.mark("result_emit_started");
                        emit_tool_result(event_tx, &result).await;
                        trace.mark("result_emitted");
                    }
                    return Ok(result);
                }
                candidate
            }
            Ok(crate::ToolBeforeOutcome::Reject { reason }) => {
                let result = ToolResult::error(
                    effective_call.id.clone(),
                    format!(
                        "Tool '{}' rejected by tool.before Hook: {reason}",
                        effective_call.name
                    ),
                );
                if emit_result_event {
                    trace.mark("result_emit_started");
                    emit_tool_result(event_tx, &result).await;
                    trace.mark("result_emitted");
                }
                return Ok(result);
            }
            Err(failure) => {
                let result = ToolResult::error(
                    effective_call.id.clone(),
                    format!(
                        "Tool '{}' failed before execution because a tool.before Hook failed: {}",
                        effective_call.name, failure.diagnostic
                    ),
                );
                if emit_result_event {
                    trace.mark("result_emit_started");
                    emit_tool_result(event_tx, &result).await;
                    trace.mark("result_emitted");
                }
                return Ok(result);
            }
        };
    }
    let call = &effective_call;
    trace.set("effective_tool_name", call.name.clone());

    debug!(
        tool_call_id = %call.id,
        tool_name = %call.name,
        arguments = %call.arguments,
        raw_arguments = ?call.raw_arguments,
        "execute_single_tool: received tool call"
    );

    let decision = trace
        .measure_local(
            "before_tool_hook_started",
            "before_tool_hook_finished",
            hook.before_tool_call(call, iteration),
        )
        .await;
    let decision = decision?;
    match decision {
        ToolDecision::Allow | ToolDecision::AuditOnly { .. } => {}
        ToolDecision::Deny { reason } => {
            let result = ToolResult::error(
                call.id.clone(),
                format!("Tool '{}' denied: {reason}", call.name),
            );
            if emit_result_event {
                trace.mark("result_emit_started");
                emit_tool_result(event_tx, &result).await;
                trace.mark("result_emitted");
            }
            let hooked = trace
                .measure_local(
                    "after_tool_hook_started",
                    "after_tool_hook_finished",
                    hook.after_tool_call(call, &result, 0),
                )
                .await;
            hooked?;
            return Ok(result);
        }
        ToolDecision::RequiresApproval { request_id, prompt } => {
            let permission_decision = trace
                .measure_local(
                    "permission_hook_started",
                    "permission_hook_finished",
                    async {
                        match permission_request_boundary {
                            Some(boundary) => boundary
                                .decide(
                                    crate::PermissionRequestInvocation {
                                        request_id: request_id.clone(),
                                        prompt: prompt.clone(),
                                        call: call.clone(),
                                        argument_digest: crate::argument_digest(call),
                                        agent_id: tool_ctx.agent_id.clone(),
                                        session_id: tool_ctx.session_id.clone(),
                                        working_directory: tool_ctx.working_dir.clone(),
                                        metadata: tool_ctx.metadata.clone(),
                                    },
                                    cancel.clone(),
                                )
                                .await
                                .unwrap_or(crate::PermissionHookDecision::NoDecision),
                            None => crate::PermissionHookDecision::NoDecision,
                        }
                    },
                )
                .await;
            if !matches!(
                permission_decision,
                crate::PermissionHookDecision::NoDecision
            ) {
                trace.mark("permission_decision_record_started");
                let recorded = hook
                    .record_permission_hook_decision(
                        &request_id,
                        &prompt,
                        call,
                        &permission_decision,
                    )
                    .await;
                trace.mark("permission_decision_record_finished");
                match recorded {
                    crate::hooks::ApprovalResult::Allow => {}
                    crate::hooks::ApprovalResult::AllowWithModifiedArgs { .. } => {
                        let result = ToolResult::error(
                            call.id.clone(),
                            "permission Hook decisions cannot mutate Tool arguments",
                        );
                        if emit_result_event {
                            trace.mark("result_emit_started");
                            emit_tool_result(event_tx, &result).await;
                            trace.mark("result_emitted");
                        }
                        return Ok(result);
                    }
                    crate::hooks::ApprovalResult::Deny { reason } => {
                        let result = ToolResult::error(call.id.clone(), reason);
                        if emit_result_event {
                            trace.mark("result_emit_started");
                            emit_tool_result(event_tx, &result).await;
                            trace.mark("result_emitted");
                        }
                        let hooked = trace
                            .measure_local(
                                "after_tool_hook_started",
                                "after_tool_hook_finished",
                                hook.after_tool_call(call, &result, 0),
                            )
                            .await;
                        hooked?;
                        return Ok(result);
                    }
                }
            } else {
                trace.mark("approval_pending_emit_started");
                let _ = event_tx
                    .send(AgentEvent::ToolApprovalPending {
                        call_id: call.id.clone(),
                        tool_name: call.name.clone(),
                        request_id: (!request_id.is_empty()).then_some(request_id.clone()),
                        metadata: serde_json::json!({ "prompt": prompt }),
                    })
                    .await;
                trace.mark("approval_pending_emitted");
                trace.checkpoint("approval_wait_ready");
                trace.mark("approval_wait_started");
                let approval = hook
                    .wait_for_approval(&call.id, &call.name, &prompt, call)
                    .await;
                trace.mark("approval_wait_finished");
                match approval {
                    ApprovalResult::Allow => {}
                    ApprovalResult::Deny { reason } => {
                        let result = ToolResult::error(
                            call.id.clone(),
                            format!("Tool '{}' denied: {reason}", call.name),
                        );
                        if emit_result_event {
                            trace.mark("result_emit_started");
                            emit_tool_result(event_tx, &result).await;
                            trace.mark("result_emitted");
                        }
                        let hooked = trace
                            .measure_local(
                                "after_tool_hook_started",
                                "after_tool_hook_finished",
                                hook.after_tool_call(call, &result, 0),
                            )
                            .await;
                        hooked?;
                        return Ok(result);
                    }
                    ApprovalResult::AllowWithModifiedArgs { args } => {
                        let mut modified_call = call.clone();
                        modified_call.arguments = args;
                        return execute_approved_tool(
                            &modified_call,
                            toolkit,
                            hook,
                            tool_ctx,
                            cancel,
                            run_control,
                            timeout,
                            event_tx,
                            emit_result_event,
                            trace,
                        )
                        .await;
                    }
                }
            }
        }
    }

    execute_approved_tool(
        call,
        toolkit,
        hook,
        tool_ctx,
        cancel,
        run_control,
        timeout,
        event_tx,
        emit_result_event,
        trace,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn execute_approved_tool(
    call: &ToolCall,
    toolkit: &AgentToolkit,
    hook: &Arc<dyn AgentHook>,
    tool_ctx: &rua_core::tool::ToolContext,
    cancel: &CancellationToken,
    run_control: Option<&watch::Receiver<rua_core::RunControl>>,
    timeout: std::time::Duration,
    event_tx: &tokio::sync::mpsc::Sender<AgentEvent>,
    emit_result_event: bool,
    trace: &mut CaptureTrace,
) -> Result<ToolResult, AgentError> {
    let executor = match toolkit.get_executor(&call.name) {
        Some(e) => e,
        None => {
            warn!("Tool '{}' not found in toolkit", call.name);
            let result =
                ToolResult::error(call.id.clone(), format!("Tool '{}' not found", call.name));
            if emit_result_event {
                trace.mark("result_emit_started");
                emit_tool_result(event_tx, &result).await;
                trace.mark("result_emitted");
            }
            let hooked = trace
                .measure_local(
                    "after_tool_hook_started",
                    "after_tool_hook_finished",
                    hook.after_tool_call(call, &result, 0),
                )
                .await;
            hooked?;
            return Ok(result);
        }
    };

    let mut enriched_ctx = tool_ctx.clone();
    enriched_ctx.metadata.insert(
        "tool_use_id".to_string(),
        serde_json::Value::String(call.id.clone()),
    );
    let transfer_contract = toolkit
        .get_definition(&call.name)
        .map(rua_tool::tool_transfer_contract)
        .unwrap_or_default();
    let already_task_owned = tool_ctx.metadata.contains_key("task_id");
    let transfer_eligible = !already_task_owned
        && transfer_contract.policy == rua_tool::ToolTransferPolicy::Transferable;
    if !transfer_eligible {
        enriched_ctx.event_tx = Some(event_tx.clone());
    }
    let execution_cancel = CancellationToken::new();
    let (execution_cancel_tx, execution_cancel_rx) = watch::channel(false);
    enriched_ctx.cancel = Some(execution_cancel_rx);
    let execution_cancel_for_bridge = execution_cancel.clone();
    let execution_cancel_tx_for_bridge = execution_cancel_tx.clone();
    let mut execution_cancel_bridge = Some(tokio::spawn(async move {
        execution_cancel_for_bridge.cancelled().await;
        let _ = execution_cancel_tx_for_bridge.send(true);
    }));
    let parent_cancel = cancel.clone();
    let execution_cancel_from_parent = execution_cancel.clone();
    let mut parent_cancel_bridge = Some(tokio::spawn(async move {
        parent_cancel.cancelled().await;
        execution_cancel_from_parent.cancel();
    }));

    let start = Instant::now();
    let foreground_wait = transfer_eligible
        .then(|| {
            toolkit
                .get_definition(&call.name)
                .and_then(|definition| rua_tool::foreground_wait_limit(definition, &call.arguments))
        })
        .flatten();
    let transfer_available =
        transfer_eligible && rua_tool::extended::task_tools::get_task_runtime(tool_ctx).is_some();
    let reserved_task_id = uuid::Uuid::new_v4().to_string();
    let task_session = if transfer_available {
        let runtime = rua_tool::extended::task_tools::get_task_runtime(tool_ctx)
            .expect("checked Task Runtime");
        Some(
            runtime
                .prepare_tool_session(
                    &rua_tool::extended::task_tools::task_owner_from_context(tool_ctx),
                    &reserved_task_id,
                )
                .map_err(|error| AgentError::Internal(error.to_string()))?,
        )
    } else {
        None
    };
    if let Some(session) = task_session.as_ref() {
        enriched_ctx.task_session = Some(Arc::new(
            rua_tool::extended::task_tools::TaskRunnerSessionHandle::new(Arc::clone(session)),
        ));
    }
    let executor_for_exec = executor.clone();
    let call_for_exec = call.clone();
    let enriched_ctx_for_exec = enriched_ctx.clone();
    let mut tool_future: RunningToolFuture = if transfer_eligible || timeout.is_zero() {
        Box::pin(async move {
            Ok(executor_for_exec
                .execute(&call_for_exec, &enriched_ctx_for_exec)
                .await)
        })
    } else {
        Box::pin(async move {
            tokio::time::timeout(
                timeout,
                executor_for_exec.execute(&call_for_exec, &enriched_ctx_for_exec),
            )
            .await
        })
    };
    let foreground_wait = async move {
        match foreground_wait {
            Some(duration) => tokio::time::sleep(duration).await,
            None => std::future::pending::<()>().await,
        }
    };
    tokio::pin!(foreground_wait);
    let mut run_control = run_control.cloned();

    trace.checkpoint("executor_wait_started");
    trace.set("executor_invoked", true);
    trace.mark("executor_started");
    let exec_result = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            execution_cancel.cancel();
            let _ = execution_cancel_tx.send(true);
            Err(AgentError::Cancelled)
        }
        _ = wait_for_interjection(&mut run_control) => {
            if transfer_available {
                trace.mark("executor_foreground_finished");
                trace.set("execution_transferred", true);
                trace.checkpoint("background_transfer_started");
                return transfer_running_tool_to_background(
                    reserved_task_id.clone(),
                    tool_future,
                    call,
                    hook,
                    tool_ctx,
                    start,
                    event_tx,
                    emit_result_event,
                    execution_cancel.clone(),
                    parent_cancel_bridge
                        .take()
                        .expect("parent tool cancellation bridge"),
                    execution_cancel_bridge
                        .take()
                        .expect("tool execution cancellation bridge"),
                    task_session.clone(),
                    trace,
                )
                .await;
            }
            if call.name.starts_with("Task_") {
                trace.mark("executor_foreground_finished");
                trace.set("task_wait_detached", true);
                if let Some(parent_cancel_bridge) = parent_cancel_bridge.take() {
                    parent_cancel_bridge.abort();
                }
                if let Some(execution_cancel_bridge) = execution_cancel_bridge.take() {
                    execution_cancel_bridge.abort();
                }
                let mut result = ToolResult::text(
                    call.id.clone(),
                    "Task waiter detached after interjection; underlying Tasks continue running.",
                );
                result.name = Some(call.name.clone());
                result.metadata.insert(
                    "task_wait_detached".to_string(),
                    serde_json::Value::Bool(true),
                );
                if emit_result_event {
                    trace.mark("result_emit_started");
                    emit_tool_result(event_tx, &result).await;
                    trace.mark("result_emitted");
                }
                let hooked = trace.measure_local("after_tool_hook_started", "after_tool_hook_finished", hook
                    .after_tool_call(call, &result, start.elapsed().as_millis() as u64)).await;
                hooked?;
                return Ok(result);
            }
            tokio::select! {
                _ = cancel.cancelled() => Err(AgentError::Cancelled),
                result = &mut tool_future => match result {
                    Ok(Ok(r)) => Ok(r),
                    Ok(Err(e)) => Err(e),
                    Err(_) => {
                        execution_cancel.cancel();
                        let _ = execution_cancel_tx.send(true);
                        Err(AgentError::Timeout {
                            timeout_secs: timeout.as_secs(),
                        })
                    },
                },
            }
        }
        result = &mut tool_future => {
            match result {
                Ok(Ok(r)) => Ok(r),
                Ok(Err(e)) => Err(e),
                Err(_) => {
                    execution_cancel.cancel();
                    let _ = execution_cancel_tx.send(true);
                    Err(AgentError::Timeout {
                        timeout_secs: timeout.as_secs(),
                    })
                },
            }
        }
        _ = &mut foreground_wait => {
            if transfer_available {
                trace.mark("executor_foreground_finished");
                trace.set("execution_transferred", true);
                trace.checkpoint("background_transfer_started");
                return transfer_running_tool_to_background(
                    reserved_task_id.clone(),
                    tool_future,
                    call,
                    hook,
                    tool_ctx,
                    start,
                    event_tx,
                    emit_result_event,
                    execution_cancel.clone(),
                    parent_cancel_bridge
                        .take()
                        .expect("parent tool cancellation bridge"),
                    execution_cancel_bridge
                        .take()
                        .expect("tool execution cancellation bridge"),
                    task_session.clone(),
                    trace,
                )
                .await;
            }
            execution_cancel.cancel();
            let _ = execution_cancel_tx.send(true);
            Err(AgentError::Tool {
                tool: call.name.clone(),
                message: "transferable Tool reached its foreground wait limit without a Task Runtime"
                    .to_string(),
            })
        }
    };
    trace.mark("executor_finished");
    if let Some(parent_cancel_bridge) = parent_cancel_bridge.take() {
        parent_cancel_bridge.abort();
    }
    if let Some(execution_cancel_bridge) = execution_cancel_bridge.take() {
        execution_cancel_bridge.abort();
    }

    let duration_ms = start.elapsed().as_millis() as u64;

    let mut result = match exec_result {
        Ok(result) => result,
        Err(AgentError::Cancelled) => {
            return Err(AgentError::Cancelled);
        }
        Err(ref e) if e.is_fatal_tool_error() => {
            return Err(exec_result.unwrap_err());
        }
        Err(e) => ToolResult::error(call.id.clone(), e.to_string()),
    };

    trace.mark("output_processing_started");
    let compacted = rua_tool::ToolResultOutputProcessor::from_tool_context(tool_ctx)
        .process(&call.id, &call.name, &call.arguments, &mut result)
        .await;
    trace.mark("output_processing_finished");
    if compacted {
        debug!(
            "ToolResultOutputProcessor compacted output for tool {:?}",
            result.name
        );
    }

    if emit_result_event {
        trace.mark("result_emit_started");
        emit_tool_result(event_tx, &result).await;
        trace.mark("result_emitted");
    }

    let hooked = trace
        .measure_local(
            "after_tool_hook_started",
            "after_tool_hook_finished",
            hook.after_tool_call(call, &result, duration_ms),
        )
        .await;
    hooked?;

    Ok(result)
}

#[allow(clippy::too_many_arguments)]
async fn transfer_running_tool_to_background(
    task_id: String,
    running_tool: RunningToolFuture,
    call: &ToolCall,
    hook: &Arc<dyn AgentHook>,
    tool_ctx: &rua_core::tool::ToolContext,
    start: Instant,
    event_tx: &tokio::sync::mpsc::Sender<AgentEvent>,
    emit_result_event: bool,
    execution_cancel: CancellationToken,
    parent_cancel_bridge: tokio::task::JoinHandle<()>,
    execution_cancel_bridge: tokio::task::JoinHandle<()>,
    task_session: Option<Arc<rua_tool::TaskRunnerSession>>,
    trace: &mut CaptureTrace,
) -> Result<ToolResult, AgentError> {
    let Some(task_runtime) = rua_tool::extended::task_tools::get_task_runtime(tool_ctx) else {
        parent_cancel_bridge.abort();
        execution_cancel.cancel();
        return Err(AgentError::Tool {
            tool: call.name.clone(),
            message: "foreground tool transfer requires a Task Runtime".to_string(),
        });
    };

    let task_owner = rua_tool::extended::task_tools::task_owner_from_context(tool_ctx);
    let origin_tool_call_id = call.id.clone();
    let origin_turn_id = tool_ctx
        .metadata
        .get("turn_id")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let background_call = call.clone();
    let background_hook = Arc::clone(hook);
    let adopted_tool_ctx = tool_ctx.clone();

    let background_result = Box::pin(async move {
        let duration_ms = start.elapsed().as_millis() as u64;
        let mut result = match running_tool.await {
            Ok(Ok(mut result)) => {
                result.duration_ms = result.duration_ms.or(Some(duration_ms));
                result
            }
            Ok(Err(e)) => {
                if e.is_cancelled() {
                    ToolResult::error(background_call.id.clone(), "Tool execution was cancelled.")
                } else {
                    ToolResult::error(background_call.id.clone(), e.to_string())
                }
            }
            Err(_) => ToolResult::error(background_call.id.clone(), "Tool execution timed out."),
        };

        let compacted = rua_tool::ToolResultOutputProcessor::from_tool_context(&adopted_tool_ctx)
            .process(
                &background_call.id,
                &background_call.name,
                &background_call.arguments,
                &mut result,
            )
            .await;
        if compacted {
            debug!(
                "ToolResultOutputProcessor compacted backgrounded output for tool {:?}",
                result.name
            );
        }

        let hook_result = background_hook
            .after_tool_call(&background_call, &result, duration_ms)
            .await;
        if let Err(e) = hook_result {
            warn!("backgrounded tool hook failed: {e}");
        }
        execution_cancel_bridge.abort();
        result
    });

    let adoption = rua_tool::adopt_running_tool(
        task_runtime,
        rua_tool::AdoptRunningToolRequest {
            task_id: task_id.clone(),
            owner: task_owner,
            authorization_agent_id: tool_ctx.agent_id.clone(),
            configuration_epoch: tool_ctx
                .metadata
                .get("chat_configuration_epoch")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            origin_turn_id,
            origin_tool_call_id: origin_tool_call_id.clone(),
            tool_name: call.name.clone(),
            arguments: call.arguments.clone(),
            instruction: format!("Foreground tool {} ({origin_tool_call_id})", call.name),
            result: background_result,
            cancel: execution_cancel,
            session: task_session,
        },
    )
    .await;
    trace.mark("background_transfer_finished");
    parent_cancel_bridge.abort();
    adoption.map_err(|e| AgentError::Tool {
        tool: call.name.clone(),
        message: format!("failed to register background Task: {e}"),
    })?;

    let mut result = ToolResult::text(
        call.id.clone(),
        format!(
            "Tool call moved to Background Tool Task. task_id: {task_id}. The original execution is continuing once in the background."
        ),
    );
    result.name = Some(call.name.clone());
    result.metadata.insert(
        "background_task_id".to_string(),
        serde_json::Value::String(task_id.clone()),
    );
    result.metadata.insert(
        "origin_tool_call_id".to_string(),
        serde_json::Value::String(call.id.clone()),
    );
    result.metadata.insert(
        "owner_agent_id".to_string(),
        serde_json::Value::String(tool_ctx.agent_id.clone()),
    );
    result.metadata.insert(
        "owner_conversation_id".to_string(),
        serde_json::Value::String(tool_ctx.session_id.clone()),
    );

    if emit_result_event {
        trace.mark("result_emit_started");
        emit_tool_result(event_tx, &result).await;
        trace.mark("result_emitted");
    }

    Ok(result)
}

const MEDIA_PLACEHOLDER: &str = "[Image was here but current model doesn't support multimodal]";
const AUDIO_PLACEHOLDER: &str = "[Audio was here but current model doesn't support multimodal]";
const VIDEO_PLACEHOLDER: &str = "[Video was here but current model doesn't support multimodal]";

pub fn has_media_blocks(messages: &[ModelDialogueItem]) -> bool {
    messages
        .iter()
        .any(|msg| msg.content.iter().any(content_part_has_media))
}

pub fn is_multimodal_error_message(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    let has_media_keyword = lower.contains("image")
        || lower.contains("vision")
        || lower.contains("multimodal")
        || lower.contains("image_url");
    let has_rejection = lower.contains("not support")
        || lower.contains("not available")
        || lower.contains("invalid content")
        || (lower.contains("unknown variant") && lower.contains("expected"))
        || lower.contains("not allowed")
        || lower.contains("could not process")
        || lower.contains("cannot process")
        || lower.contains("not accept")
        || lower.contains("is only supported");
    has_media_keyword && has_rejection
}

pub fn is_multimodal_error(err: &AgentError) -> bool {
    match err {
        AgentError::Model(llm_err) => is_multimodal_error_message(&llm_err.to_string()),
        AgentError::Internal(msg) => is_multimodal_error_message(msg),
        _ => false,
    }
}

pub fn strip_media_blocks_from_memory(messages: &mut [ModelDialogueItem]) -> usize {
    let mut stripped_count: usize = 0;
    for msg in messages.iter_mut() {
        if !msg.content.iter().any(content_part_has_media) {
            continue;
        }

        msg.content = strip_media_parts(std::mem::take(&mut msg.content), &mut stripped_count);
    }
    stripped_count
}

fn content_part_has_media(part: &ContentPart) -> bool {
    match part {
        ContentPart::Image { .. } | ContentPart::Audio { .. } | ContentPart::Video { .. } => true,
        ContentPart::ToolResult { result } => result.content.iter().any(content_part_has_media),

        _ => false,
    }
}

fn strip_media_parts(parts: Vec<ContentPart>, stripped_count: &mut usize) -> Vec<ContentPart> {
    parts
        .into_iter()
        .map(|part| match part {
            ContentPart::Image { .. } => {
                *stripped_count += 1;
                ContentPart::Text {
                    text: MEDIA_PLACEHOLDER.to_string(),
                }
            }
            ContentPart::Audio { .. } => {
                *stripped_count += 1;
                ContentPart::Text {
                    text: AUDIO_PLACEHOLDER.to_string(),
                }
            }
            ContentPart::Video { .. } => {
                *stripped_count += 1;
                ContentPart::Text {
                    text: VIDEO_PLACEHOLDER.to_string(),
                }
            }
            ContentPart::ToolResult { mut result } => {
                if result.content.iter().any(content_part_has_media) {
                    result.content = strip_media_parts(result.content, stripped_count);
                }
                ContentPart::ToolResult { result }
            }
            other => other,
        })
        .collect()
}

pub fn build_assistant_message(result: &IterationResult) -> ModelDialogueItem {
    let mut content = Vec::new();

    if !result.thinking.is_empty() {
        content.push(ContentPart::Thinking {
            text: result.thinking.clone(),
            signature: result.thinking_signature.clone(),
        });
    }

    let mut message_parts = Vec::new();
    if !result.text_messages.iter().any(|message| {
        message.metadata.contains_key(ASSISTANT_PHASE_METADATA_KEY)
            || message
                .metadata
                .contains_key(PROVIDER_MESSAGE_ID_METADATA_KEY)
    }) {
        if !result.text.is_empty() {
            content.push(ContentPart::Text {
                text: result.text.clone(),
            });
        }
    } else {
        for message in &result.text_messages {
            if message.text_content().is_empty() {
                continue;
            }
            message_parts.push(AssistantMessagePart {
                text_index: message_parts.len(),
                metadata: message.metadata.clone(),
            });
            content.extend(message.content.clone());
        }
    }

    for call in &result.tool_calls {
        content.push(ContentPart::ToolUse { call: call.clone() });
    }

    for (call, _) in &result.parse_errors {
        content.push(ContentPart::ToolUse { call: call.clone() });
    }

    let mut metadata = serde_json::Map::new();
    metadata.insert(
        MODEL_DIALOGUE_ITEM_ID_METADATA_KEY.to_string(),
        serde_json::Value::String(result.model_call_id.clone()),
    );
    if !result.thinking.is_empty() {
        metadata.insert(
            REASONING_MESSAGE_ID_METADATA_KEY.to_string(),
            serde_json::Value::String(reasoning_message_id(&result.model_call_id)),
        );
    }
    if !result.text.is_empty() {
        metadata.insert(
            TEXT_MESSAGE_ID_METADATA_KEY.to_string(),
            serde_json::Value::String(text_message_id(&result.model_call_id)),
        );
    }
    if !message_parts.is_empty() {
        metadata.insert(
            ASSISTANT_MESSAGE_PARTS_METADATA_KEY.into(),
            serde_json::to_value(message_parts).expect("assistant message metadata"),
        );
    }
    if let Some(input_token_estimate) = result.input_token_estimate.as_ref() {
        metadata.insert(
            MODEL_INPUT_TOKEN_ESTIMATE_METADATA_KEY.to_string(),
            serde_json::to_value(input_token_estimate).unwrap_or_default(),
        );
    }
    if let Some(response_metadata) = result.response_metadata.as_object() {
        for (key, value) in response_metadata {
            metadata.insert(key.clone(), value.clone());
        }
    }

    ModelDialogueItem {
        id: None,
        role: Role::Assistant,
        content,
        name: None,
        created_at: None,
        metadata,
    }
}

fn append_text_message(
    messages: &mut Vec<ModelDialogueItem>,
    delta: &str,
    metadata: &serde_json::Value,
) {
    let message_id = metadata
        .get(CANONICAL_MESSAGE_ID_METADATA_KEY)
        .and_then(serde_json::Value::as_str)
        .expect("text event has canonical identity");
    let index = messages
        .iter()
        .position(|message| message.id.as_deref() == Some(message_id))
        .unwrap_or_else(|| {
            let mut message = ModelDialogueItem::text(Role::Assistant, "");
            message.id = Some(message_id.to_string());
            message.metadata.insert(
                TEXT_MESSAGE_ID_METADATA_KEY.into(),
                serde_json::json!(message_id),
            );
            messages.push(message);
            messages.len() - 1
        });
    let message = &mut messages[index];
    if let ContentPart::Text { text } = &mut message.content[0] {
        text.push_str(delta);
    }
    for key in [
        ASSISTANT_PHASE_METADATA_KEY,
        PROVIDER_MESSAGE_ID_METADATA_KEY,
    ] {
        if let Some(value) = metadata.get(key) {
            message.metadata.insert(key.into(), value.clone());
        }
    }
}

fn with_canonical_message_id(mut event: AgentEvent, message_id: &str) -> AgentEvent {
    let metadata = match &mut event {
        AgentEvent::TextDelta { metadata, .. }
        | AgentEvent::ThinkingDelta { metadata, .. }
        | AgentEvent::ToolCallDelta { metadata, .. }
        | AgentEvent::ToolResultDelta { metadata, .. } => Some(metadata),
        _ => None,
    };
    if let Some(metadata) = metadata {
        if !metadata.is_object() {
            *metadata = serde_json::Value::Object(serde_json::Map::new());
        }
        metadata
            .as_object_mut()
            .expect("event metadata was normalized to an object")
            .insert(
                CANONICAL_MESSAGE_ID_METADATA_KEY.to_string(),
                serde_json::Value::String(message_id.to_string()),
            );
    }
    event
}

fn assign_model_output_identity(
    mut event: AgentEvent,
    model_call_id: &str,
    tool_call_ids_by_index: &mut HashMap<usize, String>,
) -> AgentEvent {
    let message_id = match &mut event {
        AgentEvent::TextDelta { metadata, .. } => {
            let source = metadata
                .get(PROVIDER_MESSAGE_ID_METADATA_KEY)
                .and_then(serde_json::Value::as_str);
            Some(source.map_or_else(
                || text_message_id(model_call_id),
                |source| format!("{}-{source}", text_message_id(model_call_id)),
            ))
        }
        AgentEvent::ThinkingDelta { .. } => Some(reasoning_message_id(model_call_id)),
        AgentEvent::ToolCallDelta { id, index, .. } => {
            if !id.is_empty() {
                tool_call_ids_by_index.insert(*index, id.clone());
            } else if let Some(resolved_id) = tool_call_ids_by_index.get(index) {
                *id = resolved_id.clone();
            }
            (!id.is_empty()).then(|| tool_use_message_id(id))
        }
        AgentEvent::ToolResultDelta { id, .. } => {
            (!id.is_empty()).then(|| tool_result_message_id(id))
        }
        AgentEvent::ToolCall { call } => {
            stamp_tool_call_identity(call);
            None
        }
        _ => None,
    };
    match message_id {
        Some(message_id) => with_canonical_message_id(event, &message_id),
        None => event,
    }
}

fn stamp_tool_call_identity(call: &mut ToolCall) {
    call.metadata.insert(
        CANONICAL_MESSAGE_ID_METADATA_KEY.to_string(),
        serde_json::Value::String(tool_use_message_id(&call.id)),
    );
}

async fn forward_captured_usage_if_needed(
    ctx: &AgentContext,
    event_tx: &tokio::sync::mpsc::Sender<AgentEvent>,
    usage: Option<&TokenUsage>,
    provider_usage_event_seen: bool,
) {
    if provider_usage_event_seen {
        return;
    }
    let Some(usage) = usage else {
        return;
    };
    let event = AgentEvent::Usage {
        usage: usage.clone(),
    };
    if ctx.hook.after_model_event(&event).await {
        let _ = event_tx.send(event).await;
    }
}

async fn emit_usage_and_end(
    event_tx: &tokio::sync::mpsc::Sender<AgentEvent>,
    usage: &TokenUsage,
    finish_reason: FinishReason,
) {
    if !usage.is_empty() {
        let _ = event_tx
            .send(AgentEvent::Usage {
                usage: usage.clone(),
            })
            .await;
    }
    let _ = event_tx
        .send(AgentEvent::End {
            finish_reason,
            usage: if usage.is_empty() {
                None
            } else {
                Some(usage.clone())
            },
            metadata: serde_json::Value::Null,
        })
        .await;
}
