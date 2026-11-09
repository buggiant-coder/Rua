use agent_types::TokenUsage;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageRecordMetadata {
    #[serde(default)]
    pub usage_id: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub chat_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub turn_id: Option<String>,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub device_id: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[async_trait]
pub trait UsageRecorder: Send + Sync {
    async fn record_usage(
        &self,
        provider_id: &str,
        model_id: &str,
        usage: &TokenUsage,
        started_at: DateTime<Utc>,
        finished_at: DateTime<Utc>,
        metadata: UsageRecordMetadata,
    );
}
