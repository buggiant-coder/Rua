use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;

use crate::errors::ApiError;
use crate::qrcode_auth::{PollResult, QRCodeAuthHandler, QRCodeResult};
use crate::qrcode_handlers::http_client::qrcode_redirect_http_client;

const WECOM_AUTH_ORIGIN: &str = "https://work.weixin.qq.com";
const WECOM_SOURCE: &str = "rua";
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

pub struct WecomQRCodeAuthHandler {
    http: Client,
    base_url: String,
}

impl WecomQRCodeAuthHandler {
    pub fn new() -> Self {
        Self {
            http: qrcode_redirect_http_client(
                REQUEST_TIMEOUT,
                reqwest::redirect::Policy::limited(10),
            ),
            base_url: WECOM_AUTH_ORIGIN.to_string(),
        }
    }

    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            http: qrcode_redirect_http_client(
                REQUEST_TIMEOUT,
                reqwest::redirect::Policy::limited(10),
            ),
            base_url: base_url.into().trim_end_matches('/').to_string(),
        }
    }

    pub fn into_arc(self) -> Arc<Self> {
        Arc::new(self)
    }
}

impl Default for WecomQRCodeAuthHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize)]
struct WecomQueryResponse {
    #[serde(default)]
    data: WecomQueryData,
}

#[derive(Debug, Deserialize, Default)]
struct WecomQueryData {
    #[serde(default)]
    status: String,
    #[serde(default)]
    bot_info: WecomBotInfo,
}

#[derive(Debug, Deserialize, Default)]
struct WecomBotInfo {
    #[serde(default)]
    botid: String,
    #[serde(default)]
    secret: String,
}

fn extract_settings_json(html: &str) -> Option<&str> {
    let marker = "window.settings";
    let marker_pos = html.find(marker)?;
    let after_marker = &html[marker_pos + marker.len()..];
    let eq_pos = after_marker.find('=')?;
    let after_eq = &after_marker[eq_pos + 1..];
    let brace_start = after_eq.find('{')?;
    let json_start = &after_eq[brace_start..];

    let mut depth = 0i32;
    for (i, ch) in json_start.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&json_start[..i + 1]);
                }
            }
            _ => {}
        }
    }
    None
}

#[async_trait]
impl QRCodeAuthHandler for WecomQRCodeAuthHandler {
    async fn fetch_qrcode(&self, _agent_id: &str) -> Result<QRCodeResult, ApiError> {
        let state = uuid::Uuid::new_v4().to_string();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();

        let gen_url = format!(
            "{}/ai/qc/gen?source={}&state={}&timestamp={}",
            self.base_url, WECOM_SOURCE, state, timestamp
        );

        let resp = self
            .http
            .get(&gen_url)
            .send()
            .await
            .map_err(|e| ApiError::Internal(format!("WeCom auth page fetch failed: {e}")))?;

        let html = resp
            .error_for_status()
            .map_err(|e| ApiError::Internal(format!("WeCom auth page fetch failed: {e}")))?
            .text()
            .await
            .map_err(|e| ApiError::Internal(format!("WeCom auth page read failed: {e}")))?;

        let json_str = extract_settings_json(&html).ok_or_else(|| {
            ApiError::Internal("Failed to parse WeCom auth page settings".to_string())
        })?;

        let settings: serde_json::Value = serde_json::from_str(json_str)
            .map_err(|e| ApiError::Internal(format!("Failed to parse WeCom settings JSON: {e}")))?;

        let scode = settings["scode"].as_str().unwrap_or_default();
        let auth_url = settings["auth_url"].as_str().unwrap_or_default();

        if scode.is_empty() || auth_url.is_empty() {
            return Err(ApiError::Internal(
                "WeCom returned empty scode or auth_url".to_string(),
            ));
        }

        Ok(QRCodeResult {
            scan_url: auth_url.to_string(),
            poll_token: scode.to_string(),
        })
    }

    async fn poll_status(&self, token: &str, _agent_id: &str) -> Result<PollResult, ApiError> {
        let query_url = format!("{}/ai/qc/query_result", self.base_url);

        let resp = self
            .http
            .get(&query_url)
            .query(&[("scode", token)])
            .send()
            .await
            .map_err(|e| ApiError::Internal(format!("WeCom status check failed: {e}")))?;

        let result: WecomQueryResponse = resp
            .error_for_status()
            .map_err(|e| ApiError::Internal(format!("WeCom status check failed: {e}")))?
            .json()
            .await
            .map_err(|e| ApiError::Internal(format!("WeCom poll parse failed: {e}")))?;

        let status = if result.data.status.is_empty() {
            "waiting".to_string()
        } else {
            result.data.status
        };

        Ok(PollResult {
            status,
            credentials: serde_json::json!({
                "bot_id": result.data.bot_info.botid,
                "secret": result.data.bot_info.secret,
            }),
        })
    }
}
