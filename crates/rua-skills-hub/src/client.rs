use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::cancel::ensure_not_cancelled;
use crate::types::HubError;

pub const RETRYABLE_HTTP_STATUS: &[u16] = &[408, 409, 425, 429, 500, 502, 503, 504];

const USER_AGENT_VALUE: &str = "rua-skills-hub/1.0";
const DEFAULT_HTTP_TIMEOUT_SECS: u64 = 15;
const DEFAULT_RETRIES: u32 = 3;
const DEFAULT_BACKOFF_BASE_MS: u64 = 800;
const DEFAULT_BACKOFF_CAP_MS: u64 = 6_000;
const DEFAULT_GITHUB_CACHE_TTL_SECS: u64 = 300;

pub fn github_cache_ttl() -> Duration {
    let raw = std::env::var("RUA_GITHUB_CACHE_TTL").unwrap_or_default();
    if raw.is_empty() {
        return Duration::from_secs(DEFAULT_GITHUB_CACHE_TTL_SECS);
    }
    raw.parse::<u64>()
        .map(Duration::from_secs)
        .unwrap_or_else(|_| Duration::from_secs(DEFAULT_GITHUB_CACHE_TTL_SECS))
}

fn http_timeout() -> Duration {
    let raw = std::env::var("RUA_SKILLS_HUB_HTTP_TIMEOUT").unwrap_or_default();
    let secs = raw
        .parse::<f64>()
        .ok()
        .map(|v| v.max(3.0))
        .unwrap_or(DEFAULT_HTTP_TIMEOUT_SECS as f64);
    Duration::from_secs_f64(secs)
}

fn http_retries() -> u32 {
    std::env::var("RUA_SKILLS_HUB_HTTP_RETRIES")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(DEFAULT_RETRIES)
}

fn backoff_base_ms() -> u64 {
    std::env::var("RUA_SKILLS_HUB_HTTP_BACKOFF_BASE")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .map(|v| (v.max(0.1) * 1000.0) as u64)
        .unwrap_or(DEFAULT_BACKOFF_BASE_MS)
}

fn backoff_cap_ms() -> u64 {
    std::env::var("RUA_SKILLS_HUB_HTTP_BACKOFF_CAP")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .map(|v| (v.max(0.5) * 1000.0) as u64)
        .unwrap_or(DEFAULT_BACKOFF_CAP_MS)
}

fn compute_backoff_ms(attempt: u32) -> u64 {
    let base = backoff_base_ms();
    let cap = backoff_cap_ms();
    let exp = attempt.saturating_sub(1);
    let scaled = base.saturating_mul(1u64 << exp.min(20));
    scaled.min(cap)
}

#[derive(Debug, Default)]
pub struct GithubCache {
    entries: Mutex<HashMap<(String, String), CacheEntry>>,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    value: serde_json::Value,
    expires_at: Instant,
}

impl GithubCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, op: &str, url: &str) -> Option<serde_json::Value> {
        let mut guard = self.entries.lock().ok()?;
        let key = (op.to_string(), url.to_string());
        match guard.get(&key) {
            Some(entry) if entry.expires_at > Instant::now() => Some(entry.value.clone()),
            Some(_) => {
                guard.remove(&key);
                None
            }
            None => None,
        }
    }

    pub fn put(&self, op: &str, url: &str, value: serde_json::Value, ttl: Duration) {
        if let Ok(mut guard) = self.entries.lock() {
            guard.insert(
                (op.to_string(), url.to_string()),
                CacheEntry {
                    value,
                    expires_at: Instant::now() + ttl,
                },
            );
        }
    }
}

pub struct HubClient {
    inner: reqwest::Client,
    github_cache: GithubCache,
}

impl HubClient {
    pub fn new() -> Result<Self, HubError> {
        Self::with_inner(
            rua_http::build_client(reqwest::Client::builder().timeout(http_timeout()))
                .map_err(|e| HubError::Transport(e.to_string()))?,
        )
    }

    pub fn with_inner(inner: reqwest::Client) -> Result<Self, HubError> {
        Ok(Self {
            inner,
            github_cache: GithubCache::new(),
        })
    }

    pub fn inner(&self) -> &reqwest::Client {
        &self.inner
    }

    pub fn github_cache(&self) -> &GithubCache {
        &self.github_cache
    }
}

impl Default for HubClient {
    fn default() -> Self {
        Self::new().expect("default HubClient should build")
    }
}

fn build_headers(url: &str, accept: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static(USER_AGENT_VALUE));
    if let Ok(value) = HeaderValue::from_str(accept) {
        headers.insert(ACCEPT, value);
    }
    if is_github_api(url) {
        if let Some(token) = std::env::var("GITHUB_TOKEN")
            .ok()
            .or_else(|| std::env::var("GH_TOKEN").ok())
        {
            if let Ok(value) = HeaderValue::from_str(&format!("Bearer {token}")) {
                headers.insert(AUTHORIZATION, value);
            }
        }
    }
    headers
}

pub fn is_github_api(url: &str) -> bool {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_lowercase))
        .map(|h| h.contains("api.github.com"))
        .unwrap_or(false)
}

pub fn convert_github_rate_limit(url: &str, status: u16, body: &str) -> Option<HubError> {
    if status != 403 || !is_github_api(url) {
        return None;
    }
    if body.to_lowercase().contains("rate limit") {
        Some(HubError::RateLimit)
    } else {
        None
    }
}

enum AttemptOutcome<T> {
    Success(T),
    RetryAfterMs(u64),
    Fatal(HubError),
}

async fn run_with_retry<F, Fut, T>(
    url: &str,
    cancel: Option<&CancellationToken>,
    mut attempt_fn: F,
) -> Result<T, HubError>
where
    F: FnMut(u32) -> Fut,
    Fut: std::future::Future<Output = AttemptOutcome<T>>,
{
    let max_attempts = http_retries().saturating_add(1);
    let mut last_error: Option<HubError> = None;
    for attempt in 1..=max_attempts {
        ensure_not_cancelled(cancel)?;
        match attempt_fn(attempt).await {
            AttemptOutcome::Success(value) => return Ok(value),
            AttemptOutcome::RetryAfterMs(delay) if attempt < max_attempts => {
                warn!(
                    url = %url,
                    attempt,
                    max_attempts,
                    delay_ms = delay,
                    "hub HTTP attempt failed, retrying",
                );
                ensure_not_cancelled(cancel)?;
                cancellable_sleep(Duration::from_millis(delay), cancel).await?;
            }
            AttemptOutcome::RetryAfterMs(_) => {
                last_error = Some(HubError::Http {
                    status: 0,
                    body: "exhausted retries".to_string(),
                });
            }
            AttemptOutcome::Fatal(err) => return Err(err),
        }
    }
    Err(last_error.unwrap_or(HubError::Http {
        status: 0,
        body: format!("retries exhausted for {url}"),
    }))
}

async fn cancellable_sleep(
    duration: Duration,
    cancel: Option<&CancellationToken>,
) -> Result<(), HubError> {
    if let Some(token) = cancel {
        tokio::select! {
            _ = tokio::time::sleep(duration) => Ok(()),
            _ = token.cancelled() => Err(HubError::Cancelled),
        }
    } else {
        tokio::time::sleep(duration).await;
        Ok(())
    }
}

async fn read_body_with_cancel(
    response: reqwest::Response,
    cancel: Option<&CancellationToken>,
) -> Result<bytes::Bytes, HubError> {
    use futures::StreamExt;
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        ensure_not_cancelled(cancel)?;
        let chunk = chunk.map_err(HubError::transport)?;
        body.extend_from_slice(&chunk);
    }
    Ok(body.into())
}

pub async fn http_get_bytes_with_retry(
    client: &HubClient,
    url: &str,
    cancel: Option<&CancellationToken>,
) -> Result<bytes::Bytes, HubError> {
    let accept = "application/json, text/plain, */*";
    run_with_retry(url, cancel, |attempt| async move {
        let headers = build_headers(url, accept);
        let send = client.inner.get(url).headers(headers).send().await;
        let response = match send {
            Ok(r) => r,
            Err(e) => {
                if e.is_timeout() || e.is_connect() {
                    return AttemptOutcome::RetryAfterMs(compute_backoff_ms(attempt));
                }
                return AttemptOutcome::Fatal(HubError::transport(e));
            }
        };

        let status = response.status();
        if status.is_success() {
            match read_body_with_cancel(response, cancel).await {
                Ok(bytes) => AttemptOutcome::Success(bytes),
                Err(err) => AttemptOutcome::Fatal(err),
            }
        } else {
            handle_error_status(response, url, status.as_u16(), attempt).await
        }
    })
    .await
}

pub async fn http_get_text_with_retry(
    client: &HubClient,
    url: &str,
    cancel: Option<&CancellationToken>,
) -> Result<String, HubError> {
    let bytes = http_get_bytes_with_retry(client, url, cancel).await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

async fn handle_error_status<T>(
    response: reqwest::Response,
    url: &str,
    status: u16,
    attempt: u32,
) -> AttemptOutcome<T> {
    let body = response.text().await.unwrap_or_default();
    if let Some(err) = convert_github_rate_limit(url, status, &body) {
        return AttemptOutcome::Fatal(err);
    }
    if RETRYABLE_HTTP_STATUS.contains(&status) {
        return AttemptOutcome::RetryAfterMs(compute_backoff_ms(attempt));
    }
    AttemptOutcome::Fatal(HubError::Http { status, body })
}
