use std::sync::Arc;

use agent_types::{ChatRequest, ModelDialogueItem, Role};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationProgressStatus {
    Available,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CollaborationProgressSummary {
    pub status: CollaborationProgressStatus,
    pub activity_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub covered_at: String,
    #[serde(default)]
    pub stale: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[async_trait]
pub trait CollaborationProgressSummarizer: Send + Sync {
    async fn summarize(
        &self,
        activity: &str,
        previous_summary: Option<&str>,
        model_slot: Option<&serde_json::Value>,
    ) -> Result<String, String>;
}

pub struct ProviderCollaborationProgressSummarizer {
    providers: Arc<rua_provider::manager::ProviderManager>,
}

impl ProviderCollaborationProgressSummarizer {
    pub fn new(providers: Arc<rua_provider::manager::ProviderManager>) -> Self {
        Self { providers }
    }
}

#[async_trait]
impl CollaborationProgressSummarizer for ProviderCollaborationProgressSummarizer {
    async fn summarize(
        &self,
        activity: &str,
        previous_summary: Option<&str>,
        model_slot: Option<&serde_json::Value>,
    ) -> Result<String, String> {
        let model = match model_slot.and_then(rua_agent::runtime::model_slot_provider_model) {
            Some((provider, model)) => self
                .providers
                .chat_model(&provider, &model)
                .map_err(|error| error.to_string())?,
            None => self
                .providers
                .default_chat_model()
                .await
                .map_err(|error| error.to_string())?,
        };
        let previous = previous_summary.unwrap_or("No previous summary.");
        let prompt = format!(
            "Previous bounded progress summary:\n{previous}\n\nNew public activity:\n{activity}\n\nSummarize only observed progress. Cover confirmed findings, the current observed step, explicit blockers, and important changes. State when blockers or completion percentage are unknown. Do not infer hidden reasoning. Do not reproduce identifiers, tool protocol, raw logs, or long outputs. Return at most 6 concise bullet points and no preamble."
        );
        let request = ChatRequest::new(vec![
            ModelDialogueItem::text(
                Role::System,
                "You create concise, evidence-bound progress summaries for delegated work. You never infer hidden reasoning or unobserved state.",
            ),
            ModelDialogueItem::text(Role::User, prompt),
        ])
        .with_max_tokens(256);
        let response = model
            .chat(request)
            .await
            .map_err(|error| error.to_string())?;
        let summary = response
            .text_content()
            .trim()
            .chars()
            .take(512)
            .collect::<String>();
        if summary.is_empty() {
            Err("Collaboration progress summarizer returned empty output".into())
        } else {
            Ok(summary)
        }
    }
}
