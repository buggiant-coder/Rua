use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::client::{http_get_text_with_retry, HubClient};
use crate::types::{HubError, RawBundlePayload};

fn aliyun_base() -> Option<String> {
    std::env::var("RUA_SKILLS_HUB_ALIYUN_BASE")
        .ok()
        .map(|s| s.trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
}

pub fn extract_aliyun_slug(url: &str) -> Option<String> {
    let parsed = Url::parse(url.trim()).ok()?;
    if parsed.host_str()?.to_lowercase() != "api.aliyun.com" {
        return None;
    }
    let parts: Vec<String> = parsed
        .path_segments()?
        .filter(|s| !s.is_empty())
        .map(|s| percent_decode(s).unwrap_or_else(|| s.to_string()))
        .collect();
    if parts.len() < 3 || parts[0] != "agentexplorer" || parts[1] != "skills" {
        return None;
    }
    let slug = parts[2].trim().to_string();
    if slug.is_empty() {
        None
    } else {
        Some(slug)
    }
}

pub async fn fetch_bundle(
    client: &HubClient,
    bundle_url: &str,
    _requested_version: &str,
    cancel: Option<&CancellationToken>,
) -> Result<RawBundlePayload, HubError> {
    let slug = extract_aliyun_slug(bundle_url).ok_or_else(|| {
        HubError::InvalidBundle(
            "Invalid Aliyun AgentExplorer URL format. Use URL like \
             https://api.aliyun.com/agentexplorer/skills/{skillName}"
                .into(),
        )
    })?;
    let detail_url = aliyun_base()
        .map(|base| format!("{base}/agentexplorer/skills/{}", percent_encode(&slug)))
        .unwrap_or_else(|| bundle_url.to_string());
    let body = http_get_text_with_retry(client, &detail_url, cancel).await?;
    let value: Value = serde_json::from_str(&body)
        .map_err(|e| HubError::InvalidBundle(format!("Aliyun detail not JSON: {e}")))?;
    extract_bundle(&value, &slug, bundle_url).ok_or_else(|| {
        HubError::InvalidBundle(
            "Aliyun skill detail did not contain SKILL.md, files, readme, content, or prompt"
                .into(),
        )
    })
}

fn extract_bundle(value: &Value, slug: &str, source_url: &str) -> Option<RawBundlePayload> {
    if value
        .get("files")
        .and_then(Value::as_object)
        .is_some_and(|files| files.contains_key("SKILL.md"))
    {
        return Some(RawBundlePayload {
            payload: value.clone(),
            source_url: source_url.to_string(),
        });
    }

    let obj = value.as_object()?;
    let data = obj
        .get("data")
        .or_else(|| obj.get("Data"))
        .and_then(Value::as_object);
    let source = data.unwrap_or(obj);
    if let Some(Value::Object(files)) = source.get("files") {
        if files.contains_key("SKILL.md") {
            let mut payload = Map::new();
            payload.insert("name".to_string(), Value::String(name_from(source, slug)));
            payload.insert("files".to_string(), Value::Object(files.clone()));
            return Some(RawBundlePayload {
                payload: Value::Object(payload),
                source_url: source_url.to_string(),
            });
        }
    }

    let content = first_str(
        source,
        &[
            "skillMd",
            "skillMD",
            "skillMarkdown",
            "readme",
            "ReadMeContent",
            "content",
            "prompt",
            "systemPrompt",
            "description",
        ],
    )?;
    let skill_md = if content.trim_start().starts_with("---") {
        content
    } else {
        format!(
            "---\nname: {}\ndescription: {}\n---\n\n{}",
            name_from(source, slug),
            first_str(source, &["description", "summary"])
                .unwrap_or_else(|| name_from(source, slug)),
            content
        )
    };
    let mut files = Map::new();
    files.insert("SKILL.md".to_string(), Value::String(skill_md));
    let mut payload = Map::new();
    payload.insert("name".to_string(), Value::String(name_from(source, slug)));
    payload.insert("files".to_string(), Value::Object(files));
    Some(RawBundlePayload {
        payload: Value::Object(payload),
        source_url: source_url.to_string(),
    })
}

fn name_from(obj: &Map<String, Value>, fallback: &str) -> String {
    first_str(obj, &["skillName", "name", "Name", "displayName"])
        .unwrap_or_else(|| fallback.to_string())
}

fn first_str(obj: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| obj.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16)?;
            let lo = (bytes[i + 2] as char).to_digit(16)?;
            out.push(((hi << 4) | lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}
