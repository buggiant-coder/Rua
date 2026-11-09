use std::sync::Arc;

use async_trait::async_trait;

use crate::errors::ApiError;
use crate::qrcode_auth::{PollResult, QRCodeAuthHandler, QRCodeResult};

use super::ilink_client::ILinkClient;

const WEIXIN_SCAN_URL_TEMPLATE: &str = "https://liteapp.weixin.qq.com/q/7GiQu1";

pub struct WeixinQRCodeAuthHandler {
    client: ILinkClient,
}

impl WeixinQRCodeAuthHandler {
    pub fn new() -> Self {
        Self {
            client: ILinkClient::new(None),
        }
    }

    pub fn with_ilink_base_url(base_url: impl AsRef<str>) -> Self {
        Self {
            client: ILinkClient::new(Some(base_url.as_ref())),
        }
    }

    pub fn into_arc(self) -> Arc<Self> {
        Arc::new(self)
    }
}

impl Default for WeixinQRCodeAuthHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl QRCodeAuthHandler for WeixinQRCodeAuthHandler {
    async fn fetch_qrcode(&self, _agent_id: &str) -> Result<QRCodeResult, ApiError> {
        let qr_data = self
            .client
            .get_bot_qrcode()
            .await
            .map_err(|e| ApiError::Internal(format!("WeChat QR code fetch failed: {e}")))?;

        if qr_data.qrcode.is_empty() && qr_data.qrcode_img_content.is_empty() {
            return Err(ApiError::Internal(
                "WeChat returned empty QR code data".to_string(),
            ));
        }

        let scan_url = if qr_data.qrcode_img_content.starts_with("http") {
            qr_data.qrcode_img_content
        } else {
            format!(
                "{}?qrcode={}&bot_type=3",
                WEIXIN_SCAN_URL_TEMPLATE, qr_data.qrcode
            )
        };

        Ok(QRCodeResult {
            scan_url,
            poll_token: qr_data.qrcode,
        })
    }

    async fn poll_status(&self, token: &str, _agent_id: &str) -> Result<PollResult, ApiError> {
        let data = self
            .client
            .get_qrcode_status(token)
            .await
            .map_err(|e| ApiError::Internal(format!("WeChat status check failed: {e}")))?;

        Ok(PollResult {
            status: if data.status.is_empty() {
                "waiting".to_string()
            } else {
                data.status
            },
            credentials: serde_json::json!({
                "bot_token": data.bot_token,
                "base_url": data.baseurl,
            }),
        })
    }
}
