use std::sync::Arc;
use std::time::Duration;

use hmac::{Hmac, Mac};
use reqwest::Client;
use sha2::Sha256;
use tokio::sync::{Mutex, RwLock};
use tokio::time::Instant;

use crate::http_client::default_http_client;

use super::types::{
    SIGN_MAX_RETRIES, SIGN_RETRY_DELAY_MS, SIGN_TOKEN_PATH, TOKEN_REFRESH_MARGIN_SECS,
};

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone)]
pub struct SignTokenResult {
    pub bot_id: String,
    pub token: String,
    pub source: String,
    pub duration_secs: u64,
}

#[derive(Debug, Clone)]
struct TokenCache {
    data: SignTokenResult,
    expires_at: Instant,
}

pub struct TokenManager {
    app_id: String,
    app_secret: String,
    api_domain: String,
    cache: Arc<RwLock<Option<TokenCache>>>,
    client: Client,
    fetch_lock: Mutex<()>,
}

impl TokenManager {
    pub fn new(app_id: String, app_secret: String, api_domain: String) -> Self {
        Self {
            app_id,
            app_secret,
            api_domain,
            cache: Arc::new(RwLock::new(None)),
            client: default_http_client(),
            fetch_lock: Mutex::new(()),
        }
    }

    pub async fn get_token(&self) -> rua_core::Result<SignTokenResult> {
        if let Some(cached) = self.cache.read().await.as_ref() {
            if Instant::now() < cached.expires_at {
                return Ok(cached.data.clone());
            }
        }
        let _lock = self.fetch_lock.lock().await;

        if let Some(cached) = self.cache.read().await.as_ref() {
            if Instant::now() < cached.expires_at {
                return Ok(cached.data.clone());
            }
        }
        self.do_fetch().await
    }

    pub async fn force_refresh(&self) -> rua_core::Result<SignTokenResult> {
        *self.cache.write().await = None;
        let _lock = self.fetch_lock.lock().await;
        self.do_fetch().await
    }

    async fn do_fetch(&self) -> rua_core::Result<SignTokenResult> {
        let domain = self
            .api_domain
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/');
        let url = format!("https://{domain}{SIGN_TOKEN_PATH}");

        for attempt in 0..=SIGN_MAX_RETRIES {
            let nonce = hex::encode(rand::random::<[u8; 16]>());
            let timestamp = beijing_timestamp();
            let signature = compute_signature(&nonce, &timestamp, &self.app_id, &self.app_secret);

            let body = serde_json::json!({
                "app_key": self.app_id,
                "nonce": nonce,
                "signature": signature,
                "timestamp": timestamp,
            });

            let resp = self
                .client
                .post(&url)
                .json(&body)
                .timeout(Duration::from_secs(10))
                .send()
                .await;

            let resp = match resp {
                Ok(r) => r,
                Err(e) => {
                    if attempt < SIGN_MAX_RETRIES {
                        tracing::warn!("yuanbao: sign-token network error: {e}, retrying...");
                        tokio::time::sleep(Duration::from_millis(SIGN_RETRY_DELAY_MS)).await;
                        continue;
                    }
                    return Err(rua_core::RuaError::Runtime {
                        message: format!("sign-token network error: {e}"),
                    });
                }
            };

            if !resp.status().is_success() {
                let status = resp.status();
                if attempt < SIGN_MAX_RETRIES {
                    tokio::time::sleep(Duration::from_millis(SIGN_RETRY_DELAY_MS)).await;
                    continue;
                }
                return Err(rua_core::RuaError::Runtime {
                    message: format!("sign-token HTTP error: {status}"),
                });
            }

            let result: serde_json::Value =
                resp.json().await.map_err(|e| rua_core::RuaError::Runtime {
                    message: format!("sign-token parse error: {e}"),
                })?;

            let code = result.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
            if code == 0 {
                let data = result.get("data").cloned().unwrap_or_default();
                let token_result = SignTokenResult {
                    bot_id: data
                        .get("bot_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    token: data
                        .get("token")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    source: data
                        .get("source")
                        .and_then(|v| v.as_str())
                        .unwrap_or("bot")
                        .to_string(),
                    duration_secs: data.get("duration").and_then(|v| v.as_u64()).unwrap_or(0),
                };

                tracing::info!("yuanbao: sign-token success bot_id={}", token_result.bot_id);

                if token_result.duration_secs > 0 {
                    let ttl_secs = token_result
                        .duration_secs
                        .saturating_sub(TOKEN_REFRESH_MARGIN_SECS);
                    let ttl = Duration::from_secs(ttl_secs.max(60));
                    *self.cache.write().await = Some(TokenCache {
                        data: token_result.clone(),
                        expires_at: Instant::now() + ttl,
                    });
                }

                return Ok(token_result);
            }

            if code == super::types::RETRYABLE_SIGN_CODE && attempt < SIGN_MAX_RETRIES {
                tracing::warn!("yuanbao: sign-token retryable code={code}, retrying...");
                tokio::time::sleep(Duration::from_millis(SIGN_RETRY_DELAY_MS)).await;
                continue;
            }

            let msg = result.get("msg").and_then(|v| v.as_str()).unwrap_or("");
            return Err(rua_core::RuaError::Runtime {
                message: format!("sign-token error: code={code}, msg={msg}"),
            });
        }

        Err(rua_core::RuaError::Runtime {
            message: "sign-token failed: max retries exceeded".into(),
        })
    }

    pub async fn close(&self) {}

    pub async fn auth_headers(
        &self,
    ) -> rua_core::Result<std::collections::HashMap<String, String>> {
        let token = self.get_token().await?;
        Ok(std::collections::HashMap::from([
            ("X-ID".to_string(), token.bot_id),
            ("X-Token".to_string(), token.token),
            ("X-Source".to_string(), token.source),
        ]))
    }
}

fn compute_signature(nonce: &str, timestamp: &str, app_id: &str, app_secret: &str) -> String {
    let plain = format!("{nonce}{timestamp}{app_id}{app_secret}");
    let mut mac =
        HmacSha256::new_from_slice(app_secret.as_bytes()).expect("HMAC accepts any key size");
    mac.update(plain.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

pub fn beijing_timestamp() -> String {
    use chrono::{FixedOffset, Utc};
    let beijing = FixedOffset::east_opt(8 * 3600).unwrap();
    let now = Utc::now().with_timezone(&beijing);
    now.format("%Y-%m-%dT%H:%M:%S+08:00").to_string()
}

pub fn compute_hmac_signature(
    nonce: &str,
    timestamp: &str,
    app_id: &str,
    app_secret: &str,
) -> String {
    compute_signature(nonce, timestamp, app_id, app_secret)
}
