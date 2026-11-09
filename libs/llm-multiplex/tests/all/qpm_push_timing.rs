use std::sync::Arc;

use llm_multiplex::rate_limiter::{LlmRateLimiter, RateLimitConfig};

#[tokio::test]
async fn qpm_push_happens_after_semaphore_acquired() {
    let config = RateLimitConfig {
        max_concurrent: 1,
        max_qpm: 100,
        pause_seconds: 0.0,
        jitter_range: 0.0,
        acquire_timeout: 5.0,
    };
    let limiter = Arc::new(LlmRateLimiter::new(&config));

    limiter.acquire().await.unwrap();

    let limiter2 = limiter.clone();
    let handle = tokio::spawn(async move {
        limiter2.acquire().await.unwrap();
        limiter2.release();
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let stats = limiter.stats().await;
    assert_eq!(
        stats.requests_last_60s, 1,
        "QPM count should reflect only requests that passed semaphore, not queued ones"
    );

    limiter.release();
    handle.await.unwrap();

    let stats = limiter.stats().await;
    assert_eq!(stats.requests_last_60s, 2);
}

#[tokio::test]
async fn low_concurrency_qpm_count_matches_acquires() {
    let config = RateLimitConfig {
        max_concurrent: 10,
        max_qpm: 100,
        pause_seconds: 0.0,
        jitter_range: 0.0,
        acquire_timeout: 5.0,
    };
    let limiter = Arc::new(LlmRateLimiter::new(&config));

    for _ in 0..5 {
        limiter.acquire().await.unwrap();
        limiter.release();
    }

    let stats = limiter.stats().await;
    assert_eq!(stats.requests_last_60s, 5);
    assert_eq!(stats.current_in_flight, 0);
}
