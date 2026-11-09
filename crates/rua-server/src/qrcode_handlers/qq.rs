use std::sync::Arc;

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use async_trait::async_trait;
use base64::Engine;
use rand::RngCore;
use reqwest::Client;
use serde::Deserialize;

use crate::errors::ApiError;
use crate::qrcode_auth::{PollResult, QRCodeAuthHandler, QRCodeResult};
use crate::qrcode_handlers::http_client::qrcode_http_client;

const QQ_PORTAL_BASE: &str = "https://q.qq.com";
const QQ_CREATE_PATH: &str = "/lite/create_bind_task";
const QQ_POLL_PATH: &str = "/lite/poll_bind_result";
const QQ_FRONTEND_PATH: &str = "/qqbot/openclaw/connect.html";
const QQ_SOURCE: &str = "RUA";
const AES_KEY_LEN: usize = 32;
const GCM_NONCE_LEN: usize = 12;
const GCM_TAG_LEN: usize = 16;
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

pub struct QqQRCodeAuthHandler {
    http: Client,
    portal_base: String,
}

impl QqQRCodeAuthHandler {
    pub fn new() -> Self {
        Self {
            http: qrcode_http_client(REQUEST_TIMEOUT),
            portal_base: QQ_PORTAL_BASE.to_string(),
        }
    }

    pub fn into_arc(self) -> Arc<Self> {
        Arc::new(self)
    }

    pub fn with_portal_base(portal_base: impl Into<String>) -> Self {
        Self {
            http: qrcode_http_client(REQUEST_TIMEOUT),
            portal_base: portal_base.into().trim_end_matches('/').to_string(),
        }
    }
}

impl Default for QqQRCodeAuthHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize)]
struct QqCreateResponse {
    #[serde(default)]
    retcode: i64,
    #[serde(default)]
    msg: String,
    #[serde(default)]
    data: QqCreateData,
}

#[derive(Debug, Deserialize, Default)]
struct QqCreateData {
    #[serde(default)]
    task_id: String,
}

#[derive(Debug, Deserialize)]
struct QqPollResponse {
    #[serde(default)]
    retcode: i64,
    #[serde(default)]
    msg: String,
    #[serde(default)]
    data: QqPollData,
}

#[derive(Debug, Deserialize, Default)]
struct QqPollData {
    #[serde(default)]
    status: i64,
    #[serde(default)]
    bot_appid: serde_json::Value,
    #[serde(default)]
    bot_encrypt_secret: String,
    #[serde(default)]
    user_openid: String,
}

fn generate_bind_key() -> String {
    let mut key = [0u8; AES_KEY_LEN];
    rand::rngs::OsRng.fill_bytes(&mut key);
    base64::engine::general_purpose::STANDARD.encode(key)
}

fn encode_poll_token(task_id: &str, aes_key: &str) -> String {
    let payload = serde_json::json!({
        "task_id": task_id,
        "key": aes_key,
    });
    base64::engine::general_purpose::URL_SAFE.encode(payload.to_string())
}

fn decode_poll_token(token: &str) -> Result<(String, String), ApiError> {
    let decoded = base64::engine::general_purpose::URL_SAFE
        .decode(token)
        .map_err(|_| ApiError::BadRequest("Invalid poll token".to_string()))?;
    let value: serde_json::Value = serde_json::from_slice(&decoded)
        .map_err(|_| ApiError::BadRequest("Invalid poll token".to_string()))?;
    let task_id = value
        .get("task_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::BadRequest("Invalid poll token".to_string()))?;
    let key = value
        .get("key")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::BadRequest("Invalid poll token".to_string()))?;
    Ok((task_id.to_string(), key.to_string()))
}

fn decrypt_secret(encrypted_base64: &str, key_base64: &str) -> Result<String, String> {
    let key = base64::engine::general_purpose::STANDARD
        .decode(key_base64)
        .map_err(|e| format!("invalid key base64: {e}"))?;
    if key.len() != AES_KEY_LEN {
        return Err(format!("invalid AES key length: {}", key.len()));
    }
    let raw = base64::engine::general_purpose::STANDARD
        .decode(encrypted_base64)
        .map_err(|e| format!("invalid secret base64: {e}"))?;
    if raw.len() < GCM_NONCE_LEN + GCM_TAG_LEN {
        return Err(format!("ciphertext too short: {}", raw.len()));
    }
    let (nonce_bytes, ciphertext) = raw.split_at(GCM_NONCE_LEN);
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| e.to_string())?;
    let plaintext = cipher
        .decrypt(Nonce::from_slice(nonce_bytes), ciphertext)
        .map_err(|e| e.to_string())?;
    String::from_utf8(plaintext).map_err(|e| e.to_string())
}

fn app_id_to_string(value: &serde_json::Value) -> String {
    value
        .as_str()
        .map(ToOwned::to_owned)
        .or_else(|| value.as_i64().map(|v| v.to_string()))
        .or_else(|| value.as_u64().map(|v| v.to_string()))
        .unwrap_or_default()
}

#[async_trait]
impl QRCodeAuthHandler for QqQRCodeAuthHandler {
    async fn fetch_qrcode(&self, _agent_id: &str) -> Result<QRCodeResult, ApiError> {
        let aes_key = generate_bind_key();
        let data: QqCreateResponse = self
            .http
            .post(format!("{}{}", self.portal_base, QQ_CREATE_PATH))
            .json(&serde_json::json!({"key": aes_key}))
            .send()
            .await
            .map_err(|e| ApiError::Internal(format!("QQ create_bind_task failed: {e}")))?
            .error_for_status()
            .map_err(|e| ApiError::Internal(format!("QQ create_bind_task failed: {e}")))?
            .json()
            .await
            .map_err(|e| ApiError::Internal(format!("QQ create_bind_task parse failed: {e}")))?;

        if data.retcode != 0 {
            return Err(ApiError::Internal(format!(
                "QQ create_bind_task error: {}",
                data.msg
            )));
        }
        if data.data.task_id.is_empty() {
            return Err(ApiError::Internal(
                "QQ create_bind_task returned empty task_id".to_string(),
            ));
        }

        Ok(QRCodeResult {
            scan_url: format!(
                "{}{}?task_id={}&_wv=2&source={}",
                self.portal_base, QQ_FRONTEND_PATH, data.data.task_id, QQ_SOURCE
            ),
            poll_token: encode_poll_token(&data.data.task_id, &aes_key),
        })
    }

    async fn poll_status(&self, token: &str, _agent_id: &str) -> Result<PollResult, ApiError> {
        let (task_id, aes_key) = decode_poll_token(token)?;
        let data: QqPollResponse = self
            .http
            .post(format!("{}{}", self.portal_base, QQ_POLL_PATH))
            .json(&serde_json::json!({"task_id": task_id}))
            .send()
            .await
            .map_err(|e| ApiError::Internal(format!("QQ poll_bind_result failed: {e}")))?
            .error_for_status()
            .map_err(|e| ApiError::Internal(format!("QQ poll_bind_result failed: {e}")))?
            .json()
            .await
            .map_err(|e| ApiError::Internal(format!("QQ poll_bind_result parse failed: {e}")))?;

        if data.retcode != 0 {
            return Ok(PollResult {
                status: "fail".to_string(),
                credentials: serde_json::json!({"fail_reason": data.msg}),
            });
        }

        match data.data.status {
            2 => {
                let app_id = app_id_to_string(&data.data.bot_appid);
                if app_id.is_empty() || data.data.bot_encrypt_secret.is_empty() {
                    return Ok(PollResult {
                        status: "fail".to_string(),
                        credentials: serde_json::json!({
                            "fail_reason": "Missing app_id or secret"
                        }),
                    });
                }
                match decrypt_secret(&data.data.bot_encrypt_secret, &aes_key) {
                    Ok(client_secret) => Ok(PollResult {
                        status: "success".to_string(),
                        credentials: serde_json::json!({
                            "app_id": app_id,
                            "client_secret": client_secret,
                            "user_openid": data.data.user_openid,
                        }),
                    }),
                    Err(_) => Ok(PollResult {
                        status: "fail".to_string(),
                        credentials: serde_json::json!({
                            "fail_reason": "Secret decryption failed"
                        }),
                    }),
                }
            }
            3 => Ok(PollResult {
                status: "expired".to_string(),
                credentials: serde_json::json!({}),
            }),
            _ => Ok(PollResult {
                status: "waiting".to_string(),
                credentials: serde_json::json!({}),
            }),
        }
    }
}
