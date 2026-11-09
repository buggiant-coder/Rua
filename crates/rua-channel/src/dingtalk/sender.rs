use std::time::Duration;

use reqwest::Client;

use crate::http_client::default_http_client;

const MAX_RETRIES: u32 = 3;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct DingTalkSender {
    client: Client,
}

impl DingTalkSender {
    pub fn new() -> Self {
        Self {
            client: default_http_client(),
        }
    }

    pub fn with_client(client: Client) -> Self {
        Self { client }
    }

    pub async fn send_to_webhook(
        &self,
        webhook_url: &str,
        body: serde_json::Value,
    ) -> rua_core::Result<()> {
        let mut last_error = None;

        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                let delay = Duration::from_secs(1 << (attempt - 1));
                tracing::debug!(
                    attempt,
                    delay_ms = delay.as_millis() as u64,
                    "retrying session_webhook POST"
                );
                tokio::time::sleep(delay).await;
            }

            match self
                .client
                .post(webhook_url)
                .json(&body)
                .timeout(REQUEST_TIMEOUT)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    // DingTalk reports expired webhooks with HTTP 200 and a
                    // nonzero errcode. Surface that failure for OpenAPI fallback.
                    let bytes =
                        resp.bytes()
                            .await
                            .map_err(|error| rua_core::RuaError::Runtime {
                                message: format!("session_webhook response: {error}"),
                            })?;
                    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                        let code = value["errcode"]
                            .as_i64()
                            .or_else(|| {
                                value["errcode"]
                                    .as_str()
                                    .and_then(|code| code.parse::<i64>().ok())
                            })
                            .unwrap_or(0);
                        if code != 0 {
                            return Err(rua_core::RuaError::Runtime {
                                message: format!(
                                    "session_webhook rejected message: errcode={code}, {}",
                                    value["errmsg"].as_str().unwrap_or("unknown error")
                                ),
                            });
                        }
                    }
                    tracing::debug!(attempt, "session_webhook POST succeeded");
                    return Ok(());
                }
                Ok(resp) => {
                    let status = resp.status();
                    let resp_body = resp.text().await.unwrap_or_default();
                    tracing::warn!(
                        attempt,
                        %status,
                        body = %resp_body,
                        "session_webhook POST non-success status"
                    );
                    last_error = Some(format!("HTTP {status}: {resp_body}"));
                }
                Err(e) => {
                    tracing::warn!(attempt, error = %e, "session_webhook POST failed");
                    last_error = Some(e.to_string());
                }
            }
        }

        Err(rua_core::RuaError::Runtime {
            message: format!(
                "session_webhook POST failed after {MAX_RETRIES} retries: {}",
                last_error.unwrap_or_default()
            ),
        })
    }

    pub fn build_text_body(text: &str) -> serde_json::Value {
        serde_json::json!({
            "msgtype": "text",
            "text": {
                "content": text
            }
        })
    }

    pub fn build_markdown_body(title: &str, text: &str) -> serde_json::Value {
        serde_json::json!({
            "msgtype": "markdown",
            "markdown": {
                "title": title,
                "text": text
            }
        })
    }

    pub fn build_markdown_image_body(title: &str, alt: &str, media_id: &str) -> serde_json::Value {
        let alt = if alt.trim().is_empty() {
            "image"
        } else {
            alt.trim()
        };
        Self::build_markdown_body(title, &format!("![{alt}]({media_id})"))
    }

    pub fn build_file_body(media_id: &str) -> serde_json::Value {
        serde_json::json!({
            "msgtype": "file",
            "file": {
                "media_id": media_id
            }
        })
    }

    pub fn build_video_body(
        video_media_id: &str,
        video_type: &str,
        duration: u64,
        pic_media_id: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "msgtype": "video",
            "video": {
                "videoMediaId": video_media_id,
                "videoType": video_type,
                "duration": duration,
                "picMediaId": pic_media_id
            }
        })
    }
}

impl Default for DingTalkSender {
    fn default() -> Self {
        Self::new()
    }
}
