use std::sync::Arc;

use llm_multiplex::rate_limiter::{LlmRateLimiterRegistry, RateLimitChatModel, RateLimitConfig};
use llm_multiplex::traits::ChatModel;
use rua_contract::UsageRecorder;

use crate::fallback::{FallbackChatModel, FallbackMode, FallbackPolicy, FallbackResolver};
use crate::retry::{RetryChatModel, RetryPolicy};
use crate::usage_recording::UsageRecordingChatModel;

#[derive(Clone)]
pub struct ModelFactory {
    retry_policy: RetryPolicy,
    rate_limit_config: Option<RateLimitConfig>,
    rate_limiter_registry: Arc<LlmRateLimiterRegistry>,
    usage_recorder: Option<Arc<dyn UsageRecorder>>,
    fallback_policy: Option<FallbackPolicy>,
    fallback_resolver: Option<Arc<dyn FallbackResolver>>,
}

impl ModelFactory {
    pub fn new() -> Self {
        Self {
            retry_policy: RetryPolicy::default(),
            rate_limit_config: Some(RateLimitConfig::default()),
            rate_limiter_registry: Arc::new(LlmRateLimiterRegistry::new()),
            usage_recorder: None,
            fallback_policy: None,
            fallback_resolver: None,
        }
    }

    pub fn with_retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.retry_policy = policy;
        self
    }

    pub fn with_rate_limit_config(mut self, config: RateLimitConfig) -> Self {
        self.rate_limit_config = Some(config);
        self
    }

    pub fn with_usage_recorder(mut self, recorder: Arc<dyn UsageRecorder>) -> Self {
        self.usage_recorder = Some(recorder);
        self
    }

    pub fn with_fallback(
        mut self,
        policy: FallbackPolicy,
        resolver: Arc<dyn FallbackResolver>,
    ) -> Self {
        self.fallback_policy = Some(policy);
        self.fallback_resolver = Some(resolver);
        self
    }

    pub fn build(&self, base_model: Box<dyn ChatModel>, provider_id: &str) -> Box<dyn ChatModel> {
        let rate_limited: Box<dyn ChatModel> = if let Some(ref config) = self.rate_limit_config {
            let key = format!(
                "{provider_id}:{}:{}:{}:{}:{}:{}",
                base_model.model_info().id,
                config.max_concurrent,
                config.max_qpm,
                config.pause_seconds.to_bits(),
                config.jitter_range.to_bits(),
                config.acquire_timeout.to_bits()
            );
            let limiter = self.rate_limiter_registry.get_or_create(&key, config);
            Box::new(RateLimitChatModel::new(base_model, limiter))
        } else {
            base_model
        };

        let retried: Box<dyn ChatModel> =
            Box::new(RetryChatModel::new(rate_limited, self.retry_policy.clone()));

        let usage_wrapped: Box<dyn ChatModel> = if let Some(ref recorder) = self.usage_recorder {
            let model_id = retried.model_info().id.clone();
            Box::new(UsageRecordingChatModel::new(
                retried,
                Arc::clone(recorder),
                provider_id.to_string(),
                model_id,
            ))
        } else {
            retried
        };

        if let (Some(ref policy), Some(ref resolver)) =
            (&self.fallback_policy, &self.fallback_resolver)
        {
            if policy.mode != FallbackMode::Disabled {
                return Box::new(FallbackChatModel::new(
                    usage_wrapped,
                    policy.clone(),
                    Arc::clone(resolver),
                ));
            }
        }

        usage_wrapped
    }
}

impl Default for ModelFactory {
    fn default() -> Self {
        Self::new()
    }
}
