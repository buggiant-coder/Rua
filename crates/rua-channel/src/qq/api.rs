use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use tokio::sync::RwLock;

use crate::http_client::default_http_client;
use crate::qq::types::{
    GatewayUrlResponse, TokenResponse, DEFAULT_API_BASE, SANDBOX_API_BASE, TOKEN_URL,
};

struct TokenCache {
    token: String,
    expires_at: Instant,
}

pub struct QqTokenManager {
    app_id: String,
    client_secret: String,
    client: reqwest::Client,
    cache: RwLock<Option<TokenCache>>,
    token_url: String,
}

impl QqTokenManager {
    pub fn new(app_id: String, client_secret: String) -> Self {
        Self {
            app_id,
            client_secret,
            client: default_http_client(),
            cache: RwLock::new(None),
            token_url: TOKEN_URL.to_string(),
        }
    }

    pub fn new_with_token_url(app_id: String, client_secret: String, token_url: String) -> Self {
        Self {
            app_id,
            client_secret,
            client: default_http_client(),
            cache: RwLock::new(None),
            token_url,
        }
    }

    pub async fn get_access_token(&self) -> Result<String, String> {
        {
            let cache = self.cache.read().await;
            if let Some(ref c) = *cache {
                if Instant::now() < c.expires_at - Duration::from_secs(300) {
                    return Ok(c.token.clone());
                }
            }
        }

        let body = serde_json::json!({
            "appId": self.app_id,
            "clientSecret": self.client_secret,
        });

        let resp = self
            .client
            .post(&self.token_url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("token request failed: {e}"))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("token request returned {status}: {text}"));
        }

        let data: TokenResponse = resp
            .json()
            .await
            .map_err(|e| format!("token response parse failed: {e}"))?;

        let expires_at = Instant::now() + Duration::from_secs(data.expires_in);
        let token = data.access_token.clone();

        *self.cache.write().await = Some(TokenCache {
            token: data.access_token,
            expires_at,
        });

        Ok(token)
    }

    pub async fn clear_cache(&self) {
        *self.cache.write().await = None;
    }
}

pub async fn get_gateway_url(
    client: &reqwest::Client,
    token: &str,
    sandbox: bool,
) -> Result<String, String> {
    let base = if sandbox {
        SANDBOX_API_BASE
    } else {
        DEFAULT_API_BASE
    };
    let url = format!("{base}/gateway");

    let resp = client
        .get(&url)
        .header("Authorization", format!("QQBot {token}"))
        .send()
        .await
        .map_err(|e| format!("gateway URL request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("gateway URL returned {status}: {text}"));
    }

    let data: GatewayUrlResponse = resp
        .json()
        .await
        .map_err(|e| format!("gateway URL parse failed: {e}"))?;

    Ok(data.url)
}

pub struct QqSender {
    client: reqwest::Client,
    api_base: String,
}

pub struct SendParams<'a> {
    pub token: &'a str,
    pub message_type: &'a str,
    pub sender_id: &'a str,
    pub channel_id: Option<&'a str>,
    pub group_openid: Option<&'a str>,
    pub guild_id: Option<&'a str>,
    pub text: &'a str,
    pub msg_id: Option<&'a str>,
}

pub struct SendBodyParams<'a> {
    pub token: &'a str,
    pub message_type: &'a str,
    pub sender_id: &'a str,
    pub channel_id: Option<&'a str>,
    pub group_openid: Option<&'a str>,
    pub guild_id: Option<&'a str>,
    pub body: serde_json::Value,
    pub msg_id: Option<&'a str>,
}

pub struct SendMediaParams<'a> {
    pub token: &'a str,
    pub message_type: &'a str,
    pub sender_id: &'a str,
    pub channel_id: Option<&'a str>,
    pub group_openid: Option<&'a str>,
    pub guild_id: Option<&'a str>,
    pub file_type: u8,
    pub url: Option<&'a str>,
    pub file_data: Option<&'a [u8]>,
    pub file_name: Option<&'a str>,
    pub msg_id: Option<&'a str>,
}

impl QqSender {
    pub fn new(sandbox: bool) -> Self {
        Self {
            client: default_http_client(),
            api_base: if sandbox {
                SANDBOX_API_BASE
            } else {
                DEFAULT_API_BASE
            }
            .to_string(),
        }
    }

    pub fn new_with_api_base(api_base: String) -> Self {
        Self {
            client: default_http_client(),
            api_base,
        }
    }

    pub async fn send_message(&self, params: SendParams<'_>) -> Result<(), String> {
        self.send_message_with_format(params, false).await
    }

    pub async fn send_message_with_format(
        &self,
        params: SendParams<'_>,
        markdown: bool,
    ) -> Result<(), String> {
        let body = if markdown {
            serde_json::json!({"msg_type":2,"markdown":{"content":params.text}})
        } else {
            serde_json::json!({"content":params.text,"msg_type":0})
        };
        self.send_message_body(SendBodyParams {
            token: params.token,
            message_type: params.message_type,
            sender_id: params.sender_id,
            channel_id: params.channel_id,
            group_openid: params.group_openid,
            guild_id: params.guild_id,
            body,
            msg_id: params.msg_id,
        })
        .await
    }

    pub async fn send_message_body(&self, params: SendBodyParams<'_>) -> Result<(), String> {
        let (path, use_msg_seq) = self.resolve_send_path(
            params.message_type,
            params.sender_id,
            params.channel_id,
            params.group_openid,
            params.guild_id,
        );

        let url = format!("{}{}", self.api_base, path);
        let mut body = params.body;

        if let Some(mid) = params.msg_id {
            if !mid.is_empty() {
                body["msg_id"] = serde_json::json!(mid);
            }
        }

        if use_msg_seq {
            body["msg_seq"] = serde_json::json!(std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .to_string());
        }

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("QQBot {}", params.token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("send message failed: {e}"))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("send message returned {status}: {text}"));
        }

        Ok(())
    }

    pub async fn ack_interaction(
        &self,
        token: &str,
        interaction_id: &str,
        code: i64,
    ) -> Result<(), String> {
        if interaction_id.trim().is_empty() {
            return Ok(());
        }
        let url = format!("{}/interactions/{}", self.api_base, interaction_id);
        let resp = self
            .client
            .put(&url)
            .header("Authorization", format!("QQBot {token}"))
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({ "code": code }))
            .send()
            .await
            .map_err(|e| format!("ack interaction failed: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("ack interaction returned {status}: {text}"));
        }
        Ok(())
    }

    pub async fn send_media(&self, params: SendMediaParams<'_>) -> Result<bool, String> {
        if matches!(params.message_type, "guild" | "dm") {
            return self.send_guild_or_dm_media(&params).await;
        }
        let Some(target_openid) =
            self.resolve_media_target(params.message_type, params.sender_id, params.group_openid)
        else {
            return Ok(false);
        };
        if params.message_type == "group" && params.file_type == 4 {
            return Ok(false);
        }
        let Some(file_info) = self.upload_media(&params, target_openid).await? else {
            return Ok(false);
        };
        self.send_media_message(&params, target_openid, &file_info)
            .await?;
        Ok(true)
    }

    async fn send_guild_or_dm_media(&self, params: &SendMediaParams<'_>) -> Result<bool, String> {
        if !matches!(params.file_type, 1 | 2) {
            return Ok(false);
        }
        let path = match params.message_type {
            "dm" => params
                .guild_id
                .filter(|guild_id| !guild_id.is_empty())
                .map(|guild_id| format!("/dms/{guild_id}/messages")),
            "guild" => params
                .channel_id
                .filter(|channel_id| !channel_id.is_empty())
                .map(|channel_id| format!("/channels/{channel_id}/messages")),
            _ => None,
        };
        let Some(path) = path else {
            return Ok(false);
        };
        let url = format!("{}{}", self.api_base, path);
        if let Some(media_url) = params.url.filter(|url| !url.is_empty()) {
            let mut body = serde_json::json!({ "image": media_url });
            if let Some(mid) = params.msg_id.filter(|mid| !mid.is_empty()) {
                body["msg_id"] = serde_json::json!(mid);
            }
            let resp = self
                .client
                .post(&url)
                .header("Authorization", format!("QQBot {}", params.token))
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| format!("send guild/dm media failed: {e}"))?;
            let status = resp.status();
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                return Err(format!("send guild/dm media returned {status}: {text}"));
            }
            return Ok(true);
        }
        let Some(data) = params.file_data else {
            return Ok(false);
        };
        let filename = params
            .file_name
            .filter(|name| !name.is_empty())
            .unwrap_or("image");
        let mut form = reqwest::multipart::Form::new().part(
            "file_image",
            reqwest::multipart::Part::bytes(data.to_vec()).file_name(filename.to_string()),
        );
        if let Some(mid) = params.msg_id.filter(|mid| !mid.is_empty()) {
            form = form.text("msg_id", mid.to_string());
        }
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("QQBot {}", params.token))
            .multipart(form)
            .send()
            .await
            .map_err(|e| format!("send guild/dm media upload failed: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!(
                "send guild/dm media upload returned {status}: {text}"
            ));
        }
        Ok(true)
    }

    async fn upload_media(
        &self,
        params: &SendMediaParams<'_>,
        target_openid: &str,
    ) -> Result<Option<String>, String> {
        let path = format!(
            "{}/{}/files",
            self.media_path_prefix(params.message_type)
                .ok_or_else(|| format!("unsupported QQ media type {}", params.message_type))?,
            target_openid
        );
        let url = format!("{}{}", self.api_base, path);
        let mut body = serde_json::json!({
            "file_type": params.file_type,
            "srv_send_msg": false,
        });
        if let Some(data) = params.file_data {
            body["file_data"] =
                serde_json::json!(base64::engine::general_purpose::STANDARD.encode(data));
            if let Some(name) = params.file_name.filter(|name| !name.is_empty()) {
                body["file_name"] = serde_json::json!(name);
            }
        } else if let Some(media_url) = params.url.filter(|url| !url.is_empty()) {
            body["url"] = serde_json::json!(media_url);
        } else {
            return Ok(None);
        }

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("QQBot {}", params.token))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("upload media failed: {e}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!("upload media returned {status}: {text}"));
        }
        let body: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("upload media parse failed: {e}"))?;
        Ok(body
            .get("file_info")
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
            .map(str::to_string))
    }

    async fn send_media_message(
        &self,
        params: &SendMediaParams<'_>,
        target_openid: &str,
        file_info: &str,
    ) -> Result<(), String> {
        let path = format!(
            "{}/{}/messages",
            self.media_path_prefix(params.message_type)
                .ok_or_else(|| format!("unsupported QQ media type {}", params.message_type))?,
            target_openid
        );
        let url = format!("{}{}", self.api_base, path);
        let mut body = serde_json::json!({
            "msg_type": 7,
            "media": { "file_info": file_info },
            "msg_seq": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .to_string(),
        });
        if let Some(name) = params.file_name.filter(|name| !name.is_empty()) {
            body["content"] = serde_json::json!(name);
        }
        if let Some(mid) = params.msg_id.filter(|mid| !mid.is_empty()) {
            body["msg_id"] = serde_json::json!(mid);
        }

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("QQBot {}", params.token))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("send media message failed: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("send media message returned {status}: {text}"));
        }
        Ok(())
    }

    fn resolve_send_path(
        &self,
        message_type: &str,
        sender_id: &str,
        channel_id: Option<&str>,
        group_openid: Option<&str>,
        guild_id: Option<&str>,
    ) -> (String, bool) {
        match message_type {
            "dm" if guild_id.is_some_and(|g| !g.is_empty()) => {
                (format!("/dms/{}/messages", guild_id.unwrap()), false)
            }
            "group" if group_openid.is_some_and(|g| !g.is_empty()) => (
                format!("/v2/groups/{}/messages", group_openid.unwrap()),
                true,
            ),
            "guild" if channel_id.is_some_and(|c| !c.is_empty()) => {
                (format!("/channels/{}/messages", channel_id.unwrap()), false)
            }
            _ => (format!("/v2/users/{}/messages", sender_id), true),
        }
    }

    fn resolve_media_target<'a>(
        &self,
        message_type: &str,
        sender_id: &'a str,
        group_openid: Option<&'a str>,
    ) -> Option<&'a str> {
        match message_type {
            "c2c" if !sender_id.is_empty() => Some(sender_id),
            "group" => group_openid.filter(|openid| !openid.is_empty()),
            _ => None,
        }
    }

    fn media_path_prefix(&self, message_type: &str) -> Option<&'static str> {
        match message_type {
            "c2c" => Some("/v2/users"),
            "group" => Some("/v2/groups"),
            _ => None,
        }
    }
}

pub fn new_sender(sandbox: bool) -> Arc<QqSender> {
    Arc::new(QqSender::new(sandbox))
}
