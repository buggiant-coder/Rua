use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use agent_types::{ContentPart, ModelDialogueItem, Role};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::context_checker::{HeuristicEstimator, TokenEstimator};
use crate::error::{AgentError, AgentErrorKind};
use crate::reactive_compaction::ContextOverflowDetector;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobileCompactionPolicy {
    #[serde(default = "default_true")]
    pub allow_llm_summary_foreground: bool,

    #[serde(default)]
    pub allow_llm_summary_background: bool,

    #[serde(default = "default_true")]
    pub require_charging_for_background_summary: bool,

    #[serde(default = "default_summary_timeout")]
    pub max_summary_runtime: Duration,

    #[serde(default = "default_mobile_inline_bytes")]
    pub max_tool_result_inline_bytes: usize,
}

fn default_true() -> bool {
    true
}

fn default_summary_timeout() -> Duration {
    Duration::from_secs(30)
}

fn default_mobile_inline_bytes() -> usize {
    4096
}

impl Default for MobileCompactionPolicy {
    fn default() -> Self {
        Self {
            allow_llm_summary_foreground: true,
            allow_llm_summary_background: false,
            require_charging_for_background_summary: true,
            max_summary_runtime: Duration::from_secs(30),
            max_tool_result_inline_bytes: 4096,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceState {
    Foreground,

    Background,

    Charging,

    ChargingWifi,
}

impl MobileCompactionPolicy {
    pub fn allow_summary(&self, state: &DeviceState) -> bool {
        match state {
            DeviceState::Foreground => self.allow_llm_summary_foreground,
            DeviceState::Background => self.allow_llm_summary_background,
            DeviceState::Charging => {
                self.allow_llm_summary_background || !self.require_charging_for_background_summary
            }
            DeviceState::ChargingWifi => true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionConfig {
    #[serde(default = "default_keep_last_turns")]
    pub keep_last_turns: usize,

    #[serde(default = "default_soft_token_threshold")]
    pub soft_token_threshold: usize,

    #[serde(default = "default_summary_max_tokens")]
    pub summary_max_tokens: usize,

    #[serde(default = "default_true")]
    pub enable_llm_summary: bool,

    #[serde(default = "default_reserve_ratio")]
    pub reserve_ratio: f64,

    #[serde(default)]
    pub reserve_tokens: usize,

    #[serde(default = "default_true")]
    pub compact_with_thinking_block: bool,

    #[serde(default)]
    pub thinking_turns_to_keep: i32,

    #[serde(default = "default_true")]
    pub compact_enhanced_prompt: bool,

    #[serde(default)]
    pub mobile_policy: MobileCompactionPolicy,
}

fn default_keep_last_turns() -> usize {
    4
}

fn default_soft_token_threshold() -> usize {
    100_000
}

fn default_summary_max_tokens() -> usize {
    2000
}

fn default_reserve_ratio() -> f64 {
    0.1
}

pub const DEFAULT_SUMMARY_TIMEOUT: Duration = Duration::from_secs(30);

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            keep_last_turns: default_keep_last_turns(),
            soft_token_threshold: default_soft_token_threshold(),
            summary_max_tokens: default_summary_max_tokens(),
            enable_llm_summary: true,
            reserve_ratio: default_reserve_ratio(),
            reserve_tokens: 0,
            compact_with_thinking_block: true,
            thinking_turns_to_keep: 1,
            compact_enhanced_prompt: true,
            mobile_policy: MobileCompactionPolicy::default(),
        }
    }
}

impl CompactionConfig {
    pub fn from_running_config(running: &rua_config::AgentsRunningConfig) -> Option<Self> {
        let cc = &running.context_compact;
        if !cc.context_compact_enabled {
            return None;
        }
        let max_input = running.max_input_length.max(1) as usize;
        let ratio = cc.blocking_compact_ratio.clamp(0.05, 0.99);
        let reserve_ratio = cc.context_reserve_ratio.clamp(0.01, 0.5);
        let mut cfg = Self {
            soft_token_threshold: ((max_input as f64) * ratio) as usize,

            summary_max_tokens: ((max_input as f64) * ratio * 0.95) as usize,

            reserve_ratio,
            reserve_tokens: ((max_input as f64) * reserve_ratio) as usize,
            keep_last_turns: if cc.min_keep_messages > 0 {
                cc.min_keep_messages as usize
            } else {
                default_keep_last_turns()
            },
            compact_with_thinking_block: cc.compact_with_thinking_block,
            thinking_turns_to_keep: cc.thinking_turns_to_keep,
            compact_enhanced_prompt: cc.compact_enhanced_prompt,
            ..Default::default()
        };
        cfg.keep_last_turns = cfg.keep_last_turns.max(1);
        Some(cfg)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundaryPlan {
    pub system_range: Range<usize>,

    pub compact_range: Range<usize>,

    pub keep_range: Range<usize>,

    pub protected_ranges: Vec<Range<usize>>,

    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionAudit {
    pub plan: BoundaryPlan,

    pub tokens_before: usize,

    pub tokens_after: usize,

    pub summary_attempted: bool,

    pub summary_succeeded: bool,

    pub hard_truncation_used: bool,

    pub tool_results_archived: usize,

    pub duration_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompactionGrade {
    G5PtlRetry,

    G6LlmSummary,

    HardTruncation,
}

#[derive(Debug, Clone)]
pub struct CompactionResult {
    pub messages: Vec<ModelDialogueItem>,

    pub compacted_turns: usize,

    pub removed_tokens_estimate: usize,

    pub summary_message_id: Option<String>,

    pub used_llm_summary: bool,

    pub used_hard_truncation: bool,

    pub tool_results_archived: usize,

    pub grade: Option<CompactionGrade>,

    pub audit: CompactionAudit,
}

pub struct BoundaryCalculator;

impl BoundaryCalculator {
    pub fn calculate(
        messages: &[ModelDialogueItem],
        keep_last_turns: usize,
        reserve_tokens: usize,
    ) -> BoundaryPlan {
        if messages.is_empty() {
            return BoundaryPlan {
                system_range: 0..0,
                compact_range: 0..0,
                keep_range: 0..0,
                protected_ranges: Vec::new(),
                reason: "empty messages".into(),
            };
        }

        let system_end = messages
            .iter()
            .take_while(|m| m.role == Role::System)
            .count();
        let system_range = 0..system_end;

        let protected_ranges = Self::find_protected_ranges(messages);

        let turn_keep =
            Self::find_keep_start(messages, system_end, keep_last_turns, &protected_ranges);
        let token_keep = if reserve_tokens > 0 {
            Self::find_keep_start_by_tokens(messages, system_end, reserve_tokens, &protected_ranges)
        } else {
            turn_keep
        };
        let keep_start = turn_keep.min(token_keep);
        let keep_range = keep_start..messages.len();

        let compact_range = system_end..keep_start;

        let reason = if compact_range.is_empty() {
            "no messages to compact".into()
        } else {
            format!(
                "compact {} messages (idx {}..{}), keep {} messages (idx {}..{})",
                compact_range.len(),
                compact_range.start,
                compact_range.end,
                keep_range.len(),
                keep_range.start,
                keep_range.end,
            )
        };

        BoundaryPlan {
            system_range,
            compact_range,
            keep_range,
            protected_ranges,
            reason,
        }
    }

    fn find_protected_ranges(messages: &[ModelDialogueItem]) -> Vec<Range<usize>> {
        let mut protected = Vec::new();
        let mut i = 0;

        while i < messages.len() {
            let msg = &messages[i];

            if msg.role == Role::Assistant && msg.has_tool_calls() {
                let tool_call_ids: Vec<String> =
                    msg.tool_calls().iter().map(|tc| tc.id.clone()).collect();

                let mut range_end = i + 1;
                while range_end < messages.len() {
                    let next = &messages[range_end];

                    let is_matching_result = next.content.iter().any(|part| {
                        if let ContentPart::ToolResult { result } = part {
                            tool_call_ids.contains(&result.call_id)
                        } else {
                            false
                        }
                    });

                    if is_matching_result || next.role == Role::Tool {
                        range_end += 1;
                    } else {
                        break;
                    }
                }

                if range_end > i + 1 {
                    protected.push(i..range_end);
                }
            }

            i += 1;
        }

        protected
    }

    fn find_keep_start(
        messages: &[ModelDialogueItem],
        system_end: usize,
        keep_turns: usize,
        protected: &[Range<usize>],
    ) -> usize {
        if messages.len() <= system_end {
            return system_end;
        }

        let mut turns_found = 0;
        let mut keep_start = messages.len();

        let mut i = messages.len();
        while i > system_end && turns_found < keep_turns {
            i -= 1;
            if messages[i].role == Role::User {
                turns_found += 1;
            }
            keep_start = i;
        }

        for range in protected {
            if range.start < keep_start && range.end > keep_start {
                keep_start = range.start;
            }
        }

        keep_start.max(system_end)
    }

    fn find_keep_start_by_tokens(
        messages: &[ModelDialogueItem],
        system_end: usize,
        reserve_tokens: usize,
        protected: &[Range<usize>],
    ) -> usize {
        if messages.len() <= system_end || reserve_tokens == 0 {
            return messages.len();
        }

        let estimator = HeuristicEstimator::default();
        let mut accumulated = 0usize;
        let mut keep_start = messages.len();

        let mut i = messages.len();
        while i > system_end && accumulated <= reserve_tokens {
            i -= 1;
            let msg_tokens = estimator.estimate_message(&messages[i]);
            accumulated += msg_tokens;
            keep_start = i;
        }

        for range in protected {
            if range.start < keep_start && range.end > keep_start {
                keep_start = range.start;
            }
        }

        keep_start.max(system_end)
    }
}

#[async_trait]
pub trait SummaryGenerator: Send + Sync {
    async fn summarize(
        &self,
        messages: &[ModelDialogueItem],
        previous_summary: Option<&str>,
        max_tokens: Option<usize>,
        model: Option<&rua_session::CheckpointModelIdentity>,
    ) -> Result<String, AgentError>;

    async fn summarize_with_context(
        &self,
        messages: &[ModelDialogueItem],
        previous_summary: Option<&str>,
        max_tokens: Option<usize>,
        model: Option<&rua_session::CheckpointModelIdentity>,
        _model_call_context: Option<&crate::model_call_context::ModelCallContext>,
    ) -> Result<String, AgentError> {
        self.summarize(messages, previous_summary, max_tokens, model)
            .await
    }

    async fn generate_history_replacement(
        &self,
        _request: &crate::context_manager::HistoryReplacementGenerationRequest,
    ) -> Result<crate::context_manager::HistoryReplacementGenerationResult, AgentError> {
        Err(AgentError::new(
            AgentErrorKind::Provider,
            "history replacement generation is unavailable",
        ))
    }
}

pub const SUMMARY_PROMPT: &str = r#"Summarize the following conversation concisely. Preserve:
- User's intent and goals
- Key decisions made
- File paths, code snippets, and tool result references
- Unfinished tasks or TODOs
- Agent persona and constraints

Do NOT include:
- API keys, passwords, or secrets
- Full tool output (reference only)
- Redundant greetings or filler

Keep the summary in the same language as the user's messages.
Output only the summary, no preamble."#;

pub struct MockSummaryGenerator {
    response: String,
    should_fail: bool,
}

impl MockSummaryGenerator {
    pub fn new(response: impl Into<String>) -> Self {
        Self {
            response: response.into(),
            should_fail: false,
        }
    }

    pub fn failing() -> Self {
        Self {
            response: String::new(),
            should_fail: true,
        }
    }
}

#[async_trait]
impl SummaryGenerator for MockSummaryGenerator {
    async fn summarize(
        &self,
        _messages: &[ModelDialogueItem],
        _previous_summary: Option<&str>,
        _max_tokens: Option<usize>,
        _model: Option<&rua_session::CheckpointModelIdentity>,
    ) -> Result<String, AgentError> {
        if self.should_fail {
            Err(AgentError::new(
                AgentErrorKind::Provider,
                "Summary generation failed (mock)",
            ))
        } else {
            Ok(self.response.clone())
        }
    }
}

pub struct HardTruncator;

impl HardTruncator {
    pub fn truncate(
        messages: &[ModelDialogueItem],
        plan: &BoundaryPlan,
    ) -> Result<(Vec<ModelDialogueItem>, usize), AgentError> {
        if plan.compact_range.is_empty() {
            return Ok((messages.to_vec(), 0));
        }

        let removed_count = plan.compact_range.len();

        let mut result = Vec::new();

        result.extend_from_slice(&messages[plan.system_range.clone()]);

        let notice = format!(
            "[Context truncated: {} messages removed to fit context window. \
             Some conversation history has been lost.]",
            removed_count,
        );
        result.push(ModelDialogueItem::text(Role::System, notice));

        if plan.keep_range.end > plan.keep_range.start {
            result.extend_from_slice(&messages[plan.keep_range.clone()]);
        }

        Ok((result, removed_count))
    }
}

const MAX_PTL_RETRIES: usize = 3;

#[derive(Clone)]
pub struct ConversationCompactor {
    config: CompactionConfig,
    summary_generator: Option<Arc<dyn SummaryGenerator>>,
    model_call_context: Option<crate::model_call_context::ModelCallContext>,
}

impl ConversationCompactor {
    pub fn new(config: CompactionConfig) -> Self {
        Self {
            config,
            summary_generator: None,
            model_call_context: None,
        }
    }

    pub fn with_summary_generator(mut self, gen: Arc<dyn SummaryGenerator>) -> Self {
        self.summary_generator = Some(gen);
        self
    }

    pub fn with_model_call_context(
        mut self,
        context: Option<crate::model_call_context::ModelCallContext>,
    ) -> Self {
        self.model_call_context = context;
        self
    }

    pub fn config(&self) -> &CompactionConfig {
        &self.config
    }

    fn build_summary_messages(
        messages: &[ModelDialogueItem],
        plan: &BoundaryPlan,
        summary_text: &str,
    ) -> (Vec<ModelDialogueItem>, String) {
        let summary_id = uuid::Uuid::new_v4().to_string();
        let mut result_messages = Vec::new();

        for msg in &messages[plan.system_range.clone()] {
            if !Self::is_summary_message(msg) {
                result_messages.push(msg.clone());
            }
        }

        let mut summary_msg = ModelDialogueItem::text(
            Role::System,
            format!("[Conversation Summary]\n{summary_text}"),
        );
        summary_msg.id = Some(summary_id.clone());
        summary_msg
            .metadata
            .insert("compaction_summary".into(), serde_json::json!(true));
        result_messages.push(summary_msg);

        result_messages.extend_from_slice(&messages[plan.keep_range.clone()]);

        (result_messages, summary_id)
    }

    fn is_summary_message(msg: &ModelDialogueItem) -> bool {
        msg.metadata
            .get("compaction_summary")
            .is_some_and(|v| v.as_bool() == Some(true))
            || msg.text_content().starts_with("[Conversation Summary]\n")
            || msg
                .text_content()
                .starts_with("# Compact Summary of Previous Conversation\n")
    }

    fn retain_recent_thinking(
        messages: &[ModelDialogueItem],
        thinking_turns_to_keep: i32,
    ) -> Vec<ModelDialogueItem> {
        if thinking_turns_to_keep < 0 {
            return messages.to_vec();
        }

        let mut keep_thinking = vec![false; messages.len()];
        let mut remaining = thinking_turns_to_keep as usize;
        for (index, message) in messages.iter().enumerate().rev() {
            if message.role == Role::Assistant && remaining > 0 {
                keep_thinking[index] = true;
                remaining -= 1;
            }
        }

        messages
            .iter()
            .enumerate()
            .map(|(index, message)| {
                if keep_thinking[index] {
                    return message.clone();
                }
                let filtered: Vec<_> = message
                    .content
                    .iter()
                    .filter(|part| !matches!(part, ContentPart::Thinking { .. }))
                    .cloned()
                    .collect();
                if filtered.len() == message.content.len() {
                    message.clone()
                } else {
                    ModelDialogueItem {
                        content: filtered,
                        ..message.clone()
                    }
                }
            })
            .collect()
    }

    async fn compact_g5_ptl_retry(
        &self,
        messages: &[ModelDialogueItem],
        plan: &BoundaryPlan,
        generator: &dyn SummaryGenerator,
        previous_summary: Option<&str>,
        start_time: std::time::Instant,
        estimate_before: usize,
    ) -> Result<Option<CompactionResult>, AgentError> {
        let estimator = HeuristicEstimator::default();
        let mut compact_messages: Vec<ModelDialogueItem> =
            messages[plan.compact_range.clone()].to_vec();

        let thinking_turns_to_keep = if self.config.compact_with_thinking_block {
            self.config.thinking_turns_to_keep
        } else {
            0
        };
        compact_messages = Self::retain_recent_thinking(&compact_messages, thinking_turns_to_keep);

        for attempt in 0..=MAX_PTL_RETRIES {
            match tokio::time::timeout(
                DEFAULT_SUMMARY_TIMEOUT,
                generator.summarize_with_context(
                    &compact_messages,
                    previous_summary,
                    Some(self.config.summary_max_tokens),
                    None,
                    self.model_call_context.as_ref(),
                ),
            )
            .await
            {
                Ok(Ok(summary_text)) => {
                    let grade = if attempt == 0 {
                        CompactionGrade::G6LlmSummary
                    } else {
                        CompactionGrade::G5PtlRetry
                    };

                    let (result_messages, summary_id) =
                        Self::build_summary_messages(messages, plan, &summary_text);

                    let estimate_after = estimator
                        .estimate_messages("", "", &result_messages, &[])
                        .await
                        .map(|e| e.total)
                        .unwrap_or(0);

                    tracing::info!(
                        grade = ?grade,
                        attempt,
                        tokens_saved = estimate_before.saturating_sub(estimate_after),
                        "LLM summary compaction succeeded"
                    );

                    let audit = CompactionAudit {
                        plan: plan.clone(),
                        tokens_before: estimate_before,
                        tokens_after: estimate_after,
                        summary_attempted: true,
                        summary_succeeded: true,
                        hard_truncation_used: false,
                        tool_results_archived: 0,
                        duration_ms: start_time.elapsed().as_millis() as u64,
                    };

                    return Ok(Some(CompactionResult {
                        messages: result_messages,
                        compacted_turns: plan.compact_range.len(),
                        removed_tokens_estimate: estimate_before.saturating_sub(estimate_after),
                        summary_message_id: Some(summary_id),
                        used_llm_summary: true,
                        used_hard_truncation: false,
                        tool_results_archived: 0,
                        grade: Some(grade),
                        audit,
                    }));
                }
                Ok(Err(e)) => {
                    let is_ptl = Self::is_ptl_error(&e);
                    if is_ptl && attempt < MAX_PTL_RETRIES && !compact_messages.is_empty() {
                        let drop_count = std::cmp::max(1, compact_messages.len() / 5);
                        compact_messages = compact_messages[drop_count..].to_vec();
                        tracing::warn!(
                            attempt = attempt + 1,
                            max_retries = MAX_PTL_RETRIES,
                            dropped = drop_count,
                            remaining = compact_messages.len(),
                            "G5: PTL error, dropping oldest messages and retrying"
                        );
                        continue;
                    }
                    tracing::warn!(error = %e, attempt, "LLM summary failed (non-PTL or retries exhausted)");
                    return Ok(None);
                }
                Err(_) => {
                    tracing::warn!(attempt, "LLM summary timed out");
                    return Ok(None);
                }
            }
        }

        Ok(None)
    }

    fn is_ptl_error(err: &AgentError) -> bool {
        ContextOverflowDetector::is_context_overflow_message(&err.to_string())
    }

    pub async fn compact_graded(
        &self,
        messages: &[ModelDialogueItem],
        existing_summary: Option<&str>,
    ) -> Result<CompactionResult, AgentError> {
        if existing_summary.is_some() {
            self.compact_updating_summary(messages, existing_summary)
                .await
        } else {
            self.compact(messages).await
        }
    }

    pub async fn compact(
        &self,
        messages: &[ModelDialogueItem],
    ) -> Result<CompactionResult, AgentError> {
        let start_time = std::time::Instant::now();

        let estimator = HeuristicEstimator::default();
        let estimate_before = estimator
            .estimate_messages("", "", messages, &[])
            .await
            .map(|e| e.total)
            .unwrap_or(0);

        let plan = BoundaryCalculator::calculate(
            messages,
            self.config.keep_last_turns,
            self.config.reserve_tokens,
        );

        if plan.compact_range.is_empty() {
            let audit = CompactionAudit {
                plan: plan.clone(),
                tokens_before: estimate_before,
                tokens_after: estimate_before,
                summary_attempted: false,
                summary_succeeded: false,
                hard_truncation_used: false,
                tool_results_archived: 0,
                duration_ms: start_time.elapsed().as_millis() as u64,
            };
            return Ok(CompactionResult {
                messages: messages.to_vec(),
                compacted_turns: 0,
                removed_tokens_estimate: 0,
                summary_message_id: None,
                used_llm_summary: false,
                used_hard_truncation: false,
                tool_results_archived: 0,
                grade: None,
                audit,
            });
        }

        if self.config.enable_llm_summary {
            if let Some(ref generator) = self.summary_generator {
                if let Some(result) = self
                    .compact_g5_ptl_retry(
                        messages,
                        &plan,
                        generator.as_ref(),
                        None,
                        start_time,
                        estimate_before,
                    )
                    .await?
                {
                    return Ok(result);
                }
            }
        }

        let (truncated, removed) = HardTruncator::truncate(messages, &plan)?;

        let estimate_after = estimator
            .estimate_messages("", "", &truncated, &[])
            .await
            .map(|e| e.total)
            .unwrap_or(0);

        let audit = CompactionAudit {
            plan,
            tokens_before: estimate_before,
            tokens_after: estimate_after,
            summary_attempted: true,
            summary_succeeded: false,
            hard_truncation_used: true,
            tool_results_archived: 0,
            duration_ms: start_time.elapsed().as_millis() as u64,
        };

        Ok(CompactionResult {
            messages: truncated,
            compacted_turns: removed,
            removed_tokens_estimate: estimate_before.saturating_sub(estimate_after),
            summary_message_id: None,
            used_llm_summary: false,
            used_hard_truncation: true,
            tool_results_archived: 0,
            grade: Some(CompactionGrade::HardTruncation),
            audit,
        })
    }

    pub async fn compact_updating_summary(
        &self,
        messages: &[ModelDialogueItem],
        existing_summary: Option<&str>,
    ) -> Result<CompactionResult, AgentError> {
        let start_time = std::time::Instant::now();
        let estimator = HeuristicEstimator::default();
        let estimate_before = estimator
            .estimate_messages("", "", messages, &[])
            .await
            .map(|e| e.total)
            .unwrap_or(0);
        let plan = BoundaryCalculator::calculate(
            messages,
            self.config.keep_last_turns,
            self.config.reserve_tokens,
        );

        if plan.compact_range.is_empty() {
            let audit = CompactionAudit {
                plan: plan.clone(),
                tokens_before: estimate_before,
                tokens_after: estimate_before,
                summary_attempted: false,
                summary_succeeded: false,
                hard_truncation_used: false,
                tool_results_archived: 0,
                duration_ms: start_time.elapsed().as_millis() as u64,
            };
            return Ok(CompactionResult {
                messages: messages.to_vec(),
                compacted_turns: 0,
                removed_tokens_estimate: 0,
                summary_message_id: None,
                used_llm_summary: false,
                used_hard_truncation: false,
                tool_results_archived: 0,
                grade: None,
                audit,
            });
        }

        if self.config.enable_llm_summary {
            if let (Some(generator), Some(existing_summary)) =
                (self.summary_generator.as_ref(), existing_summary)
            {
                if let Some(result) = self
                    .compact_g5_ptl_retry(
                        messages,
                        &plan,
                        generator.as_ref(),
                        Some(existing_summary),
                        start_time,
                        estimate_before,
                    )
                    .await?
                {
                    return Ok(result);
                }
            }
        }

        self.compact(messages).await
    }
}
