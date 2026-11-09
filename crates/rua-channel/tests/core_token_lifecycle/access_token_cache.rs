use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rua_channel::core::access_token::AccessTokenCache;

fn make_counter_fetch(
    counter: Arc<AtomicU32>,
    ttl: Duration,
) -> rua_channel::core::access_token::FetchTokenFn {
    Box::new(move || {
        let counter = counter.clone();
        let ttl = ttl;
        Box::pin(async move {
            let n = counter.fetch_add(1, Ordering::SeqCst);
            Ok((format!("token-{n}"), ttl))
        })
    })
}

#[tokio::test]
async fn cache_hit_returns_same_token() {
    let counter = Arc::new(AtomicU32::new(0));
    let cache = AccessTokenCache::new(make_counter_fetch(counter.clone(), Duration::from_secs(60)));

    let t1 = cache.get_or_refresh().await.unwrap();
    let t2 = cache.get_or_refresh().await.unwrap();

    assert_eq!(t1, "token-0");
    assert_eq!(t2, "token-0");
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn expired_token_triggers_refresh() {
    let counter = Arc::new(AtomicU32::new(0));
    let cache = AccessTokenCache::new(make_counter_fetch(
        counter.clone(),
        Duration::from_millis(50),
    ));

    let t1 = cache.get_or_refresh().await.unwrap();
    assert_eq!(t1, "token-0");

    tokio::time::sleep(Duration::from_millis(100)).await;

    let t2 = cache.get_or_refresh().await.unwrap();
    assert_eq!(t2, "token-1");
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn concurrent_refresh_only_fetches_once() {
    let counter = Arc::new(AtomicU32::new(0));
    let cache = Arc::new(AccessTokenCache::new(make_counter_fetch(
        counter.clone(),
        Duration::from_secs(60),
    )));

    let mut handles = vec![];
    for _ in 0..10 {
        let c = cache.clone();
        handles.push(tokio::spawn(
            async move { c.get_or_refresh().await.unwrap() },
        ));
    }

    let mut tokens = vec![];
    for h in handles {
        tokens.push(h.await.unwrap());
    }

    for t in &tokens {
        assert_eq!(t, "token-0");
    }
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}
