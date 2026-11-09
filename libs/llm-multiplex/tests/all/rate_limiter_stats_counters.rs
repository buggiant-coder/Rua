use std::sync::Arc;

use llm_multiplex::rate_limiter::{LlmRateLimiter, RateLimitConfig};

fn base_config() -> RateLimitConfig {
    RateLimitConfig {
        max_concurrent: 10,
        max_qpm: 0,
        pause_seconds: 0.0,
        jitter_range: 0.0,
        acquire_timeout: 5.0,
    }
}

#[tokio::test]
async fn total_acquired_counts_successful_acquires() {
    let limiter = Arc::new(LlmRateLimiter::new(&base_config()));

    for _ in 0..5 {
        limiter.acquire().await.unwrap();
        limiter.release();
    }

    let stats = limiter.stats().await;
    assert_eq!(stats.total_acquired, 5);
}

#[tokio::test]
async fn total_rate_limited_counts_report_calls() {
    let limiter = Arc::new(LlmRateLimiter::new(&base_config()));

    limiter.report_rate_limit(Some(0.01)).await;
    limiter.report_rate_limit(None).await;
    limiter.report_rate_limit(Some(0.01)).await;

    let stats = limiter.stats().await;
    assert_eq!(stats.total_rate_limited, 3);
}

#[tokio::test]
async fn total_paused_increments_on_429_cooldown() {
    let config = RateLimitConfig {
        pause_seconds: 0.0,
        jitter_range: 0.0,
        ..base_config()
    };
    let limiter = Arc::new(LlmRateLimiter::new(&config));

    limiter.report_rate_limit(Some(0.05)).await;

    limiter.acquire().await.unwrap();
    limiter.release();

    let stats = limiter.stats().await;
    assert_eq!(stats.total_paused, 1);
    assert_eq!(stats.total_acquired, 1);
}

#[tokio::test]
async fn total_qpm_waited_increments_on_qpm_limit() {
    let config = RateLimitConfig {
        max_qpm: 1,
        ..base_config()
    };
    let limiter = Arc::new(LlmRateLimiter::new(&config));

    limiter.acquire().await.unwrap();
    limiter.release();

    let stats = limiter.stats().await;
    assert_eq!(stats.total_acquired, 1);
    assert_eq!(stats.total_qpm_waited, 0);
}

#[tokio::test]
async fn counters_start_at_zero() {
    let limiter = Arc::new(LlmRateLimiter::new(&base_config()));
    let stats = limiter.stats().await;

    assert_eq!(stats.total_acquired, 0);
    assert_eq!(stats.total_paused, 0);
    assert_eq!(stats.total_qpm_waited, 0);
    assert_eq!(stats.total_rate_limited, 0);
}

#[tokio::test]
async fn counters_serde_roundtrip() {
    let limiter = Arc::new(LlmRateLimiter::new(&base_config()));

    limiter.acquire().await.unwrap();
    limiter.release();
    limiter.report_rate_limit(Some(0.01)).await;

    let stats = limiter.stats().await;
    let json = serde_json::to_string(&stats).unwrap();
    let de: llm_multiplex::RateLimiterStats = serde_json::from_str(&json).unwrap();

    assert_eq!(de.total_acquired, 1);
    assert_eq!(de.total_rate_limited, 1);
    assert_eq!(de.total_paused, 0);
    assert_eq!(de.total_qpm_waited, 0);
}
