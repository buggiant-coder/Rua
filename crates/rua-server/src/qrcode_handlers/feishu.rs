use std::sync::Arc;

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;

use crate::errors::ApiError;
use crate::qrcode_auth::{PollResult, QRCodeAuthHandler, QRCodeAuthParams, QRCodeResult};
use crate::qrcode_handlers::http_client::qrcode_http_client;

const FEISHU_ACCOUNTS_DOMAIN: &str = "https://accounts.feishu.cn";
const LARK_ACCOUNTS_DOMAIN: &str = "https://accounts.larksuite.com";
const FEISHU_REGISTER_ENDPOINT: &str = "/oauth/v1/app/registration";
const FEISHU_SOURCE: &str = "RUA";
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

pub struct FeishuQRCodeAuthHandler {
    http: Client,
    feishu_accounts_base: String,
    lark_accounts_base: String,
}

impl FeishuQRCodeAuthHandler {
    pub fn new() -> Self {
        Self {
            http: qrcode_http_client(REQUEST_TIMEOUT),
            feishu_accounts_base: FEISHU_ACCOUNTS_DOMAIN.to_string(),
            lark_accounts_base: LARK_ACCOUNTS_DOMAIN.to_string(),
        }
    }

    pub fn with_accounts_base(base_url: impl Into<String>) -> Self {
        let base = base_url.into().trim_end_matches('/').to_string();
        Self {
            http: qrcode_http_client(REQUEST_TIMEOUT),
            feishu_accounts_base: base.clone(),
            lark_accounts_base: base,
        }
    }

    pub fn with_domain_bases(
        feishu_accounts_base: impl Into<String>,
        lark_accounts_base: impl Into<String>,
    ) -> Self {
        Self {
            http: qrcode_http_client(REQUEST_TIMEOUT),
            feishu_accounts_base: feishu_accounts_base
                .into()
                .trim_end_matches('/')
                .to_string(),
            lark_accounts_base: lark_accounts_base.into().trim_end_matches('/').to_string(),
        }
    }

    pub fn into_arc(self) -> Arc<Self> {
        Arc::new(self)
    }

    fn accounts_base(&self, params: &QRCodeAuthParams) -> &str {
        match params.domain.as_deref() {
            Some("lark") => &self.lark_accounts_base,
            _ => &self.feishu_accounts_base,
        }
    }
}

impl Default for FeishuQRCodeAuthHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize)]
struct FeishuInitResponse {
    #[serde(default)]
    supported_auth_methods: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct FeishuBeginResponse {
    #[serde(default)]
    device_code: String,
    #[serde(default)]
    verification_uri_complete: String,
}

#[derive(Debug, Deserialize)]
struct FeishuPollResponse {
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    client_secret: String,
    #[serde(default)]
    user_info: FeishuUserInfo,
    #[serde(default)]
    error: String,
}

#[derive(Debug, Deserialize, Default)]
struct FeishuUserInfo {
    #[serde(default)]
    open_id: String,
    #[serde(default)]
    tenant_brand: String,
}

fn append_source(scan_url: &str) -> String {
    if scan_url.contains('?') {
        format!("{scan_url}&source={FEISHU_SOURCE}")
    } else {
        format!("{scan_url}?source={FEISHU_SOURCE}")
    }
}

#[async_trait]
impl QRCodeAuthHandler for FeishuQRCodeAuthHandler {
    async fn fetch_qrcode(&self, agent_id: &str) -> Result<QRCodeResult, ApiError> {
        self.fetch_qrcode_with_params(agent_id, &QRCodeAuthParams::default())
            .await
    }

    async fn poll_status(&self, token: &str, agent_id: &str) -> Result<PollResult, ApiError> {
        self.poll_status_with_params(token, agent_id, &QRCodeAuthParams::default())
            .await
    }

    async fn fetch_qrcode_with_params(
        &self,
        _agent_id: &str,
        params: &QRCodeAuthParams,
    ) -> Result<QRCodeResult, ApiError> {
        let endpoint = format!("{}{}", self.accounts_base(params), FEISHU_REGISTER_ENDPOINT);
        let init_data: FeishuInitResponse = self
            .http
            .post(&endpoint)
            .form(&[("action", "init")])
            .send()
            .await
            .map_err(|e| ApiError::Internal(format!("Feishu QR code fetch failed: {e}")))?
            .error_for_status()
            .map_err(|e| ApiError::Internal(format!("Feishu init failed: {e}")))?
            .json()
            .await
            .map_err(|e| ApiError::Internal(format!("Feishu init parse failed: {e}")))?;

        if !init_data
            .supported_auth_methods
            .iter()
            .any(|method| method == "client_secret")
        {
            return Err(ApiError::Internal(
                "Feishu: unsupported auth methods".to_string(),
            ));
        }

        let begin_data: FeishuBeginResponse = self
            .http
            .post(&endpoint)
            .form(&[
                ("action", "begin"),
                ("archetype", "PersonalAgent"),
                ("auth_method", "client_secret"),
                ("request_user_info", "open_id"),
            ])
            .send()
            .await
            .map_err(|e| ApiError::Internal(format!("Feishu QR code fetch failed: {e}")))?
            .error_for_status()
            .map_err(|e| ApiError::Internal(format!("Feishu begin failed: {e}")))?
            .json()
            .await
            .map_err(|e| ApiError::Internal(format!("Feishu begin parse failed: {e}")))?;

        if begin_data.device_code.is_empty() || begin_data.verification_uri_complete.is_empty() {
            return Err(ApiError::Internal(
                "Feishu: missing device_code or QR URL".to_string(),
            ));
        }

        Ok(QRCodeResult {
            scan_url: append_source(&begin_data.verification_uri_complete),
            poll_token: begin_data.device_code,
        })
    }

    async fn poll_status_with_params(
        &self,
        token: &str,
        _agent_id: &str,
        params: &QRCodeAuthParams,
    ) -> Result<PollResult, ApiError> {
        let endpoint = format!("{}{}", self.accounts_base(params), FEISHU_REGISTER_ENDPOINT);
        let data: FeishuPollResponse = self
            .http
            .post(endpoint)
            .form(&[("action", "poll"), ("device_code", token)])
            .send()
            .await
            .map_err(|e| ApiError::Internal(format!("Feishu status check failed: {e}")))?
            .json()
            .await
            .map_err(|e| ApiError::Internal(format!("Feishu poll parse failed: {e}")))?;

        if !data.client_id.is_empty() && !data.client_secret.is_empty() {
            return Ok(PollResult {
                status: "success".to_string(),
                credentials: serde_json::json!({
                    "app_id": data.client_id,
                    "app_secret": data.client_secret,
                    "open_id": data.user_info.open_id,
                    "tenant_brand": if data.user_info.tenant_brand.is_empty() {
                        "feishu"
                    } else {
                        data.user_info.tenant_brand.as_str()
                    },
                }),
            });
        }

        match data.error.as_str() {
            "expired_token" | "invalid_grant" => Ok(PollResult {
                status: "expired".to_string(),
                credentials: serde_json::json!({"fail_reason": "QR code expired"}),
            }),
            "access_denied" => Ok(PollResult {
                status: "fail".to_string(),
                credentials: serde_json::json!({"fail_reason": "User denied authorization"}),
            }),
            "" | "authorization_pending" | "slow_down" => Ok(PollResult {
                status: "waiting".to_string(),
                credentials: serde_json::json!({}),
            }),
            other => Ok(PollResult {
                status: "fail".to_string(),
                credentials: serde_json::json!({"fail_reason": other}),
            }),
        }
    }
}
