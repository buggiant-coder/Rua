use std::sync::OnceLock;
use std::time::Duration;

use agent_types::ModelDialogueItem;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokenizers::Tokenizer;

use crate::error::AgentError;

const TOKENIZER_BYTES: &[u8] = include_bytes!("../data/tokenizer.json");

static BPE_TOKENIZER: OnceLock<Tokenizer> = OnceLock::new();

fn get_bpe_tokenizer() -> &'static Tokenizer {
    BPE_TOKENIZER.get_or_init(|| {
        Tokenizer::from_bytes(TOKENIZER_BYTES).expect("bundled tokenizer.json must be valid")
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextLimit {
    pub max_context_tokens: usize,

    pub reserved_output_tokens: usize,

    pub reserved_tool_tokens: usize,

    #[serde(default = "default_soft_threshold")]
    pub soft_threshold_ratio: f32,
}

fn default_soft_threshold() -> f32 {
    0.85
}

impl Default for ContextLimit {
    fn default() -> Self {
        Self {
            max_context_tokens: 128_000,
            reserved_output_tokens: 4096,
            reserved_tool_tokens: 2048,
            soft_threshold_ratio: 0.85,
        }
    }
}

impl ContextLimit {
    pub fn with_max(max_context_tokens: usize) -> Self {
        Self {
            max_context_tokens,
            ..Default::default()
        }
    }

    pub fn effective_budget(&self) -> usize {
        self.max_context_tokens
            .saturating_sub(self.reserved_output_tokens)
            .saturating_sub(self.reserved_tool_tokens)
    }

    pub fn soft_threshold(&self) -> usize {
        let budget = self.effective_budget();
        (budget as f64 * self.soft_threshold_ratio as f64) as usize
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextCheckReason {
    WithinBudget,

    SoftThresholdExceeded,

    HardLimitExceeded,

    EstimationFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextCheckResult {
    pub valid: bool,

    pub estimated_tokens: usize,

    pub limit: usize,

    pub overflow_by: isize,

    pub reason: ContextCheckReason,

    pub needs_compaction: bool,

    pub estimated: bool,
}

#[derive(Debug, Clone)]
pub struct TokenEstimate {
    pub total: usize,

    pub per_message: Vec<usize>,

    pub estimated: bool,

    pub method: EstimationMethod,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EstimationMethod {
    ProviderTokenizer,

    Tiktoken,

    LocalTokenizer,

    Heuristic,
}

#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: String,

    pub description: String,

    pub parameters: serde_json::Value,
}

#[async_trait]
pub trait TokenEstimator: Send + Sync {
    async fn estimate_messages(
        &self,
        provider_id: &str,
        model_id: &str,
        messages: &[ModelDialogueItem],
        tools: &[ToolSpec],
    ) -> Result<TokenEstimate, AgentError>;

    fn estimate_string(&self, text: &str) -> usize;
}

#[derive(Debug, Clone)]
pub struct HeuristicEstimator {
    chars_per_token_latin: f64,

    chars_per_token_cjk: f64,

    per_message_overhead: usize,

    per_tool_overhead: usize,
}

impl Default for HeuristicEstimator {
    fn default() -> Self {
        Self::with_divisor(4.0)
    }
}

impl HeuristicEstimator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_divisor(divisor: f64) -> Self {
        let latin = divisor.clamp(1.0, 10.0);
        Self {
            chars_per_token_latin: latin,
            chars_per_token_cjk: (latin * 0.5).max(1.0),
            per_message_overhead: 4,
            per_tool_overhead: 20,
        }
    }

    pub fn estimate_string(&self, text: &str) -> usize {
        if text.is_empty() {
            return 0;
        }

        let mut latin_chars = 0usize;
        let mut cjk_chars = 0usize;

        for ch in text.chars() {
            if is_cjk_char(ch) {
                cjk_chars += 1;
            } else {
                latin_chars += 1;
            }
        }

        let latin_tokens = (latin_chars as f64 / self.chars_per_token_latin).ceil() as usize;
        let cjk_tokens = (cjk_chars as f64 / self.chars_per_token_cjk).ceil() as usize;

        latin_tokens + cjk_tokens
    }

    pub fn estimate_message(&self, msg: &ModelDialogueItem) -> usize {
        let mut tokens = self.per_message_overhead;

        for part in &msg.content {
            tokens += self.estimate_content_part(part);
        }

        if msg.name.is_some() {
            tokens += 2;
        }

        tokens
    }

    fn estimate_content_part(&self, part: &agent_types::ContentPart) -> usize {
        match part {
            agent_types::ContentPart::Text { text } => self.estimate_string(text),
            agent_types::ContentPart::Thinking { text, .. } => self.estimate_string(text),
            agent_types::ContentPart::ToolUse { call } => {
                let name_tokens = self.estimate_string(&call.name);
                let args_tokens = self.estimate_string(&call.arguments.to_string());
                name_tokens + args_tokens + 5
            }
            agent_types::ContentPart::ToolResult { result } => {
                let content_tokens: usize = result
                    .content
                    .iter()
                    .map(|p| self.estimate_content_part(p))
                    .sum();
                content_tokens + 5
            }
            agent_types::ContentPart::Image { .. } => 256,
            agent_types::ContentPart::Audio { .. } => 128,
            agent_types::ContentPart::Video { .. } => 256,
            agent_types::ContentPart::File { .. } => 64,
            agent_types::ContentPart::Data { value } => self.estimate_string(&value.to_string()),
            _ => 10,
        }
    }

    pub fn estimate_tools(&self, tools: &[ToolSpec]) -> usize {
        tools
            .iter()
            .map(|tool| {
                self.per_tool_overhead
                    + self.estimate_string(&tool.name)
                    + self.estimate_string(&tool.description)
                    + self.estimate_string(&tool.parameters.to_string())
            })
            .sum()
    }
}

pub fn is_cjk_char(ch: char) -> bool {
    matches!(ch,
        '\u{4E00}'..='\u{9FFF}'
        | '\u{3400}'..='\u{4DBF}'
        | '\u{F900}'..='\u{FAFF}'
        | '\u{3000}'..='\u{303F}'
        | '\u{3040}'..='\u{309F}'
        | '\u{30A0}'..='\u{30FF}'
        | '\u{AC00}'..='\u{D7AF}'
    )
}

#[async_trait]
impl TokenEstimator for HeuristicEstimator {
    async fn estimate_messages(
        &self,
        _provider_id: &str,
        _model_id: &str,
        messages: &[ModelDialogueItem],
        tools: &[ToolSpec],
    ) -> Result<TokenEstimate, AgentError> {
        let per_message: Vec<usize> = messages.iter().map(|m| self.estimate_message(m)).collect();
        let messages_total: usize = per_message.iter().sum();
        let tools_total = self.estimate_tools(tools);

        Ok(TokenEstimate {
            total: messages_total + tools_total,
            per_message,
            estimated: true,
            method: EstimationMethod::Heuristic,
        })
    }

    fn estimate_string(&self, text: &str) -> usize {
        HeuristicEstimator::estimate_string(self, text)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BpeEstimator {
    per_message_overhead: usize,
    per_tool_overhead: usize,
}

impl Default for BpeEstimator {
    fn default() -> Self {
        Self {
            per_message_overhead: 4,
            per_tool_overhead: 20,
        }
    }
}

impl BpeEstimator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn estimate_string(&self, text: &str) -> usize {
        HeuristicEstimator::default().estimate_string(text)
    }

    pub fn encode_len(&self, text: &str) -> usize {
        if text.is_empty() {
            return 0;
        }
        let tokenizer = get_bpe_tokenizer();
        match tokenizer.encode(text, false) {
            Ok(encoding) => encoding.get_ids().len(),
            Err(_) => {
                let fallback = HeuristicEstimator::default();
                fallback.estimate_string(text)
            }
        }
    }

    fn estimate_content_part(&self, part: &agent_types::ContentPart) -> usize {
        match part {
            agent_types::ContentPart::Text { text } => self.encode_len(text),
            agent_types::ContentPart::Thinking { text, .. } => self.encode_len(text),
            agent_types::ContentPart::ToolUse { call } => {
                let name_tokens = self.encode_len(&call.name);
                let args_tokens = self.encode_len(&call.arguments.to_string());
                name_tokens + args_tokens + 5
            }
            agent_types::ContentPart::ToolResult { result } => {
                let content_tokens: usize = result
                    .content
                    .iter()
                    .map(|p| self.estimate_content_part(p))
                    .sum();
                content_tokens + 5
            }
            agent_types::ContentPart::Image { .. } => 256,
            agent_types::ContentPart::Audio { .. } => 128,
            agent_types::ContentPart::Video { .. } => 256,
            agent_types::ContentPart::File { .. } => 64,
            agent_types::ContentPart::Data { value } => self.encode_len(&value.to_string()),
            _ => 10,
        }
    }

    pub fn estimate_message(&self, msg: &ModelDialogueItem) -> usize {
        let mut tokens = self.per_message_overhead;
        for part in &msg.content {
            tokens += self.estimate_content_part(part);
        }
        if msg.name.is_some() {
            tokens += 2;
        }
        tokens
    }

    pub fn estimate_tools(&self, tools: &[ToolSpec]) -> usize {
        tools
            .iter()
            .map(|tool| {
                self.per_tool_overhead
                    + self.encode_len(&tool.name)
                    + self.encode_len(&tool.description)
                    + self.encode_len(&tool.parameters.to_string())
            })
            .sum()
    }
}

#[async_trait]
impl TokenEstimator for BpeEstimator {
    async fn estimate_messages(
        &self,
        _provider_id: &str,
        _model_id: &str,
        messages: &[ModelDialogueItem],
        tools: &[ToolSpec],
    ) -> Result<TokenEstimate, AgentError> {
        let per_message: Vec<usize> = messages.iter().map(|m| self.estimate_message(m)).collect();
        let messages_total: usize = per_message.iter().sum();
        let tools_total = self.estimate_tools(tools);

        Ok(TokenEstimate {
            total: messages_total + tools_total,
            per_message,
            estimated: false,
            method: EstimationMethod::Tiktoken,
        })
    }

    fn estimate_string(&self, text: &str) -> usize {
        BpeEstimator::estimate_string(self, text)
    }
}

pub struct ContextChecker {
    estimator: Box<dyn TokenEstimator>,
    limit: ContextLimit,

    estimation_timeout: Duration,
}

impl ContextChecker {
    pub fn new(estimator: Box<dyn TokenEstimator>, limit: ContextLimit) -> Self {
        Self {
            estimator,
            limit,
            estimation_timeout: Duration::from_secs(5),
        }
    }

    pub fn heuristic(limit: ContextLimit) -> Self {
        Self::new(Box::new(HeuristicEstimator::default()), limit)
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.estimation_timeout = timeout;
        self
    }

    pub fn limit(&self) -> &ContextLimit {
        &self.limit
    }

    pub fn set_limit(&mut self, limit: ContextLimit) {
        self.limit = limit;
    }

    pub async fn check(
        &self,
        provider_id: &str,
        model_id: &str,
        messages: &[ModelDialogueItem],
        tools: &[ToolSpec],
    ) -> ContextCheckResult {
        let estimate = tokio::time::timeout(
            self.estimation_timeout,
            self.estimator
                .estimate_messages(provider_id, model_id, messages, tools),
        )
        .await;

        match estimate {
            Ok(Ok(est)) => {
                let budget = self.limit.effective_budget();
                let soft = self.limit.soft_threshold();
                let overflow = est.total as isize - budget as isize;

                if est.total > budget {
                    ContextCheckResult {
                        valid: false,
                        estimated_tokens: est.total,
                        limit: budget,
                        overflow_by: overflow,
                        reason: ContextCheckReason::HardLimitExceeded,
                        needs_compaction: true,
                        estimated: est.estimated,
                    }
                } else if est.total > soft {
                    ContextCheckResult {
                        valid: true,
                        estimated_tokens: est.total,
                        limit: budget,
                        overflow_by: overflow,
                        reason: ContextCheckReason::SoftThresholdExceeded,
                        needs_compaction: true,
                        estimated: est.estimated,
                    }
                } else {
                    ContextCheckResult {
                        valid: true,
                        estimated_tokens: est.total,
                        limit: budget,
                        overflow_by: overflow,
                        reason: ContextCheckReason::WithinBudget,
                        needs_compaction: false,
                        estimated: est.estimated,
                    }
                }
            }
            Ok(Err(_)) | Err(_) => {
                let budget = self.limit.effective_budget();
                ContextCheckResult {
                    valid: false,
                    estimated_tokens: 0,
                    limit: budget,
                    overflow_by: 0,
                    reason: ContextCheckReason::EstimationFailed,
                    needs_compaction: true,
                    estimated: true,
                }
            }
        }
    }
}
