use crate::core::content::{AgentChannelRequest, IncomingContentPart};
use crate::dingtalk::mention;
use crate::dingtalk::webhook::DingTalkWebhookPayload;
use rua_core::Result;

fn build_request(
    payload: &DingTalkWebhookPayload,
    input: Vec<IncomingContentPart>,
) -> AgentChannelRequest {
    AgentChannelRequest {
        agent_id: String::new(),
        user_id: payload.sender_id.clone(),
        session_id: payload.conversation_id.clone(),
        input,
        channel_id: "dingtalk".into(),
        conversation_id: payload.conversation_id.clone(),
        message_id: if payload.msg_id.is_empty() {
            None
        } else {
            Some(payload.msg_id.clone())
        },
        metadata: serde_json::json!({
            "session_webhook": payload.session_webhook,
            "sender_nick": payload.sender_nick,
            "sender_staff_id": payload.sender_staff_id,
            "user_name": payload.sender_nick,
            "msg_type": payload.msg_type,
            "conversation_type": payload.conversation_type,
            "robot_code": payload.robot_code,
        }),
    }
}

fn input_text(input: &[IncomingContentPart]) -> String {
    input
        .iter()
        .filter_map(|part| match part {
            IncomingContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn extract_str<'a>(value: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(serde_json::Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn filename_hint(value: &serde_json::Value) -> Option<&str> {
    extract_str(
        value,
        &["fileName", "file_name", "filename", "name", "title"],
    )
}

fn map_media_type(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "picture" | "image" => "image",
        "voice" | "audio" => "audio",
        "video" => "video",
        "file" => "file",
        _ => "file",
    }
}

fn media_placeholder(kind: &str, download_code: &str, hint: Option<&str>) -> IncomingContentPart {
    let label = if kind == "file" {
        hint.unwrap_or(download_code)
    } else {
        download_code
    };
    IncomingContentPart::Text {
        text: format!("[{kind}: {label}]"),
    }
}

fn local_media_part(
    kind: &str,
    download_code: &str,
    value: &serde_json::Value,
) -> Option<IncomingContentPart> {
    let path = extract_str(value, &["local_path", "path"]).map(std::path::PathBuf::from)?;
    let mime = extract_str(value, &["mime", "mime_type", "content_type"]).map(ToOwned::to_owned);
    let name = filename_hint(value).map(ToOwned::to_owned);
    match kind {
        "image" => Some(IncomingContentPart::Image {
            file_id: download_code.to_string(),
            path,
            mime,
        }),
        "audio" => Some(IncomingContentPart::Audio {
            file_id: download_code.to_string(),
            path,
            mime,
            name,
        }),
        "video" => Some(IncomingContentPart::Video {
            file_id: download_code.to_string(),
            path,
            mime,
            name,
        }),
        _ => Some(IncomingContentPart::File {
            file_id: download_code.to_string(),
            path,
            mime,
            name,
        }),
    }
}

fn rich_content_parts(content: &serde_json::Value, msg_type: &str) -> Vec<IncomingContentPart> {
    let mut parts = Vec::new();

    let rich_list = content
        .get("richText")
        .or_else(|| content.get("rich_text"))
        .and_then(serde_json::Value::as_array);

    if let Some(items) = rich_list {
        for item in items {
            if !item.is_object() {
                continue;
            }

            if let Some(text) = extract_str(item, &["text", "content"]) {
                parts.push(IncomingContentPart::Text {
                    text: text.to_string(),
                });
            }

            let Some(download_code) = extract_str(
                item,
                &[
                    "downloadCode",
                    "download_code",
                    "pictureDownloadCode",
                    "picture_download_code",
                ],
            ) else {
                continue;
            };

            let kind = map_media_type(extract_str(item, &["type", "msgType"]).unwrap_or("file"));
            parts.push(
                local_media_part(kind, download_code, item)
                    .unwrap_or_else(|| media_placeholder(kind, download_code, filename_hint(item))),
            );
        }
    }

    if parts.is_empty() {
        if map_media_type(msg_type) == "audio" {
            if let Some(recognition) = extract_str(content, &["recognition"]) {
                return vec![IncomingContentPart::Text {
                    text: recognition.to_string(),
                }];
            }
        }

        if let Some(download_code) = extract_str(content, &["downloadCode", "download_code"]) {
            let kind = map_media_type(msg_type);
            parts.push(
                local_media_part(kind, download_code, content).unwrap_or_else(|| {
                    media_placeholder(kind, download_code, filename_hint(content))
                }),
            );
        }
    }

    parts
}

fn rich_payload_has_text(content: &serde_json::Value) -> bool {
    content
        .get("richText")
        .or_else(|| content.get("rich_text"))
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .any(|item| item.is_object() && extract_str(item, &["text", "content"]).is_some())
        })
        .unwrap_or(false)
}

fn quoted_rich_text_parts(content: &serde_json::Value) -> Vec<IncomingContentPart> {
    content
        .get("richText")
        .or_else(|| content.get("rich_text"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            if !item.is_object() {
                return None;
            }

            let item_type = extract_str(item, &["msgType", "type"]).unwrap_or("");
            if item_type == "text" {
                return extract_str(item, &["content", "text"]).map(|text| {
                    IncomingContentPart::Text {
                        text: format!("[quoted message: {text}]"),
                    }
                });
            }

            let download_code = extract_str(
                item,
                &[
                    "downloadCode",
                    "download_code",
                    "pictureDownloadCode",
                    "picture_download_code",
                ],
            )?;
            let kind = map_media_type(item_type);
            Some(
                local_media_part(kind, download_code, item)
                    .unwrap_or_else(|| media_placeholder(kind, download_code, filename_hint(item))),
            )
        })
        .collect()
}

fn quoted_parts(payload: &DingTalkWebhookPayload) -> Vec<IncomingContentPart> {
    let Some(text) = payload.text.as_ref() else {
        return Vec::new();
    };
    if !text
        .extra
        .get("isReplyMsg")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return Vec::new();
    }

    let Some(replied_msg) = text
        .extra
        .get("repliedMsg")
        .filter(|value| value.is_object())
    else {
        return Vec::new();
    };

    let msg_type = extract_str(replied_msg, &["msgType", "type"]).unwrap_or("");
    let Some(content) = replied_msg.get("content") else {
        return vec![IncomingContentPart::Text {
            text: "[quoted bot message: content unavailable]".into(),
        }];
    };

    match msg_type {
        "text" => {
            let quoted = content
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .or_else(|| extract_str(content, &["text", "content"]));
            quoted
                .map(|text| {
                    vec![IncomingContentPart::Text {
                        text: format!("[quoted message: {text}]"),
                    }]
                })
                .unwrap_or_default()
        }
        "richText" => {
            let parts = quoted_rich_text_parts(content);
            if parts.is_empty() {
                vec![IncomingContentPart::Text {
                    text: "[quoted richText message]".into(),
                }]
            } else {
                parts
            }
        }
        "picture" | "image" | "voice" | "audio" | "video" | "file" => {
            if map_media_type(msg_type) == "audio" {
                if let Some(recognition) = extract_str(content, &["recognition"]) {
                    return vec![IncomingContentPart::Text {
                        text: format!("[quoted voice message: {recognition}]"),
                    }];
                }
            }

            let Some(download_code) = extract_str(content, &["downloadCode", "download_code"])
            else {
                let kind = map_media_type(msg_type);
                return vec![IncomingContentPart::Text {
                    text: format!("[quoted {kind} message]"),
                }];
            };

            let kind = map_media_type(msg_type);
            vec![local_media_part(kind, download_code, content)
                .unwrap_or_else(|| media_placeholder(kind, download_code, filename_hint(content)))]
        }
        _ => Vec::new(),
    }
}

fn normalize_input(payload: &DingTalkWebhookPayload) -> Vec<IncomingContentPart> {
    let top_text = payload.text_content();
    let content = payload.extra.get("content");
    let mut rich_parts = content
        .map(|content| rich_content_parts(content, &payload.msg_type))
        .unwrap_or_default();
    let quoted = quoted_parts(payload);

    if rich_parts.is_empty() {
        let mut input = if top_text.is_empty() {
            Vec::new()
        } else {
            vec![IncomingContentPart::Text { text: top_text }]
        };
        if !quoted.is_empty() {
            let mut quoted_input = quoted;
            quoted_input.extend(input);
            input = quoted_input;
        }
        return input;
    }

    let rich_has_text = content.map(rich_payload_has_text).unwrap_or(false);
    let top_text_is_placeholder = matches!(top_text.as_str(), "\\n" | "//n");
    if !top_text.is_empty() && !rich_has_text && !top_text_is_placeholder {
        rich_parts.insert(0, IncomingContentPart::Text { text: top_text });
    }

    if !quoted.is_empty() {
        let mut quoted = quoted;
        quoted.extend(rich_parts);
        return quoted;
    }

    rich_parts
}

pub fn parse_native(native: serde_json::Value) -> Result<AgentChannelRequest> {
    let payload: DingTalkWebhookPayload =
        serde_json::from_value(native).map_err(|e| rua_core::RuaError::Validation {
            message: format!("invalid DingTalk webhook payload: {e}"),
        })?;
    Ok(build_request(&payload, normalize_input(&payload)))
}

pub fn parse_with_routing(
    native: serde_json::Value,
    bot_id: &str,
    bot_name: &str,
) -> Result<Option<AgentChannelRequest>> {
    let payload: DingTalkWebhookPayload =
        serde_json::from_value(native).map_err(|e| rua_core::RuaError::Validation {
            message: format!("invalid DingTalk webhook payload: {e}"),
        })?;

    let input = normalize_input(&payload);
    let text = input_text(&input);

    if !payload.is_group() {
        return Ok(Some(build_request(&payload, input)));
    }

    let at_users_hit = mention::bot_in_at_users(&payload, bot_id);
    let text_hit = !bot_name.is_empty() && text.contains(&format!("@{bot_name}"));

    if !at_users_hit && !text_hit {
        tracing::debug!(
            conversation_id = %payload.conversation_id,
            "group message without @bot mention, skipping"
        );
        return Ok(None);
    }

    let processed = mention::strip_at_prefix(&text);
    let routed_input = if processed.is_empty() {
        Vec::new()
    } else if processed == text {
        input
    } else {
        vec![IncomingContentPart::Text { text: processed }]
    };
    Ok(Some(build_request(&payload, routed_input)))
}
