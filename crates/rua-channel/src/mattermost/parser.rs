use crate::core::content::{AgentChannelRequest, IncomingContentPart};

pub fn parse_native(native: serde_json::Value) -> rua_core::Result<AgentChannelRequest> {
    let data = native.get("data").unwrap_or(&native);

    let raw_post = data.get("post").and_then(|v| v.as_str()).unwrap_or("{}");
    let post: serde_json::Value =
        serde_json::from_str(raw_post).unwrap_or_else(|_| serde_json::json!({}));

    let channel_type = data
        .get("channel_type")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let user_id = post["user_id"].as_str().unwrap_or("").to_string();
    let mm_channel_id = post["channel_id"].as_str().unwrap_or("").to_string();
    let post_id = post["id"].as_str().unwrap_or("").to_string();
    let message = post["message"].as_str().unwrap_or("").to_string();
    let root_id = post["root_id"].as_str().unwrap_or("").to_string();

    let session_id = if channel_type == "D" {
        format!("mattermost_dm:{mm_channel_id}")
    } else {
        let effective_root = if root_id.is_empty() {
            &post_id
        } else {
            &root_id
        };
        format!("mattermost_thread:{effective_root}")
    };

    let target_root_id = if channel_type == "D" {
        root_id
    } else if root_id.is_empty() {
        post_id.clone()
    } else {
        root_id
    };

    let mut input = Vec::new();
    if !message.is_empty() {
        input.push(IncomingContentPart::Text { text: message });
    }
    input.extend(mattermost_attachment_placeholders(&post));

    let metadata = serde_json::json!({
        "mm_channel_id": mm_channel_id,
        "root_id": target_root_id,
        "channel_type": channel_type,
        "post_id": post_id,
    });

    Ok(AgentChannelRequest {
        agent_id: String::new(),
        user_id,
        session_id,
        input,
        channel_id: "mattermost".into(),
        conversation_id: mm_channel_id,
        message_id: Some(post_id),
        metadata,
    })
}

fn mattermost_attachment_placeholders(post: &serde_json::Value) -> Vec<IncomingContentPart> {
    let file_ids = post
        .get("file_ids")
        .and_then(|value| value.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let files = post
        .pointer("/metadata/files")
        .and_then(|value| value.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[]);

    file_ids
        .iter()
        .filter_map(|id| id.as_str().map(str::trim).filter(|id| !id.is_empty()))
        .map(|id| {
            let file = files
                .iter()
                .find(|file| file.get("id").and_then(|value| value.as_str()) == Some(id));
            if let Some(part) = file.and_then(|file| mattermost_local_media_part(id, file)) {
                return part;
            }
            let name = file
                .and_then(|file| file.get("name").and_then(|value| value.as_str()))
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or(id);
            IncomingContentPart::Text {
                text: format!("[file: {name}]"),
            }
        })
        .collect()
}

fn mattermost_local_media_part(
    file_id: &str,
    file: &serde_json::Value,
) -> Option<IncomingContentPart> {
    let path = file
        .get("local_path")
        .or_else(|| file.get("path"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)?;
    let mime = file
        .get("mime_type")
        .or_else(|| file.get("mime"))
        .or_else(|| file.get("content_type"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let name = file
        .get("name")
        .or_else(|| file.get("filename"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    if mime
        .as_deref()
        .is_some_and(|value| value.starts_with("image/"))
    {
        Some(IncomingContentPart::Image {
            file_id: file_id.to_string(),
            path,
            mime,
        })
    } else if mime
        .as_deref()
        .is_some_and(|value| value.starts_with("audio/"))
    {
        Some(IncomingContentPart::Audio {
            file_id: file_id.to_string(),
            path,
            mime,
            name,
        })
    } else if mime
        .as_deref()
        .is_some_and(|value| value.starts_with("video/"))
    {
        Some(IncomingContentPart::Video {
            file_id: file_id.to_string(),
            path,
            mime,
            name,
        })
    } else {
        Some(IncomingContentPart::File {
            file_id: file_id.to_string(),
            path,
            mime,
            name,
        })
    }
}

pub fn parse_with_routing(
    native: serde_json::Value,
    bot_id: &str,
    bot_username: &str,
) -> rua_core::Result<Option<AgentChannelRequest>> {
    parse_with_thread_policy(native, bot_id, bot_username, false)
}

pub fn parse_with_thread_policy(
    native: serde_json::Value,
    bot_id: &str,
    bot_username: &str,
    thread_followed: bool,
) -> rua_core::Result<Option<AgentChannelRequest>> {
    let data = native.get("data").unwrap_or(&native);
    let raw_post = data.get("post").and_then(|v| v.as_str()).unwrap_or("{}");
    let post: serde_json::Value =
        serde_json::from_str(raw_post).unwrap_or_else(|_| serde_json::json!({}));

    let sender_id = post["user_id"].as_str().unwrap_or("");
    if sender_id == bot_id {
        return Ok(None);
    }

    let channel_type = data
        .get("channel_type")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let message = post["message"].as_str().unwrap_or("");

    let is_dm = channel_type == "D";
    let bot_mention = format!("@{bot_username}");
    let is_mentioned =
        !bot_username.is_empty() && message.to_lowercase().contains(&bot_mention.to_lowercase());

    if !is_dm && !is_mentioned && !thread_followed {
        return Ok(None);
    }

    let mut req = parse_native(native)?;
    req.metadata["is_dm"] = serde_json::json!(is_dm);
    req.metadata["is_group"] = serde_json::json!(!is_dm);
    req.metadata["bot_mentioned"] = serde_json::json!(is_mentioned || thread_followed);
    req.metadata["mentioned"] = serde_json::json!(is_mentioned || thread_followed);

    if is_mentioned && !req.input.is_empty() {
        if let Some(IncomingContentPart::Text { text }) = req.input.first_mut() {
            *text = text
                .replace(&bot_mention, "")
                .replace(&bot_mention.to_lowercase(), "")
                .trim()
                .to_string();
        }
    }

    Ok(Some(req))
}
