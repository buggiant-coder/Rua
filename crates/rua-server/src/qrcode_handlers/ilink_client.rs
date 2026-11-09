use std::collections::HashMap;

use reqwest::Client;
use serde::Deserialize;

use crate::qrcode_handlers::http_client::qrcode_http_client;

const DEFAULT_BASE_URL: &str = "https://ilinkai.weixin.qq.com";
const DEFAULT_TIMEOUT_SECS: u64 = 15;

#[derive(Debug, Deserialize)]
pub struct ILinkQRCodeResponse {
    #[serde(default)]
    pub qrcode: String,
    #[serde(default)]
    pub qrcode_img_content: String,
}

#[derive(Debug, Deserialize)]
pub struct ILinkQRCodeStatusResponse {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub bot_token: String,
    #[serde(default)]
    pub baseurl: String,
}

pub struct ILinkClient {
    http: Client,
    base_url: String,
}

impl ILinkClient {
    pub fn new(base_url: Option<&str>) -> Self {
        let base = base_url
            .unwrap_or(DEFAULT_BASE_URL)
            .trim_end_matches('/')
            .to_string();
        Self {
            http: qrcode_http_client(std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS)),
            base_url: base,
        }
    }

    pub async fn get_bot_qrcode(&self) -> Result<ILinkQRCodeResponse, reqwest::Error> {
        let mut params = HashMap::new();
        params.insert("bot_type", "3");
        self.http
            .get(format!("{}/ilink/bot/get_bot_qrcode", self.base_url))
            .query(&params)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
    }

    pub async fn get_qrcode_status(
        &self,
        qrcode: &str,
    ) -> Result<ILinkQRCodeStatusResponse, reqwest::Error> {
        let mut params = HashMap::new();
        params.insert("qrcode", qrcode);
        self.http
            .get(format!("{}/ilink/bot/get_qrcode_status", self.base_url))
            .query(&params)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
    }
}
