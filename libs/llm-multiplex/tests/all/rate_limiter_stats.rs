use std::sync::Arc;

use llm_multiplex::rate_limiter::{LlmRateLimiter, RateLimitConfig};

fn test_config() -> RateLimitConfig {
    RateLimitConfig {
        max_concurrent: 10,
        max_qpm: 100,
        pause_seconds: 5.0,
        jitter_range: 0.0,
        acquire_timeout: 5.0,
    }
}

#[tokio::test]
async fn stats_requests_last_60s_after_acquires() {
    let limiter = Arc::new(LlmRateLimiter::new(&test_config()));

    for _ in 0..5 {
        limiter.acquire().await.unwrap();
        limiter.release();
    }

    let stats = limiter.stats().await;
    assert_eq!(stats.requests_last_60s, 5);
    assert_eq!(stats.current_in_flight, 0);
    assert!(!stats.is_paused);
    assert!(stats.pause_remaining_s.is_none());
}

#[tokio::test]
async fn stats_current_in_flight() {
    let limiter = Arc::new(LlmRateLimiter::new(&test_config()));

    limiter.acquire().await.unwrap();
    limiter.acquire().await.unwrap();

    let stats = limiter.stats().await;
    assert_eq!(stats.current_in_flight, 2);

    limiter.release();
    let stats = limiter.stats().await;
    assert_eq!(stats.current_in_flight, 1);

    limiter.release();
    let stats = limiter.stats().await;
    assert_eq!(stats.current_in_flight, 0);
}

#[tokio::test]
async fn stats_pause_after_429() {
    let limiter = Arc::new(LlmRateLimiter::new(&test_config()));

    limiter.report_rate_limit(Some(2.0)).await;

    let stats = limiter.stats().await;
    assert!(stats.is_paused);
    assert!(stats.pause_remaining_s.is_some());
    let remaining = stats.pause_remaining_s.unwrap();
    assert!(remaining > 0.0 && remaining <= 2.0);
}

#[tokio::test]
async fn report_rate_limit_without_retry_after_uses_configured_default_pause() {
    let config = RateLimitConfig {
        pause_seconds: 0.05,
        ..test_config()
    };
    let limiter = Arc::new(LlmRateLimiter::new(&config));

    limiter.report_rate_limit(None).await;

    let stats = limiter.stats().await;
    assert!(stats.is_paused);
    let remaining = stats.pause_remaining_s.expect("default pause remaining");
    assert!(
        remaining > 0.0 && remaining <= 0.05,
        "Retry-After absence should use configured default pause, got {remaining}"
    );
    assert_eq!(stats.total_rate_limited, 1);
}

#[tokio::test]
async fn stats_no_qpm_tracking_when_disabled() {
    let config = RateLimitConfig {
        max_qpm: 0,
        ..test_config()
    };
    let limiter = Arc::new(LlmRateLimiter::new(&config));

    for _ in 0..3 {
        limiter.acquire().await.unwrap();
        limiter.release();
    }

    let stats = limiter.stats().await;
    assert_eq!(stats.requests_last_60s, 0);
}

#[tokio::test]
async fn stats_serde_roundtrip() {
    let limiter = Arc::new(LlmRateLimiter::new(&test_config()));
    limiter.acquire().await.unwrap();
    limiter.release();

    let stats = limiter.stats().await;
    let json = serde_json::to_string(&stats).unwrap();
    let deserialized: llm_multiplex::RateLimiterStats = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.current_in_flight, stats.current_in_flight);
    assert_eq!(deserialized.requests_last_60s, stats.requests_last_60s);
    assert_eq!(deserialized.is_paused, stats.is_paused);
    assert_eq!(deserialized.pause_remaining_s, stats.pause_remaining_s);
}
