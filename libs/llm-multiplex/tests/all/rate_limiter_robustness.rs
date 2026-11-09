use std::sync::Arc;
use std::time::Duration;

use llm_multiplex::rate_limiter::{LlmRateLimiter, LlmRateLimiterRegistry, RateLimitConfig};

fn cfg(pause_seconds: f64) -> RateLimitConfig {
    RateLimitConfig {
        max_concurrent: 1,
        max_qpm: 0,
        pause_seconds,
        jitter_range: 0.0,
        acquire_timeout: 5.0,
    }
}

#[tokio::test]
async fn report_rate_limit_caps_oversized_retry_after() {
    let limiter = LlmRateLimiter::new(&cfg(30.0));

    limiter.report_rate_limit(Some(51496.0)).await;

    let stats = limiter.stats().await;
    assert!(stats.is_paused);
    let remaining = stats.pause_remaining_s.unwrap();
    assert!(
        remaining <= LlmRateLimiter::MAX_PAUSE_SECONDS + 0.5,
        "pause must be capped at MAX_PAUSE_SECONDS, got {remaining}"
    );
    assert!(
        remaining > LlmRateLimiter::MAX_PAUSE_SECONDS - 1.0,
        "pause should sit at ~MAX_PAUSE_SECONDS, got {remaining}"
    );
}

#[tokio::test]
async fn report_rate_limit_keeps_small_retry_after() {
    let limiter = LlmRateLimiter::new(&cfg(30.0));
    limiter.report_rate_limit(Some(3.0)).await;

    let stats = limiter.stats().await;
    let remaining = stats.pause_remaining_s.unwrap();
    assert!(
        remaining > 2.0 && remaining <= 3.0,
        "small retry_after must pass through uncapped, got {remaining}"
    );
}

#[tokio::test]
async fn constructor_clamps_unsafe_rate_limit_config_values() {
    let limiter = LlmRateLimiter::new(&RateLimitConfig {
        max_concurrent: 0,
        max_qpm: 0,
        pause_seconds: -5.0,
        jitter_range: -1.0,
        acquire_timeout: 0.0,
    });

    limiter
        .acquire()
        .await
        .expect("max_concurrent=0 should clamp to one available slot");
    assert_eq!(limiter.in_flight(), 1);
    limiter.release();
    assert_eq!(limiter.in_flight(), 0);

    limiter.report_rate_limit(None).await;
    let stats = limiter.stats().await;
    assert_eq!(stats.total_rate_limited, 1);
    assert!(
        !stats.is_paused,
        "negative default pause should clamp to zero instead of installing a cooldown"
    );
}

#[tokio::test]
async fn registry_reuses_same_limiter_for_same_key() {
    let reg = LlmRateLimiterRegistry::new();
    let a = reg.get_or_create("openai:gpt-4", &cfg(30.0));
    let b = reg.get_or_create("openai:gpt-4", &cfg(30.0));
    assert!(
        Arc::ptr_eq(&a, &b),
        "same provider:model key must reuse the same limiter across calls"
    );
}

#[tokio::test]
async fn registry_isolates_different_keys() {
    let reg = LlmRateLimiterRegistry::new();
    let a = reg.get_or_create("openai:gpt-4", &cfg(30.0));
    let b = reg.get_or_create("anthropic:claude", &cfg(30.0));
    assert!(
        !Arc::ptr_eq(&a, &b),
        "different keys must get independent limiters"
    );

    a.report_rate_limit(Some(30.0)).await;
    assert!(a.stats().await.is_paused);
    assert!(
        !b.stats().await.is_paused,
        "429 on key A must not pause key B"
    );
}

#[tokio::test]
async fn registry_preserves_cooldown_across_lookups() {
    let reg = LlmRateLimiterRegistry::new();
    reg.get_or_create("dashscope:qwen", &cfg(30.0))
        .report_rate_limit(Some(30.0))
        .await;

    let again = reg.get_or_create("dashscope:qwen", &cfg(30.0));
    assert!(
        again.stats().await.is_paused,
        "cooldown must survive a subsequent registry lookup for the same key"
    );
}

#[tokio::test]
async fn on_success_preserves_fresh_pause() {
    let limiter = LlmRateLimiter::new(&cfg(30.0));
    let acquired_at = tokio::time::Instant::now();
    tokio::time::sleep(Duration::from_millis(10)).await;

    limiter.report_rate_limit(Some(30.0)).await;
    limiter.on_success(acquired_at).await;

    assert!(
        limiter.stats().await.is_paused,
        "a pause set after acquire must survive on_success"
    );
}

#[tokio::test]
async fn on_success_clears_stale_pause() {
    let limiter = LlmRateLimiter::new(&cfg(30.0));

    limiter.report_rate_limit(Some(0.05)).await;
    tokio::time::sleep(Duration::from_millis(80)).await;

    let acquired_at = tokio::time::Instant::now();
    limiter.on_success(acquired_at).await;

    assert!(
        !limiter.stats().await.is_paused,
        "a stale pause predating acquire must be cleared"
    );
}
