use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::AgentError;
use crate::memory_manager::{InMemoryMemoryManager, LlmMemoryManager, NoOpMemoryManager};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactContextRequest {
    pub running_config: rua_config::AgentsRunningConfig,
    pub session_id: String,
    pub agent_id: String,
    pub messages: Vec<serde_json::Value>,
    pub token_budget: usize,

    #[serde(default)]
    pub existing_summary: Option<String>,
    #[serde(skip)]
    pub model_call_context: Option<crate::model_call_context::ModelCallContext>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactContextResult {
    pub compacted_messages: Vec<serde_json::Value>,
    pub tokens_saved: usize,
    pub summary_generated: Option<String>,

    #[serde(default)]
    pub grade: Option<crate::compaction::CompactionGrade>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSummaryRequest {
    pub session_id: String,
    pub agent_id: String,
    pub messages: Vec<agent_types::ModelDialogueItem>,
    #[serde(default)]
    pub previous_summary: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<usize>,
    #[serde(default)]
    pub model: Option<rua_session::CheckpointModelIdentity>,
    #[serde(skip)]
    pub model_call_context: Option<crate::model_call_context::ModelCallContext>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSummaryResult {
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryReplacementGenerationRequest {
    pub session_id: String,
    pub agent_id: String,
    pub turn_id: String,
    pub source_block_id: String,
    pub usage_id: String,
    pub source_content: String,
    pub max_tokens: usize,
    pub model: rua_session::CheckpointModelIdentity,
    #[serde(skip)]
    pub model_call_context: Option<crate::model_call_context::ModelCallContext>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryReplacementGenerationResult {
    pub digest: String,
    pub finish_reason: String,
    #[serde(default)]
    pub usage: Option<agent_types::TokenUsage>,
}

#[async_trait]
pub trait ContextManager: Send + Sync {
    async fn compact_context(
        &self,
        req: CompactContextRequest,
    ) -> Result<CompactContextResult, AgentError>;

    async fn summarize_context(
        &self,
        req: ContextSummaryRequest,
    ) -> Result<ContextSummaryResult, AgentError>;

    async fn generate_history_replacement(
        &self,
        _req: HistoryReplacementGenerationRequest,
    ) -> Result<HistoryReplacementGenerationResult, AgentError> {
        Err(AgentError::new(
            crate::error::AgentErrorKind::Provider,
            "history replacement generation is unavailable",
        ))
    }
}

#[async_trait]
impl ContextManager for LlmMemoryManager {
    async fn compact_context(
        &self,
        req: CompactContextRequest,
    ) -> Result<CompactContextResult, AgentError> {
        use crate::compaction::ConversationCompactor;
        use agent_types::ModelDialogueItem;

        let messages: Vec<ModelDialogueItem> = req
            .messages
            .iter()
            .filter_map(|v| serde_json::from_value(v.clone()).ok())
            .collect();

        if messages.len() <= 2 {
            return Ok(CompactContextResult {
                compacted_messages: req.messages,
                tokens_saved: 0,
                summary_generated: None,
                grade: None,
            });
        }

        let Some(config) =
            crate::compaction::CompactionConfig::from_running_config(&req.running_config)
        else {
            return Ok(CompactContextResult {
                compacted_messages: req.messages,
                tokens_saved: 0,
                summary_generated: None,
                grade: None,
            });
        };

        let compactor = ConversationCompactor::new(config)
            .with_summary_generator(self.summary_generator())
            .with_model_call_context(req.model_call_context.clone());

        let result = if req.existing_summary.as_deref().is_some() {
            compactor
                .compact_updating_summary(&messages, req.existing_summary.as_deref())
                .await
        } else {
            compactor.compact_graded(&messages, None).await
        };

        match result {
            Ok(cr) => {
                let compacted: Vec<serde_json::Value> = cr
                    .messages
                    .iter()
                    .filter_map(|m| serde_json::to_value(m).ok())
                    .collect();

                let summary_text = cr.messages.iter().find_map(|m| {
                    if m.metadata
                        .get("compaction_summary")
                        .is_some_and(|v| v.as_bool() == Some(true))
                    {
                        let text = m.text_content();
                        let stripped = text
                            .strip_prefix("[Conversation Summary]\n")
                            .unwrap_or(&text);
                        Some(stripped.to_string())
                    } else {
                        None
                    }
                });

                Ok(CompactContextResult {
                    compacted_messages: compacted,
                    tokens_saved: cr.removed_tokens_estimate,
                    summary_generated: summary_text,
                    grade: cr.grade,
                })
            }
            Err(e) => {
                tracing::warn!("Graded compaction failed: {e}");
                Ok(CompactContextResult {
                    compacted_messages: req.messages,
                    tokens_saved: 0,
                    summary_generated: None,
                    grade: None,
                })
            }
        }
    }

    async fn summarize_context(
        &self,
        req: ContextSummaryRequest,
    ) -> Result<ContextSummaryResult, AgentError> {
        match self
            .summary_generator()
            .summarize_with_context(
                &req.messages,
                req.previous_summary.as_deref(),
                req.max_tokens,
                req.model.as_ref(),
                req.model_call_context.as_ref(),
            )
            .await
        {
            Ok(summary) => Ok(ContextSummaryResult { summary }),
            Err(e) => Err(e),
        }
    }

    async fn generate_history_replacement(
        &self,
        req: HistoryReplacementGenerationRequest,
    ) -> Result<HistoryReplacementGenerationResult, AgentError> {
        self.summary_generator()
            .generate_history_replacement(&req)
            .await
    }
}

#[async_trait]
impl ContextManager for NoOpMemoryManager {
    async fn compact_context(
        &self,
        req: CompactContextRequest,
    ) -> Result<CompactContextResult, AgentError> {
        Ok(CompactContextResult {
            compacted_messages: req.messages,
            tokens_saved: 0,
            summary_generated: None,
            grade: None,
        })
    }

    async fn summarize_context(
        &self,
        _req: ContextSummaryRequest,
    ) -> Result<ContextSummaryResult, AgentError> {
        Ok(ContextSummaryResult {
            summary: String::new(),
        })
    }
}

#[async_trait]
impl ContextManager for InMemoryMemoryManager {
    async fn compact_context(
        &self,
        req: CompactContextRequest,
    ) -> Result<CompactContextResult, AgentError> {
        Ok(CompactContextResult {
            compacted_messages: req.messages,
            tokens_saved: 0,
            summary_generated: None,
            grade: None,
        })
    }

    async fn summarize_context(
        &self,
        _req: ContextSummaryRequest,
    ) -> Result<ContextSummaryResult, AgentError> {
        Ok(ContextSummaryResult {
            summary: String::new(),
        })
    }
}
