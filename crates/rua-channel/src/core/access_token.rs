use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::Instant;

const DEFAULT_TTL: Duration = Duration::from_secs(7200);

struct CachedToken {
    token: String,
    expires_at: Instant,
}

pub type FetchTokenFn = Box<
    dyn Fn() -> Pin<Box<dyn Future<Output = rua_core::Result<(String, Duration)>> + Send>>
        + Send
        + Sync,
>;

pub struct AccessTokenCache {
    inner: Mutex<Option<CachedToken>>,
    fetch: FetchTokenFn,
}

impl AccessTokenCache {
    pub fn new(fetch: FetchTokenFn) -> Self {
        Self {
            inner: Mutex::new(None),
            fetch,
        }
    }

    pub async fn get_or_refresh(&self) -> rua_core::Result<String> {
        let mut guard = self.inner.lock().await;

        if let Some(ref cached) = *guard {
            if Instant::now() < cached.expires_at {
                return Ok(cached.token.clone());
            }
        }

        let (token, ttl) = (self.fetch)().await?;
        let ttl = if ttl.is_zero() { DEFAULT_TTL } else { ttl };
        *guard = Some(CachedToken {
            token: token.clone(),
            expires_at: Instant::now() + ttl,
        });
        Ok(token)
    }

    pub async fn invalidate(&self) {
        let mut guard = self.inner.lock().await;
        *guard = None;
    }
}
