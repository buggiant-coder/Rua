use std::sync::Arc;

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;

use crate::errors::ApiError;
use crate::qrcode_auth::{PollResult, QRCodeAuthHandler, QRCodeResult};
use crate::qrcode_handlers::http_client::qrcode_http_client;

const DINGTALK_API_BASE: &str = "https://oapi.dingtalk.com";
const DINGTALK_SOURCE: &str = "RUA";
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

pub struct DingtalkQRCodeAuthHandler {
    http: Client,
    api_base_url: String,
}

impl DingtalkQRCodeAuthHandler {
    pub fn new() -> Self {
        Self {
            http: qrcode_http_client(REQUEST_TIMEOUT),
            api_base_url: DINGTALK_API_BASE.to_string(),
        }
    }

    pub fn with_api_base_url(api_base_url: impl Into<String>) -> Self {
        Self {
            http: qrcode_http_client(REQUEST_TIMEOUT),
            api_base_url: api_base_url.into().trim_end_matches('/').to_string(),
        }
    }

    pub fn into_arc(self) -> Arc<Self> {
        Arc::new(self)
    }
}

impl Default for DingtalkQRCodeAuthHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize)]
struct DingtalkInitResponse {
    #[serde(default)]
    errcode: i64,
    #[serde(default)]
    errmsg: String,
    #[serde(default)]
    nonce: String,
}

#[derive(Debug, Deserialize)]
struct DingtalkBeginResponse {
    #[serde(default)]
    errcode: i64,
    #[serde(default)]
    errmsg: String,
    #[serde(default)]
    device_code: String,
    #[serde(default)]
    verification_uri_complete: String,
}

#[derive(Debug, Deserialize)]
struct DingtalkPollResponse {
    #[serde(default)]
    status: String,
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    client_secret: String,
    #[serde(default)]
    fail_reason: String,
}

#[async_trait]
impl QRCodeAuthHandler for DingtalkQRCodeAuthHandler {
    async fn fetch_qrcode(&self, _agent_id: &str) -> Result<QRCodeResult, ApiError> {
        let init_resp = self
            .http
            .post(format!("{}/app/registration/init", self.api_base_url))
            .json(&serde_json::json!({"source": DINGTALK_SOURCE}))
            .send()
            .await
            .map_err(|e| ApiError::Internal(format!("DingTalk QR code fetch failed: {e}")))?;

        let init_data: DingtalkInitResponse = init_resp
            .error_for_status()
            .map_err(|e| ApiError::Internal(format!("DingTalk init failed: {e}")))?
            .json()
            .await
            .map_err(|e| ApiError::Internal(format!("DingTalk init parse failed: {e}")))?;

        if init_data.errcode != 0 {
            return Err(ApiError::Internal(format!(
                "DingTalk init failed: {}",
                init_data.errmsg
            )));
        }

        if init_data.nonce.is_empty() {
            return Err(ApiError::Internal(
                "DingTalk returned empty nonce".to_string(),
            ));
        }

        let begin_resp = self
            .http
            .post(format!("{}/app/registration/begin", self.api_base_url))
            .json(&serde_json::json!({"nonce": init_data.nonce}))
            .send()
            .await
            .map_err(|e| ApiError::Internal(format!("DingTalk QR code fetch failed: {e}")))?;

        let begin_data: DingtalkBeginResponse = begin_resp
            .error_for_status()
            .map_err(|e| ApiError::Internal(format!("DingTalk begin failed: {e}")))?
            .json()
            .await
            .map_err(|e| ApiError::Internal(format!("DingTalk begin parse failed: {e}")))?;

        if begin_data.errcode != 0 {
            return Err(ApiError::Internal(format!(
                "DingTalk begin failed: {}",
                begin_data.errmsg
            )));
        }

        if begin_data.device_code.is_empty() || begin_data.verification_uri_complete.is_empty() {
            return Err(ApiError::Internal(
                "DingTalk returned empty device_code or URI".to_string(),
            ));
        }

        Ok(QRCodeResult {
            scan_url: begin_data.verification_uri_complete,
            poll_token: begin_data.device_code,
        })
    }

    async fn poll_status(&self, token: &str, _agent_id: &str) -> Result<PollResult, ApiError> {
        let resp = self
            .http
            .post(format!("{}/app/registration/poll", self.api_base_url))
            .json(&serde_json::json!({"device_code": token}))
            .send()
            .await
            .map_err(|e| ApiError::Internal(format!("DingTalk status check failed: {e}")))?;

        let data: DingtalkPollResponse = resp
            .error_for_status()
            .map_err(|e| ApiError::Internal(format!("DingTalk status check failed: {e}")))?
            .json()
            .await
            .map_err(|e| ApiError::Internal(format!("DingTalk poll parse failed: {e}")))?;

        match data.status.as_str() {
            "SUCCESS" => Ok(PollResult {
                status: "success".to_string(),
                credentials: serde_json::json!({
                    "client_id": data.client_id,
                    "client_secret": data.client_secret,
                }),
            }),
            "FAIL" => Ok(PollResult {
                status: "fail".to_string(),
                credentials: serde_json::json!({
                    "fail_reason": data.fail_reason,
                }),
            }),
            "EXPIRED" => Ok(PollResult {
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
