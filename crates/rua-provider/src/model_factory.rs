use std::sync::Arc;

use llm_multiplex::rate_limiter::{LlmRateLimiterRegistry, RateLimitChatModel, RateLimitConfig};
use llm_multiplex::traits::ChatModel;
use rua_contract::UsageRecorder;

use crate::fallback::{FallbackChatModel, FallbackMode, FallbackPolicy, FallbackResolver};
use crate::input_guard::InputLimitChatModel;
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
    max_input_tokens: Option<u64>,
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
            max_input_tokens: None,
        }
    }

    /// Reject requests whose serialized input exceeds `max_input_tokens` before
    /// they are sent. Projection normally keeps input inside the limit, so this
    /// only fires when an estimate or a summary path has gone wrong. The limit
    /// is the caller's declared ceiling, not the model's advertised window.
    pub fn with_max_input_tokens(mut self, max_input_tokens: u64) -> Self {
        self.max_input_tokens = Some(max_input_tokens);
        self
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
        // Only guard when a caller states the limit explicitly. Deriving it from
        // the model's context window would apply a different number than the
        // projection planner uses and reject requests the caller intends to send.
        let guarded: Box<dyn ChatModel> = match self.max_input_tokens {
            Some(max_input_tokens) if max_input_tokens > 0 => {
                Box::new(InputLimitChatModel::new(base_model, max_input_tokens))
            }
            _ => base_model,
        };

        let rate_limited: Box<dyn ChatModel> = if let Some(ref config) = self.rate_limit_config {
            let key = format!(
                "{provider_id}:{}:{}:{}:{}:{}:{}",
                guarded.model_info().id,
                config.max_concurrent,
                config.max_qpm,
                config.pause_seconds.to_bits(),
                config.jitter_range.to_bits(),
                config.acquire_timeout.to_bits()
            );
            let limiter = self.rate_limiter_registry.get_or_create(&key, config);
            Box::new(RateLimitChatModel::new(guarded, limiter))
        } else {
            guarded
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
