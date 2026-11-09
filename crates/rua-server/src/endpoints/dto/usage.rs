use rua_usage::{
    TokenUsageByModel as StoreByModel, TokenUsageFilter, TokenUsageRecord as StoreRecord,
    TokenUsageStats as StoreStats, TokenUsageSummary,
};

use crate::endpoints::dto::{
    TokenUsageByModel, TokenUsageQuery, TokenUsageRecord, TokenUsageResponse, TokenUsageStats,
};

impl From<&TokenUsageQuery> for TokenUsageFilter {
    fn from(query: &TokenUsageQuery) -> Self {
        TokenUsageFilter {
            start_date: query.start_date.clone(),
            end_date: query.end_date.clone(),
            days: query.days,
            provider_id: query.provider_id.clone(),
            model_name: query.model_name.clone(),
            agent_id: query.agent_id.clone(),
            chat_id: query.chat_id.clone(),
        }
    }
}

impl From<StoreStats> for TokenUsageStats {
    fn from(stats: StoreStats) -> Self {
        TokenUsageStats {
            prompt_tokens: stats.prompt_tokens,
            completion_tokens: stats.completion_tokens,
            call_count: stats.call_count,
        }
    }
}

impl From<StoreByModel> for TokenUsageByModel {
    fn from(by: StoreByModel) -> Self {
        TokenUsageByModel {
            provider_id: by.provider_id,
            model: by.model,
            prompt_tokens: by.prompt_tokens,
            completion_tokens: by.completion_tokens,
            call_count: by.call_count,
        }
    }
}

impl From<StoreRecord> for TokenUsageRecord {
    fn from(record: StoreRecord) -> Self {
        TokenUsageRecord {
            date: record.date,
            provider_id: record.provider_id,
            model: record.model,
            prompt_tokens: record.prompt_tokens,
            completion_tokens: record.completion_tokens,
            call_count: record.call_count,
        }
    }
}

impl From<TokenUsageSummary> for TokenUsageResponse {
    fn from(summary: TokenUsageSummary) -> Self {
        TokenUsageResponse {
            total_prompt_tokens: summary.total_prompt_tokens,
            total_completion_tokens: summary.total_completion_tokens,
            total_calls: summary.total_calls,
            by_model: summary
                .by_model
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
            by_provider: summary
                .by_provider
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
            by_date: summary
                .by_date
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
        }
    }
}
