use std::time::Duration;

use reqwest::Client;

use crate::http_client::build_http_client;

const MAX_RETRIES: u32 = 3;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const POST_CHUNK_SIZE: usize = 4000;

#[derive(Clone)]
pub struct MattermostSender {
    client: Client,
    base_url: String,
}

impl MattermostSender {
    pub fn new(base_url: &str, bot_token: &str) -> Self {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Ok(val) = reqwest::header::HeaderValue::from_str(&format!("Bearer {bot_token}")) {
            headers.insert(reqwest::header::AUTHORIZATION, val);
        }
        let client = build_http_client(
            Client::builder()
                .default_headers(headers)
                .timeout(Duration::from_secs(30)),
        );
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    #[cfg(test)]
    pub fn with_client(client: Client, base_url: &str) -> Self {
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    pub async fn post_message(
        &self,
        channel_id: &str,
        text: &str,
        root_id: &str,
    ) -> rua_core::Result<()> {
        for chunk in chunk_text(text) {
            self.post_single(channel_id, &chunk, root_id).await?;
        }
        Ok(())
    }

    pub async fn post_message_with_files(
        &self,
        channel_id: &str,
        text: &str,
        root_id: &str,
        file_ids: &[String],
    ) -> rua_core::Result<()> {
        self.post_single_with_files(channel_id, text, root_id, file_ids)
            .await
    }

    pub async fn upload_file(
        &self,
        channel_id: &str,
        path: &std::path::Path,
    ) -> rua_core::Result<String> {
        let url = format!("{}/api/v4/files", self.base_url);
        let file_name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("file")
            .to_string();
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|err| rua_core::RuaError::Runtime {
                message: format!("mattermost upload read {}: {err}", path.display()),
            })?;
        let form = reqwest::multipart::Form::new()
            .text("channel_id", channel_id.to_string())
            .part(
                "files",
                reqwest::multipart::Part::bytes(bytes).file_name(file_name),
            );
        let resp = self
            .client
            .post(&url)
            .query(&[("channel_id", channel_id)])
            .multipart(form)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|err| rua_core::RuaError::Runtime {
                message: format!("mattermost upload failed: {err}"),
            })?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(rua_core::RuaError::Runtime {
                message: format!("mattermost upload HTTP {status}: {body}"),
            });
        }
        let body: serde_json::Value =
            resp.json()
                .await
                .map_err(|err| rua_core::RuaError::Runtime {
                    message: format!("mattermost upload parse failed: {err}"),
                })?;
        body.get("file_infos")
            .and_then(|value| value.as_array())
            .and_then(|items| items.first())
            .and_then(|item| item.get("id"))
            .and_then(|value| value.as_str())
            .map(ToOwned::to_owned)
            .ok_or_else(|| rua_core::RuaError::Runtime {
                message: format!("mattermost upload missing file id: {body}"),
            })
    }

    async fn post_single(
        &self,
        channel_id: &str,
        text: &str,
        root_id: &str,
    ) -> rua_core::Result<()> {
        self.post_single_with_files(channel_id, text, root_id, &[])
            .await
    }

    async fn post_single_with_files(
        &self,
        channel_id: &str,
        text: &str,
        root_id: &str,
        file_ids: &[String],
    ) -> rua_core::Result<()> {
        let url = format!("{}/api/v4/posts", self.base_url);
        let mut payload = serde_json::json!({
            "channel_id": channel_id,
            "message": text,
        });
        if !root_id.is_empty() {
            payload["root_id"] = serde_json::Value::String(root_id.to_string());
        }
        if !file_ids.is_empty() {
            payload["file_ids"] = serde_json::json!(file_ids);
        }

        let mut last_error = None;
        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                let delay = Duration::from_secs(1 << (attempt - 1));
                tracing::debug!(
                    attempt,
                    delay_ms = delay.as_millis() as u64,
                    "retrying mattermost post"
                );
                tokio::time::sleep(delay).await;
            }
            match self
                .client
                .post(&url)
                .json(&payload)
                .timeout(REQUEST_TIMEOUT)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    tracing::debug!(attempt, "mattermost post succeeded");
                    return Ok(());
                }
                Ok(resp) => {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    tracing::warn!(attempt, %status, body = %body, "mattermost post non-success");
                    last_error = Some(format!("HTTP {status}: {body}"));
                }
                Err(e) => {
                    tracing::warn!(attempt, error = %e, "mattermost post failed");
                    last_error = Some(e.to_string());
                }
            }
        }
        Err(rua_core::RuaError::Runtime {
            message: format!(
                "mattermost post failed after {MAX_RETRIES} retries: {}",
                last_error.unwrap_or_default()
            ),
        })
    }

    pub async fn get_bot_info(&self) -> rua_core::Result<(String, String)> {
        let url = format!("{}/api/v4/users/me", self.base_url);
        let resp = self
            .client
            .get(&url)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|e| rua_core::RuaError::Runtime {
                message: format!("mattermost /users/me failed: {e}"),
            })?;
        if !resp.status().is_success() {
            return Err(rua_core::RuaError::Runtime {
                message: format!("mattermost /users/me returned {}", resp.status()),
            });
        }
        let data: serde_json::Value =
            resp.json().await.map_err(|e| rua_core::RuaError::Runtime {
                message: format!("mattermost /users/me parse failed: {e}"),
            })?;
        let bot_id = data["id"].as_str().unwrap_or("").to_string();
        let bot_username = data["username"].as_str().unwrap_or("").to_string();
        Ok((bot_id, bot_username))
    }

    pub async fn fetch_thread_context(
        &self,
        root_id: &str,
        limit: usize,
    ) -> rua_core::Result<Vec<serde_json::Value>> {
        if root_id.trim().is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let url = format!("{}/api/v4/posts/{root_id}/thread", self.base_url);
        let resp = self
            .client
            .get(&url)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|e| rua_core::RuaError::Runtime {
                message: format!("mattermost thread fetch failed: {e}"),
            })?;
        if !resp.status().is_success() {
            return Err(rua_core::RuaError::Runtime {
                message: format!("mattermost thread fetch returned {}", resp.status()),
            });
        }
        let data: serde_json::Value =
            resp.json().await.map_err(|e| rua_core::RuaError::Runtime {
                message: format!("mattermost thread fetch parse failed: {e}"),
            })?;
        let posts = data.get("posts").and_then(|value| value.as_object());
        let order = data
            .get("order")
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
            .filter_map(|value| value.as_str());
        let mut context = Vec::new();
        for post_id in order {
            if post_id == root_id {
                continue;
            }
            let Some(post) = posts.and_then(|posts| posts.get(post_id)) else {
                continue;
            };
            context.push(serde_json::json!({
                "id": post.get("id").and_then(|value| value.as_str()).unwrap_or(post_id),
                "user_id": post.get("user_id").and_then(|value| value.as_str()).unwrap_or(""),
                "message": post.get("message").and_then(|value| value.as_str()).unwrap_or(""),
                "create_at": post.get("create_at").cloned().unwrap_or(serde_json::Value::Null),
            }));
            if context.len() >= limit {
                break;
            }
        }
        Ok(context)
    }

    pub async fn post_typing(
        &self,
        bot_id: &str,
        channel_id: &str,
        root_id: &str,
    ) -> rua_core::Result<()> {
        let url = format!("{}/api/v4/users/{bot_id}/typing", self.base_url);
        let mut payload = serde_json::json!({
            "channel_id": channel_id,
        });
        if !root_id.is_empty() {
            payload["parent_id"] = serde_json::Value::String(root_id.to_string());
        }
        let resp = self
            .client
            .post(&url)
            .json(&payload)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|e| rua_core::RuaError::Runtime {
                message: format!("mattermost typing failed: {e}"),
            })?;
        if !resp.status().is_success() {
            return Err(rua_core::RuaError::Runtime {
                message: format!("mattermost typing returned {}", resp.status()),
            });
        }
        Ok(())
    }
}

fn chunk_text(text: &str) -> Vec<String> {
    if text.is_empty() {
        return vec![];
    }
    if text.len() <= POST_CHUNK_SIZE {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        if rest.len() <= POST_CHUNK_SIZE {
            chunks.push(rest.to_string());
            break;
        }
        let chunk_end = char_boundary_at_or_before(rest, POST_CHUNK_SIZE);
        let chunk = &rest[..chunk_end];
        let split_at = chunk
            .rfind('\n')
            .filter(|&p| p > POST_CHUNK_SIZE / 2)
            .or_else(|| chunk.rfind(' ').filter(|&p| p > POST_CHUNK_SIZE / 2))
            .unwrap_or(chunk_end);
        chunks.push(rest[..split_at].to_string());
        rest = rest[split_at..].trim_start_matches(['\n', ' ']);
    }
    chunks
}

fn char_boundary_at_or_before(text: &str, max: usize) -> usize {
    if text.len() <= max {
        return text.len();
    }
    if text.is_char_boundary(max) {
        return max;
    }
    let mut idx = max - 1;
    while idx > 0 && !text.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}
