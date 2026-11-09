use std::time::Duration;

use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use aes::{Aes128, Aes192, Aes256};
use base64::Engine;
use md5::{Digest, Md5};
use rand::RngCore;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::http_client::build_http_client;

use rua_core::{Result, RuaError};

const DEFAULT_BASE_URL: &str = "https://ilinkai.weixin.qq.com";
const CHANNEL_VERSION: &str = "2.0.1";
const GETUPDATES_TIMEOUT: Duration = Duration::from_secs(45);
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);
const QRCODE_STATUS_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QrCodeResponse {
    pub qrcode: String,
    #[serde(default)]
    pub qrcode_img_content: String,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QrCodeStatus {
    pub status: String,
    #[serde(default)]
    pub bot_token: String,
    #[serde(default)]
    pub baseurl: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetUpdatesResponse {
    #[serde(default)]
    pub ret: i32,
    #[serde(default)]
    pub msgs: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    pub get_updates_buf: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WechatUploadResult {
    pub encrypt_query_param: String,
    pub aes_key_b64: String,
    pub filesize: usize,
}

#[derive(Debug, Clone)]
pub struct ILinkClient {
    pub bot_token: String,
    pub base_url: String,
    client: Client,
}

impl ILinkClient {
    pub fn new(bot_token: String, base_url: Option<String>) -> Self {
        let client = build_http_client(Client::builder().timeout(GETUPDATES_TIMEOUT));
        Self {
            bot_token,
            base_url: base_url
                .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
                .trim_end_matches('/')
                .to_string(),
            client,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url, path.trim_start_matches('/'))
    }

    fn headers(&self) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        if !self.bot_token.is_empty() {
            if let Ok(val) =
                reqwest::header::HeaderValue::from_str(&format!("Bearer {}", self.bot_token))
            {
                headers.insert(reqwest::header::AUTHORIZATION, val);
            }
        }
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            reqwest::header::HeaderValue::from_static("application/json"),
        );
        headers
    }

    pub async fn get_bot_qrcode(&self) -> Result<QrCodeResponse> {
        let resp = self
            .client
            .get(self.url("ilink/bot/get_bot_qrcode"))
            .query(&[("bot_type", "3")])
            .headers(self.headers())
            .timeout(DEFAULT_TIMEOUT)
            .send()
            .await
            .map_err(|e| RuaError::Runtime {
                message: format!("get_bot_qrcode request failed: {e}"),
            })?;
        let data: QrCodeResponse = resp.json().await.map_err(|e| RuaError::Runtime {
            message: format!("get_bot_qrcode parse failed: {e}"),
        })?;
        Ok(data)
    }

    pub async fn get_qrcode_status(&self, qrcode: &str) -> Result<QrCodeStatus> {
        let resp = self
            .client
            .get(self.url("ilink/bot/get_qrcode_status"))
            .query(&[("qrcode", qrcode)])
            .headers(self.headers())
            .timeout(QRCODE_STATUS_TIMEOUT)
            .send()
            .await
            .map_err(|e| RuaError::Runtime {
                message: format!("get_qrcode_status request failed: {e}"),
            })?;
        let data: QrCodeStatus = resp.json().await.map_err(|e| RuaError::Runtime {
            message: format!("get_qrcode_status parse failed: {e}"),
        })?;
        Ok(data)
    }

    pub async fn wait_for_login(&self, qrcode: &str) -> Result<(String, String)> {
        let max_wait = Duration::from_secs(300);
        let poll_interval = Duration::from_millis(1500);
        let start = std::time::Instant::now();

        loop {
            if start.elapsed() >= max_wait {
                return Err(RuaError::Runtime {
                    message: "wechat: QR code not scanned within 300s".into(),
                });
            }
            match self.get_qrcode_status(qrcode).await {
                Ok(status) => match status.status.as_str() {
                    "confirmed" => {
                        let base_url = if status.baseurl.is_empty() {
                            self.base_url.clone()
                        } else {
                            status.baseurl
                        };
                        return Ok((status.bot_token, base_url));
                    }
                    "expired" => {
                        return Err(RuaError::Runtime {
                            message: "wechat: QR code expired".into(),
                        });
                    }
                    _ => {}
                },
                Err(e) => {
                    tracing::warn!("wechat: QR status poll error: {e}");
                }
            }
            tokio::time::sleep(poll_interval).await;
        }
    }

    pub async fn getupdates(&self, cursor: &str) -> Result<GetUpdatesResponse> {
        let body = serde_json::json!({
            "get_updates_buf": cursor,
            "base_info": { "channel_version": CHANNEL_VERSION },
        });
        let resp = self
            .client
            .post(self.url("ilink/bot/getupdates"))
            .headers(self.headers())
            .json(&body)
            .timeout(GETUPDATES_TIMEOUT)
            .send()
            .await
            .map_err(|e| RuaError::Runtime {
                message: format!("getupdates request failed: {e}"),
            })?;
        let data: GetUpdatesResponse = resp.json().await.map_err(|e| RuaError::Runtime {
            message: format!("getupdates parse failed: {e}"),
        })?;
        Ok(data)
    }

    pub async fn sendmessage(
        &self,
        to_user_id: &str,
        text: &str,
        context_token: &str,
    ) -> Result<serde_json::Value> {
        let msg = serde_json::json!({
            "from_user_id": "",
            "to_user_id": to_user_id,
            "client_id": uuid::Uuid::new_v4().to_string(),
            "message_type": 2,
            "message_state": 2,
            "context_token": context_token,
            "item_list": [{ "type": 1, "text_item": { "text": text } }],
        });
        self.send_raw_message(msg).await
    }

    pub async fn getconfig(
        &self,
        ilink_user_id: &str,
        context_token: &str,
    ) -> Result<serde_json::Value> {
        let body = serde_json::json!({
            "ilink_user_id": ilink_user_id,
            "context_token": context_token,
            "base_info": { "channel_version": CHANNEL_VERSION },
        });
        let resp = self
            .client
            .post(self.url("ilink/bot/getconfig"))
            .headers(self.headers())
            .json(&body)
            .timeout(DEFAULT_TIMEOUT)
            .send()
            .await
            .map_err(|e| RuaError::Runtime {
                message: format!("getconfig request failed: {e}"),
            })?;
        let data: serde_json::Value = resp.json().await.map_err(|e| RuaError::Runtime {
            message: format!("getconfig parse failed: {e}"),
        })?;
        Ok(data)
    }

    pub async fn sendtyping(
        &self,
        to_user_id: &str,
        typing_ticket: &str,
        status: i32,
    ) -> Result<serde_json::Value> {
        let body = serde_json::json!({
            "ilink_user_id": to_user_id,
            "typing_ticket": typing_ticket,
            "status": status,
            "base_info": { "channel_version": CHANNEL_VERSION },
        });
        let resp = self
            .client
            .post(self.url("ilink/bot/sendtyping"))
            .headers(self.headers())
            .json(&body)
            .timeout(DEFAULT_TIMEOUT)
            .send()
            .await
            .map_err(|e| RuaError::Runtime {
                message: format!("sendtyping request failed: {e}"),
            })?;
        let data: serde_json::Value = resp.json().await.map_err(|e| RuaError::Runtime {
            message: format!("sendtyping parse failed: {e}"),
        })?;
        Ok(data)
    }

    pub async fn send_raw_message(&self, msg: serde_json::Value) -> Result<serde_json::Value> {
        let body = serde_json::json!({
            "msg": msg,
            "base_info": { "channel_version": CHANNEL_VERSION },
        });
        let resp = self
            .client
            .post(self.url("ilink/bot/sendmessage"))
            .headers(self.headers())
            .json(&body)
            .timeout(DEFAULT_TIMEOUT)
            .send()
            .await
            .map_err(|e| RuaError::Runtime {
                message: format!("sendmessage request failed: {e}"),
            })?;
        let data: serde_json::Value = resp.json().await.map_err(|e| RuaError::Runtime {
            message: format!("sendmessage parse failed: {e}"),
        })?;
        Ok(data)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn getuploadurl(
        &self,
        filekey: &str,
        media_type: u8,
        to_user_id: &str,
        rawsize: usize,
        rawfilemd5: &str,
        filesize: usize,
        aeskey: &str,
    ) -> Result<serde_json::Value> {
        let body = serde_json::json!({
            "filekey": filekey,
            "media_type": media_type,
            "to_user_id": to_user_id,
            "rawsize": rawsize,
            "rawfilemd5": rawfilemd5,
            "filesize": filesize,
            "aeskey": aeskey,
            "no_need_thumb": true,
            "base_info": { "channel_version": CHANNEL_VERSION },
        });
        let resp = self
            .client
            .post(self.url("ilink/bot/getuploadurl"))
            .headers(self.headers())
            .json(&body)
            .timeout(DEFAULT_TIMEOUT)
            .send()
            .await
            .map_err(|e| RuaError::Runtime {
                message: format!("getuploadurl request failed: {e}"),
            })?;
        let data: serde_json::Value = resp.json().await.map_err(|e| RuaError::Runtime {
            message: format!("getuploadurl parse failed: {e}"),
        })?;
        Ok(data)
    }

    pub async fn upload_media(
        &self,
        file_path: &std::path::Path,
        media_type: u8,
        to_user_id: &str,
    ) -> Result<WechatUploadResult> {
        let raw_data = tokio::fs::read(file_path).await.map_err(|e| RuaError::Io {
            message: format!("read wechat media {}: {e}", file_path.display()),
        })?;
        let rawsize = raw_data.len();
        let mut md5 = Md5::new();
        md5.update(&raw_data);
        let rawfilemd5 = format!("{:x}", md5.finalize());

        let mut aes_key_raw = [0_u8; 16];
        rand::thread_rng().fill_bytes(&mut aes_key_raw);
        let aes_key_hex = hex_lower(&aes_key_raw);
        let aes_key_b64 = base64::engine::general_purpose::STANDARD.encode(aes_key_hex.as_bytes());
        let aes_key_b64_for_encrypt = base64::engine::general_purpose::STANDARD.encode(aes_key_raw);
        let filekey = uuid::Uuid::new_v4().simple().to_string();
        let encrypted_data =
            aes_ecb_encrypt_pkcs7(&raw_data, &aes_key_b64_for_encrypt).map_err(|e| {
                RuaError::Runtime {
                    message: format!("wechat media encrypt failed: {e}"),
                }
            })?;
        let filesize = encrypted_data.len();

        let upload_resp = self
            .getuploadurl(
                &filekey,
                media_type,
                to_user_id,
                rawsize,
                &rawfilemd5,
                filesize,
                &aes_key_hex,
            )
            .await?;
        let upload_url = upload_resp
            .get("upload_full_url")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .or_else(|| {
                upload_resp
                    .get("upload_param")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.trim().is_empty())
                    .map(|param| {
                        format!(
                            "https://novac2c.cdn.weixin.qq.com/c2c/upload?encrypted_query_param={param}&filekey={filekey}"
                        )
                    })
            })
            .ok_or_else(|| RuaError::Runtime {
                message: format!("wechat getuploadurl returned no upload URL: {upload_resp}"),
            })?;

        let resp = self
            .client
            .post(upload_url)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(encrypted_data)
            .timeout(Duration::from_secs(120))
            .send()
            .await
            .map_err(|e| RuaError::Runtime {
                message: format!("wechat media CDN upload failed: {e}"),
            })?;
        if !resp.status().is_success() {
            return Err(RuaError::Runtime {
                message: format!("wechat media CDN upload HTTP {}", resp.status()),
            });
        }
        let encrypt_query_param = resp
            .headers()
            .get("x-encrypted-param")
            .or_else(|| resp.headers().get("X-Encrypted-Param"))
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .ok_or_else(|| RuaError::Runtime {
                message: "wechat media CDN upload returned no x-encrypted-param".into(),
            })?;

        Ok(WechatUploadResult {
            encrypt_query_param,
            aes_key_b64,
            filesize,
        })
    }

    pub async fn send_file(
        &self,
        to_user_id: &str,
        file_path: &std::path::Path,
        filename: &str,
        context_token: &str,
    ) -> Result<serde_json::Value> {
        let upload = self.upload_media(file_path, 3, to_user_id).await?;
        self.send_raw_message(serde_json::json!({
            "to_user_id": to_user_id,
            "client_id": uuid::Uuid::new_v4().to_string(),
            "message_type": 2,
            "message_state": 2,
            "context_token": context_token,
            "item_list": [{
                "type": 4,
                "file_item": {
                    "media": media_json(&upload),
                    "file_name": filename,
                    "len": upload.filesize.to_string(),
                }
            }],
        }))
        .await
    }

    pub async fn send_image(
        &self,
        to_user_id: &str,
        image_path: &std::path::Path,
        context_token: &str,
    ) -> Result<serde_json::Value> {
        let upload = self.upload_media(image_path, 1, to_user_id).await?;
        self.send_raw_message(serde_json::json!({
            "to_user_id": to_user_id,
            "client_id": uuid::Uuid::new_v4().to_string(),
            "message_type": 2,
            "message_state": 2,
            "context_token": context_token,
            "item_list": [{
                "type": 2,
                "image_item": {
                    "media": media_json(&upload),
                    "mid_size": upload.filesize,
                }
            }],
        }))
        .await
    }

    pub async fn send_video(
        &self,
        to_user_id: &str,
        video_path: &std::path::Path,
        context_token: &str,
    ) -> Result<serde_json::Value> {
        let upload = self.upload_media(video_path, 2, to_user_id).await?;
        self.send_raw_message(serde_json::json!({
            "to_user_id": to_user_id,
            "client_id": uuid::Uuid::new_v4().to_string(),
            "message_type": 2,
            "message_state": 2,
            "context_token": context_token,
            "item_list": [{
                "type": 5,
                "video_item": {
                    "media": media_json(&upload),
                }
            }],
        }))
        .await
    }

    pub async fn send_audio(
        &self,
        to_user_id: &str,
        audio_path: &std::path::Path,
        filename: &str,
        context_token: &str,
    ) -> Result<serde_json::Value> {
        let upload = self.upload_media(audio_path, 4, to_user_id).await?;
        self.send_raw_message(serde_json::json!({
            "to_user_id": to_user_id,
            "client_id": uuid::Uuid::new_v4().to_string(),
            "message_type": 2,
            "message_state": 2,
            "context_token": context_token,
            "item_list": [{
                "type": 4,
                "file_item": {
                    "media": media_json(&upload),
                    "file_name": filename,
                    "len": upload.filesize.to_string(),
                }
            }],
        }))
        .await
    }

    pub async fn download_media(
        &self,
        url: &str,
        aes_key: &str,
        encrypt_query_param: &str,
    ) -> Result<(Vec<u8>, Option<String>)> {
        let download_url = if !encrypt_query_param.trim().is_empty() {
            format!(
                "https://novac2c.cdn.weixin.qq.com/c2c/download?encrypted_query_param={}",
                encrypt_query_param.trim()
            )
        } else if url.starts_with("http://") || url.starts_with("https://") {
            url.to_string()
        } else {
            return Err(RuaError::Validation {
                message: "wechat media requires HTTP URL or encrypt_query_param".into(),
            });
        };
        let resp = self
            .client
            .get(download_url)
            .timeout(Duration::from_secs(60))
            .send()
            .await
            .map_err(|e| RuaError::Runtime {
                message: format!("wechat media download failed: {e}"),
            })?;
        if !resp.status().is_success() {
            return Err(RuaError::Runtime {
                message: format!("wechat media download HTTP {}", resp.status()),
            });
        }
        let mime = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        let bytes = resp.bytes().await.map_err(|e| RuaError::Runtime {
            message: format!("wechat media download bytes failed: {e}"),
        })?;
        let bytes = if aes_key.trim().is_empty() {
            bytes.to_vec()
        } else {
            aes_ecb_decrypt_pkcs7(&bytes, aes_key).map_err(|e| RuaError::Runtime {
                message: format!("wechat media decrypt failed: {e}"),
            })?
        };
        Ok((bytes, mime))
    }
}

fn media_json(upload: &WechatUploadResult) -> serde_json::Value {
    serde_json::json!({
        "encrypt_query_param": upload.encrypt_query_param,
        "aes_key": upload.aes_key_b64,
        "encrypt_type": 1,
    })
}

fn aes_ecb_decrypt_pkcs7(data: &[u8], key_hint: &str) -> std::result::Result<Vec<u8>, String> {
    if data.is_empty() || !data.len().is_multiple_of(16) {
        return Err("ciphertext length must be a non-empty multiple of 16".into());
    }
    let key = parse_aes_key(key_hint)?;
    let mut out = data.to_vec();
    match key.len() {
        16 => {
            let cipher = Aes128::new_from_slice(&key).map_err(|err| err.to_string())?;
            for block in out.chunks_exact_mut(16) {
                cipher.decrypt_block(aes::cipher::generic_array::GenericArray::from_mut_slice(
                    block,
                ));
            }
        }
        24 => {
            let cipher = Aes192::new_from_slice(&key).map_err(|err| err.to_string())?;
            for block in out.chunks_exact_mut(16) {
                cipher.decrypt_block(aes::cipher::generic_array::GenericArray::from_mut_slice(
                    block,
                ));
            }
        }
        32 => {
            let cipher = Aes256::new_from_slice(&key).map_err(|err| err.to_string())?;
            for block in out.chunks_exact_mut(16) {
                cipher.decrypt_block(aes::cipher::generic_array::GenericArray::from_mut_slice(
                    block,
                ));
            }
        }
        len => return Err(format!("invalid AES key length: {len}")),
    }
    unpad_pkcs7(&out)
}

fn aes_ecb_encrypt_pkcs7(data: &[u8], key_hint: &str) -> std::result::Result<Vec<u8>, String> {
    let key = parse_aes_key(key_hint)?;
    let mut out = pad_pkcs7(data);
    match key.len() {
        16 => {
            let cipher = Aes128::new_from_slice(&key).map_err(|err| err.to_string())?;
            for block in out.chunks_exact_mut(16) {
                cipher.encrypt_block(aes::cipher::generic_array::GenericArray::from_mut_slice(
                    block,
                ));
            }
        }
        24 => {
            let cipher = Aes192::new_from_slice(&key).map_err(|err| err.to_string())?;
            for block in out.chunks_exact_mut(16) {
                cipher.encrypt_block(aes::cipher::generic_array::GenericArray::from_mut_slice(
                    block,
                ));
            }
        }
        32 => {
            let cipher = Aes256::new_from_slice(&key).map_err(|err| err.to_string())?;
            for block in out.chunks_exact_mut(16) {
                cipher.encrypt_block(aes::cipher::generic_array::GenericArray::from_mut_slice(
                    block,
                ));
            }
        }
        len => return Err(format!("invalid AES key length: {len}")),
    }
    Ok(out)
}

fn pad_pkcs7(data: &[u8]) -> Vec<u8> {
    let pad = 16 - (data.len() % 16);
    let mut out = Vec::with_capacity(data.len() + pad);
    out.extend_from_slice(data);
    out.extend(std::iter::repeat_n(pad as u8, pad));
    out
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn parse_aes_key(raw: &str) -> std::result::Result<Vec<u8>, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("missing AES key".into());
    }
    if matches!(raw.len(), 32 | 48 | 64) && raw.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return decode_hex(raw);
    }
    let padded = {
        let mut value = raw.to_string();
        while !value.len().is_multiple_of(4) {
            value.push('=');
        }
        value
    };
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(padded.as_bytes())
        .unwrap_or_else(|_| raw.as_bytes().to_vec());
    if decoded.len() == 32 && decoded.iter().all(u8::is_ascii_hexdigit) {
        let hex = std::str::from_utf8(&decoded).map_err(|err| err.to_string())?;
        return decode_hex(hex);
    }
    Ok(decoded)
}

fn decode_hex(raw: &str) -> std::result::Result<Vec<u8>, String> {
    if !raw.len().is_multiple_of(2) {
        return Err("hex key length must be even".into());
    }
    let mut out = Vec::with_capacity(raw.len() / 2);
    for idx in (0..raw.len()).step_by(2) {
        let byte = u8::from_str_radix(&raw[idx..idx + 2], 16).map_err(|err| err.to_string())?;
        out.push(byte);
    }
    Ok(out)
}

fn unpad_pkcs7(data: &[u8]) -> std::result::Result<Vec<u8>, String> {
    let Some(&pad) = data.last() else {
        return Err("empty plaintext".into());
    };
    let pad = pad as usize;
    if pad == 0 || pad > 16 || pad > data.len() {
        return Err("invalid PKCS7 padding".into());
    }
    if !data[data.len() - pad..]
        .iter()
        .all(|byte| *byte as usize == pad)
    {
        return Err("invalid PKCS7 padding bytes".into());
    }
    Ok(data[..data.len() - pad].to_vec())
}
