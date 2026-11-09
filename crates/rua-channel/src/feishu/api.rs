use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

const TOKEN_URL: &str = "https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal";
const TOKEN_REFRESH_MARGIN: Duration = Duration::from_secs(300);
const MAX_RETRIES: u32 = 3;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

struct CachedToken {
    token: String,
    fetched_at: Instant,
    expires_in: Duration,
}

pub struct FeishuTokenManager {
    app_id: String,
    app_secret: String,
    client: reqwest::Client,
    cached: RwLock<Option<CachedToken>>,
    token_url: String,
}

#[derive(Debug, serde::Deserialize)]
struct TokenResponse {
    #[serde(default)]
    code: i32,
    #[serde(default)]
    tenant_access_token: String,
    #[serde(default)]
    expire: u64,
}

#[derive(Debug, serde::Deserialize)]
struct ImV1Response {
    #[serde(default)]
    code: i32,
    #[serde(default)]
    msg: String,
}

#[derive(Debug, serde::Deserialize)]
struct CardCreateResponse {
    #[serde(default)]
    code: i32,
    #[serde(default)]
    msg: String,
    #[serde(default)]
    data: Option<CardCreateData>,
}

#[derive(Debug, serde::Deserialize)]
struct CardCreateData {
    #[serde(default)]
    card_id: String,
}

#[derive(Debug, serde::Deserialize)]
struct CardkitResponse {
    #[serde(default)]
    code: i32,
    #[serde(default)]
    msg: String,
}

impl FeishuTokenManager {
    pub fn new(app_id: String, app_secret: String, client: reqwest::Client) -> Arc<Self> {
        Arc::new(Self {
            app_id,
            app_secret,
            client,
            cached: RwLock::new(None),
            token_url: TOKEN_URL.to_string(),
        })
    }

    #[doc(hidden)]
    pub fn new_with_token_url(
        app_id: String,
        app_secret: String,
        client: reqwest::Client,
        token_url: String,
    ) -> Arc<Self> {
        Arc::new(Self {
            app_id,
            app_secret,
            client,
            cached: RwLock::new(None),
            token_url,
        })
    }

    pub async fn get_token(&self) -> rua_core::Result<String> {
        {
            let guard = self.cached.read().await;
            if let Some(ref c) = *guard {
                let elapsed = c.fetched_at.elapsed();
                if elapsed + TOKEN_REFRESH_MARGIN < c.expires_in {
                    return Ok(c.token.clone());
                }
            }
        }
        self.refresh_token().await
    }

    async fn refresh_token(&self) -> rua_core::Result<String> {
        let body = serde_json::json!({
            "app_id": self.app_id,
            "app_secret": self.app_secret,
        });

        let resp = self
            .client
            .post(&self.token_url)
            .json(&body)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|e| rua_core::RuaError::Runtime {
                message: format!("feishu token request failed: {e}"),
            })?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(rua_core::RuaError::Runtime {
                message: format!("feishu token endpoint returned {status}: {text}"),
            });
        }

        let token_resp: TokenResponse =
            resp.json().await.map_err(|e| rua_core::RuaError::Runtime {
                message: format!("feishu token response parse failed: {e}"),
            })?;

        if token_resp.code != 0 {
            return Err(rua_core::RuaError::Runtime {
                message: format!("feishu token error code: {}", token_resp.code),
            });
        }
        if token_resp.tenant_access_token.is_empty() {
            return Err(rua_core::RuaError::Runtime {
                message: "feishu: empty tenant_access_token".into(),
            });
        }

        let cached = CachedToken {
            token: token_resp.tenant_access_token.clone(),
            fetched_at: Instant::now(),
            expires_in: Duration::from_secs(token_resp.expire),
        };
        *self.cached.write().await = Some(cached);

        Ok(token_resp.tenant_access_token)
    }
}

pub async fn send_im_v1(
    client: &reqwest::Client,
    token: &str,
    receive_id_type: &str,
    receive_id: &str,
    text: &str,
) -> rua_core::Result<()> {
    send_im_v1_with_base_url(
        client,
        "https://open.feishu.cn",
        token,
        receive_id_type,
        receive_id,
        "text",
        serde_json::json!({"text": text}).to_string(),
    )
    .await
}

pub async fn send_im_v1_with_base_url(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    receive_id_type: &str,
    receive_id: &str,
    msg_type: &str,
    content_json: String,
) -> rua_core::Result<()> {
    let body = serde_json::json!({
        "receive_id": receive_id,
        "msg_type": msg_type,
        "content": content_json,
    });

    let url = format!(
        "{}/open-apis/im/v1/messages?receive_id_type={receive_id_type}",
        base_url.trim_end_matches('/')
    );

    let mut last_error = None;
    for attempt in 0..=MAX_RETRIES {
        if attempt > 0 {
            let delay = Duration::from_secs(1 << (attempt - 1));
            tracing::debug!(attempt, "retrying feishu im/v1 POST");
            tokio::time::sleep(delay).await;
        }

        match client
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                let resp_body = resp.text().await.unwrap_or_default();
                if let Ok(im_resp) = serde_json::from_str::<ImV1Response>(&resp_body) {
                    if im_resp.code != 0 {
                        last_error = Some(format!("im/v1 code={}: {}", im_resp.code, im_resp.msg));
                        continue;
                    }
                }
                tracing::debug!(attempt, "feishu im/v1 POST succeeded");
                return Ok(());
            }
            Ok(resp) => {
                let status = resp.status();
                let resp_body = resp.text().await.unwrap_or_default();
                tracing::warn!(attempt, %status, body = %resp_body, "feishu im/v1 POST non-success");
                last_error = Some(format!("HTTP {status}: {resp_body}"));
            }
            Err(e) => {
                tracing::warn!(attempt, error = %e, "feishu im/v1 POST failed");
                last_error = Some(e.to_string());
            }
        }
    }

    Err(rua_core::RuaError::Runtime {
        message: format!(
            "feishu im/v1 POST failed after {MAX_RETRIES} retries: {}",
            last_error.unwrap_or_default()
        ),
    })
}

pub fn resolve_receive_id(metadata: &serde_json::Value) -> Option<(&'static str, String)> {
    let chat_type = metadata
        .get("feishu_chat_type")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if chat_type == "group" {
        let chat_id = metadata
            .get("feishu_chat_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !chat_id.is_empty() {
            return Some(("chat_id", chat_id.to_string()));
        }
    }

    let sender_id = metadata
        .get("feishu_sender_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !sender_id.is_empty() {
        return Some(("open_id", sender_id.to_string()));
    }

    None
}

pub async fn create_card_entity_with_base_url(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    card_json: serde_json::Value,
) -> rua_core::Result<String> {
    let body = serde_json::json!({
        "type": "card_json",
        "data": card_json.to_string(),
    });
    let url = format!(
        "{}/open-apis/cardkit/v1/cards",
        base_url.trim_end_matches('/')
    );
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json; charset=utf-8")
        .json(&body)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| rua_core::RuaError::Runtime {
            message: format!("feishu cardkit create failed: {e}"),
        })?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(rua_core::RuaError::Runtime {
            message: format!("feishu cardkit create returned {status}: {text}"),
        });
    }
    let body: CardCreateResponse = resp.json().await.map_err(|e| rua_core::RuaError::Runtime {
        message: format!("feishu cardkit create response parse failed: {e}"),
    })?;
    if body.code != 0 {
        return Err(rua_core::RuaError::Runtime {
            message: format!("feishu cardkit create code={}: {}", body.code, body.msg),
        });
    }
    let card_id = body
        .data
        .map(|data| data.card_id)
        .unwrap_or_default()
        .trim()
        .to_string();
    if card_id.is_empty() {
        return Err(rua_core::RuaError::Runtime {
            message: "feishu cardkit create missing card_id".into(),
        });
    }
    Ok(card_id)
}

pub async fn stream_card_element_content_with_base_url(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    card_id: &str,
    element_id: &str,
    content: &str,
    sequence: i32,
) -> rua_core::Result<()> {
    let body = serde_json::json!({
        "content": content,
        "uuid": uuid::Uuid::new_v4().to_string(),
        "sequence": sequence,
    });
    let url = format!(
        "{}/open-apis/cardkit/v1/cards/{}/elements/{}/content",
        base_url.trim_end_matches('/'),
        card_id,
        element_id
    );
    post_cardkit_json(
        client.put(url),
        token,
        body,
        "feishu cardkit stream content",
    )
    .await
}

pub async fn update_card_settings_with_base_url(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    card_id: &str,
    settings: serde_json::Value,
    sequence: i32,
) -> rua_core::Result<()> {
    let body = serde_json::json!({
        "settings": settings.to_string(),
        "uuid": uuid::Uuid::new_v4().to_string(),
        "sequence": sequence,
    });
    let url = format!(
        "{}/open-apis/cardkit/v1/cards/{}/settings",
        base_url.trim_end_matches('/'),
        card_id
    );
    post_cardkit_json(client.patch(url), token, body, "feishu cardkit settings").await
}

async fn post_cardkit_json(
    request: reqwest::RequestBuilder,
    token: &str,
    body: serde_json::Value,
    operation: &str,
) -> rua_core::Result<()> {
    let resp = request
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json; charset=utf-8")
        .json(&body)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| rua_core::RuaError::Runtime {
            message: format!("{operation} failed: {e}"),
        })?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(rua_core::RuaError::Runtime {
            message: format!("{operation} returned {status}: {text}"),
        });
    }
    let body: CardkitResponse = resp.json().await.map_err(|e| rua_core::RuaError::Runtime {
        message: format!("{operation} response parse failed: {e}"),
    })?;
    if body.code != 0 {
        return Err(rua_core::RuaError::Runtime {
            message: format!("{operation} code={}: {}", body.code, body.msg),
        });
    }
    Ok(())
}
