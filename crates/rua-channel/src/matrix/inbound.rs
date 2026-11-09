//! Matrix event normalization and QwenPaw's bounded group history policy.
use std::collections::VecDeque;
use std::sync::Arc;

use matrix_sdk::media::{MediaFormat, MediaRequestParameters};
use matrix_sdk::ruma::events::room::MediaSource;
use matrix_sdk::{Client, Room, RoomMemberships};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::core::allowlist::ChannelAcl;
use crate::core::channel::AgentRequest;
use crate::core::config::BaseChannelConfig;
use crate::core::content::IncomingContentPart;
use rua_core::{Result, RuaError};

use super::channel::{
    atomic_write_file, matrix_download_filename, matrix_media_dir, matrix_request,
    MAX_INBOUND_MEDIA_SIZE, REQUEST_TIMEOUT,
};
use super::types::{MatrixConfig, MatrixOptions};

const MAX_HISTORY_ROOMS: usize = 256;
const HISTORY_MARKER: &str = "[Chat messages since your last reply - for context]";
const CURRENT_MARKER: &str = "[Current message - respond to this]";

struct HistoryEntry {
    text: String,
    media: Vec<IncomingContentPart>,
}

#[derive(Default)]
struct RoomHistories(VecDeque<(String, VecDeque<HistoryEntry>)>);

impl RoomHistories {
    fn record(&mut self, key: String, entry: HistoryEntry, limit: usize) {
        if limit == 0 {
            return;
        }
        let mut entries = self.take(&key);
        entries.push_back(entry);
        while entries.len() > limit {
            entries.pop_front();
        }
        self.0.push_back((key, entries));
        while self.0.len() > MAX_HISTORY_ROOMS {
            self.0.pop_front();
        }
    }

    fn take(&mut self, key: &str) -> VecDeque<HistoryEntry> {
        self.0
            .iter()
            .position(|(id, _)| id == key)
            .and_then(|i| self.0.remove(i))
            .map(|(_, entries)| entries)
            .unwrap_or_default()
    }
}

pub(super) struct MatrixInbound {
    config: MatrixConfig,
    options: MatrixOptions,
    acl: Value,
    // Serialize preparation so history cannot overtake the mention that consumes it.
    history: Mutex<RoomHistories>,
}

impl MatrixInbound {
    pub(super) fn new(config: MatrixConfig, options: MatrixOptions, acl: Value) -> Arc<Self> {
        Arc::new(Self {
            config,
            options,
            acl,
            history: Mutex::new(RoomHistories::default()),
        })
    }

    pub(super) async fn prepare(
        &self,
        client: &Client,
        room: &Room,
        mut native: Value,
    ) -> Result<Option<AgentRequest>> {
        let mut history = self.history.lock().await;
        let own_id = client
            .user_id()
            .map(ToString::to_string)
            .unwrap_or_default();
        let sender = native["sender"].as_str().unwrap_or_default().to_owned();
        let members = room
            .members(RoomMemberships::JOIN)
            .await
            .unwrap_or_default();
        let is_dm = room.is_direct().await.unwrap_or(false)
            || (members.len() == 2
                && members.iter().any(|m| m.user_id().as_str() == own_id)
                && members.iter().any(|m| m.user_id().as_str() == sender));
        native["is_dm"] = json!(is_dm);
        let mut metadata = self.acl.clone();
        if !is_dm && !self.options.group_allow_from.is_empty() {
            metadata["allow_from"] = json!(self.options.group_allow_from);
        }
        if metadata[if is_dm {
            "dm_disabled"
        } else {
            "group_disabled"
        }] == true
        {
            return Ok(None);
        }
        // Do not retain content from denied senders in the next allowed user's history.
        let gate: BaseChannelConfig = serde_json::from_value(metadata.clone()).unwrap_or_default();
        let allowed = ChannelAcl::from_config(&gate).is_allowed(&sender, !is_dm);

        let mut req = matrix_request(&native, &self.config, &own_id)?;
        if let Some(meta) = metadata.as_object() {
            req.metadata
                .as_object_mut()
                .expect("request metadata")
                .extend(meta.clone());
        }
        let body = native["body"].as_str().unwrap_or_default().to_owned();
        let bot_display = members
            .iter()
            .find(|m| m.user_id().as_str() == own_id)
            .and_then(|m| m.display_name());
        let mentioned = was_mentioned(&native, &own_id);
        req.metadata["mentioned"] = json!(mentioned);
        req.metadata["bot_mentioned"] = json!(mentioned);
        req.metadata["require_mention"] =
            json!(!is_dm && self.options.require_mention(room.room_id().as_str()));
        if !allowed {
            // The shared pipeline owns deny messages and access-control behavior.
            return Ok(Some(req));
        }

        let text = strip_mention_prefix(&body, &own_id, bot_display);
        if text == "NO_REPLY" {
            return Ok(None);
        }
        let is_media = matches!(
            native["msgtype"].as_str(),
            Some("m.image" | "m.audio" | "m.video" | "m.file")
        );
        if is_media {
            if native["msgtype"] == "m.image" && !self.options.vision_enabled {
                req.input = vec![IncomingContentPart::Text {
                    text: format!("[User sent an image (vision disabled): {body}]"),
                }];
            } else {
                match download_media(client, &self.config.media_dir, &mut native).await {
                    Ok(()) => req.input = matrix_request(&native, &self.config, &own_id)?.input,
                    Err(error) => {
                        tracing::warn!(%error, "matrix incoming media unavailable");
                        req.input = vec![IncomingContentPart::Text {
                            text: format!("[Media unavailable: {body}]"),
                        }];
                    }
                }
            }
        } else {
            let text = if text == "/reset" || text.starts_with("/reset ") {
                text.replacen("/reset", "/clear", 1)
            } else {
                text
            };
            req.input = vec![IncomingContentPart::Text { text }];
        }

        let sender_name = members
            .iter()
            .find(|m| m.user_id().as_str() == sender)
            .and_then(|m| m.display_name())
            .unwrap_or(&sender);
        req.metadata["user_name"] = json!(sender_name);
        req.metadata["acl_sender_id"] = json!(sender);
        // Keep history from distinct Matrix threads isolated.
        let history_key = req.session_id.clone();
        if !is_dm && self.options.require_mention(room.room_id().as_str()) && !mentioned {
            history.record(
                history_key,
                HistoryEntry {
                    text: format!(
                        "{sender_name}: {body} [id:{}]",
                        native["event_id"].as_str().unwrap_or_default()
                    ),
                    media: req
                        .input
                        .into_iter()
                        .filter(|part| !matches!(part, IncomingContentPart::Text { .. }))
                        .collect(),
                },
                self.options.history_limit,
            );
            return Ok(None);
        }

        if !is_dm {
            let entries = history.take(&history_key);
            let command = matches!(req.input.first(), Some(IncomingContentPart::Text { text }) if text.starts_with('/'));
            if !command {
                match req.input.first_mut() {
                    Some(IncomingContentPart::Text { text }) => {
                        *text = format!("{sender_name}: {text}")
                    }
                    _ => req.input.insert(
                        0,
                        IncomingContentPart::Text {
                            text: format!("{sender_name}:"),
                        },
                    ),
                }
                if !entries.is_empty() && self.options.history_limit > 0 {
                    let context = entries
                        .iter()
                        .map(|e| e.text.as_str())
                        .collect::<Vec<_>>()
                        .join("\n");
                    let mut input = vec![IncomingContentPart::Text {
                        text: format!("{HISTORY_MARKER}\n{context}\n\n{CURRENT_MARKER}"),
                    }];
                    for entry in entries {
                        input.extend(entry.media);
                    }
                    input.extend(req.input);
                    req.input = input;
                }
            }
        }
        Ok(Some(req))
    }
}

pub(super) fn was_mentioned(native: &Value, own_id: &str) -> bool {
    if own_id.is_empty() {
        return false;
    }
    let mentions = &native["m.mentions"];
    mentions["room"] == true
        || mentions["user_ids"]
            .as_array()
            .is_some_and(|ids| ids.iter().any(|id| id == own_id))
        || native["body"]
            .as_str()
            .unwrap_or_default()
            .to_lowercase()
            .contains(&own_id.to_lowercase())
        || native["formatted_body"].as_str().is_some_and(|html| {
            let encoded = super::channel::percent_encode_path_segment(own_id);
            [own_id, encoded.as_str()].iter().any(|id| {
                let link = format!("https://matrix.to/#/{id}");
                html.to_lowercase()
                    .contains(&format!("{link}\"").to_lowercase())
                    || html
                        .to_lowercase()
                        .contains(&format!("{link}'").to_lowercase())
            })
        })
}

fn strip_mention_prefix(body: &str, own_id: &str, display: Option<&str>) -> String {
    let local = own_id
        .trim_start_matches('@')
        .split(':')
        .next()
        .unwrap_or_default();
    for prefix in [own_id, display.unwrap_or_default(), local] {
        if prefix.is_empty() {
            continue;
        }
        if let Some(tail) = body.get(prefix.len()..) {
            if body[..prefix.len()].eq_ignore_ascii_case(prefix)
                && (tail.is_empty() || tail.starts_with([':', ' ', '\n', '\t', ',']))
            {
                return tail
                    .trim_start_matches([':', ' ', '\n', '\t', ','])
                    .to_string();
            }
        }
    }
    body.to_string()
}

async fn download_media(client: &Client, media_dir: &str, native: &mut Value) -> Result<()> {
    if native
        .pointer("/info/size")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        > MAX_INBOUND_MEDIA_SIZE
    {
        return Err(RuaError::Validation {
            message: "matrix media exceeds 50 MiB".into(),
        });
    }
    let source = if native["file"].is_object() {
        MediaSource::Encrypted(Box::new(
            serde_json::from_value(native["file"].clone()).map_err(|e| RuaError::Validation {
                message: format!("invalid encrypted Matrix media: {e}"),
            })?,
        ))
    } else {
        let url = native["url"]
            .as_str()
            .filter(|v| v.starts_with("mxc://"))
            .ok_or_else(|| RuaError::Validation {
                message: "matrix media URL missing".into(),
            })?;
        MediaSource::Plain(url.into())
    };
    let bytes = tokio::time::timeout(
        REQUEST_TIMEOUT,
        client.media().get_media_content(
            &MediaRequestParameters {
                source,
                format: MediaFormat::File,
            },
            false,
        ),
    )
    .await
    .map_err(|_| RuaError::Runtime {
        message: "matrix media download timed out".into(),
    })?
    .map_err(|e| RuaError::Runtime {
        message: format!("matrix media download failed: {e}"),
    })?;
    if bytes.len() as u64 > MAX_INBOUND_MEDIA_SIZE {
        return Err(RuaError::Validation {
            message: "matrix media exceeds 50 MiB".into(),
        });
    }
    let dir = matrix_media_dir(media_dir);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| RuaError::Io {
            message: e.to_string(),
        })?;
    let path = dir.join(format!(
        "{}_{}",
        uuid::Uuid::new_v4(),
        matrix_download_filename(native, "media")
    ));
    atomic_write_file(&path, &bytes).await?;
    native["local_path"] = json!(path.to_string_lossy());
    Ok(())
}
