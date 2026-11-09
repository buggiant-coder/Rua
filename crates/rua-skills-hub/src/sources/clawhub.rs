use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::client::{http_get_text_with_retry, HubClient};
use crate::types::{HubError, RawBundlePayload};
use crate::{
    DEFAULT_HUB_BASE_URL, DEFAULT_HUB_DETAIL_PATH, DEFAULT_HUB_FILE_PATH, DEFAULT_HUB_VERSION_PATH,
};

pub fn extract_slug_from_url(url: &Url) -> Option<String> {
    let host = url.host_str()?.to_lowercase();
    if !host.contains("clawhub.ai") {
        return None;
    }
    let segments: Vec<&str> = url.path_segments()?.filter(|s| !s.is_empty()).collect();
    segments
        .last()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn hub_base_url() -> String {
    std::env::var("RUA_SKILLS_HUB_BASE_URL").unwrap_or_else(|_| DEFAULT_HUB_BASE_URL.into())
}

fn hub_detail_path() -> String {
    std::env::var("RUA_SKILLS_HUB_DETAIL_PATH").unwrap_or_else(|_| DEFAULT_HUB_DETAIL_PATH.into())
}

fn hub_version_path() -> String {
    std::env::var("RUA_SKILLS_HUB_VERSION_PATH").unwrap_or_else(|_| DEFAULT_HUB_VERSION_PATH.into())
}

fn hub_file_path() -> String {
    std::env::var("RUA_SKILLS_HUB_FILE_PATH").unwrap_or_else(|_| DEFAULT_HUB_FILE_PATH.into())
}

fn join_url(base: &str, suffix: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        suffix.trim_start_matches('/'),
    )
}

pub async fn fetch_bundle(
    client: &HubClient,
    slug: &str,
    requested_version: &str,
    cancel: Option<&CancellationToken>,
) -> Result<RawBundlePayload, HubError> {
    if slug.is_empty() {
        return Err(HubError::InvalidBundle(
            "ClawHub install requires a slug".into(),
        ));
    }
    let detail_url = join_url(&hub_base_url(), &hub_detail_path().replace("{slug}", slug));
    let body = http_get_text_with_retry(client, &detail_url, cancel).await?;
    let value: Value = serde_json::from_str(&body)
        .map_err(|e| HubError::InvalidBundle(format!("ClawHub detail not JSON: {e}")))?;
    let hydrated = hydrate_clawhub_payload(client, value, slug, requested_version, cancel).await?;
    Ok(RawBundlePayload {
        payload: hydrated,
        source_url: detail_url,
    })
}

async fn hydrate_clawhub_payload(
    client: &HubClient,
    data: Value,
    slug: &str,
    requested_version: &str,
    cancel: Option<&CancellationToken>,
) -> Result<Value, HubError> {
    if bundle_has_content(&data) {
        return Ok(data);
    }
    let Value::Object(root) = &data else {
        return Ok(data);
    };
    let Some(skill_obj) = root.get("skill").and_then(Value::as_object) else {
        return Ok(data);
    };

    let skill_slug = skill_obj
        .get("slug")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(slug)
        .to_string();
    if skill_slug.is_empty() {
        return Ok(data);
    }

    let version_obj = root.get("version").and_then(Value::as_object).cloned();
    let resolved_version = if let Some(obj) = version_obj.as_ref() {
        if obj.get("files").and_then(Value::as_array).is_some() {
            Some(Value::Object(obj.clone()))
        } else {
            None
        }
    } else {
        None
    };
    let version_payload = if let Some(v) = resolved_version {
        v
    } else {
        let version_hint = extract_version_hint(&data, requested_version);
        if version_hint.is_empty() {
            return Ok(data);
        }
        let url = join_url(
            &hub_base_url(),
            &hub_version_path()
                .replace("{slug}", &skill_slug)
                .replace("{version}", &version_hint),
        );
        let body = http_get_text_with_retry(client, &url, cancel).await?;
        serde_json::from_str(&body)
            .map_err(|e| HubError::InvalidBundle(format!("ClawHub version not JSON: {e}")))?
    };

    let version_obj = version_payload
        .get("version")
        .and_then(Value::as_object)
        .cloned()
        .or_else(|| version_payload.as_object().cloned())
        .ok_or_else(|| HubError::InvalidBundle("ClawHub version payload malformed".into()))?;

    let Some(files_meta) = version_obj.get("files").and_then(Value::as_array) else {
        return Ok(data);
    };

    let version_str = version_obj
        .get("version")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| requested_version.to_string());

    let file_url = join_url(
        &hub_base_url(),
        &hub_file_path().replace("{slug}", &skill_slug),
    );

    let mut files: Map<String, Value> = Map::new();
    let mut last_fetch_error: Option<HubError> = None;
    for item in files_meta {
        let Some(item_obj) = item.as_object() else {
            continue;
        };
        let Some(path) = item_obj.get("path").and_then(Value::as_str) else {
            continue;
        };
        if path.is_empty() {
            continue;
        }
        let mut url = format!("{file_url}?path={}", percent_encode(path));
        if !version_str.is_empty() {
            url.push_str("&version=");
            url.push_str(&percent_encode(&version_str));
        }
        match http_get_text_with_retry(client, &url, cancel).await {
            Ok(body) => {
                files.insert(path.to_string(), Value::String(body));
            }
            Err(err) => {
                tracing::warn!(path, error = ?err, "failed to fetch ClawHub file");
                last_fetch_error = Some(err);
            }
        }
    }

    if !files.contains_key("SKILL.md") {
        if let Some(err) = last_fetch_error {
            return Err(err);
        }
        return Ok(data);
    }

    let display_name = skill_obj
        .get("displayName")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| skill_slug.clone());

    let mut payload = Map::new();
    payload.insert("name".to_string(), Value::String(display_name));
    payload.insert("files".to_string(), Value::Object(files));
    Ok(Value::Object(payload))
}

fn bundle_has_content(data: &Value) -> bool {
    let Some(obj) = data.as_object() else {
        return false;
    };
    let direct = obj
        .get("content")
        .or_else(|| obj.get("skill_md"))
        .or_else(|| obj.get("skillMd"));
    if let Some(Value::String(s)) = direct {
        if !s.trim().is_empty() {
            return true;
        }
    }
    if let Some(Value::Object(files)) = obj.get("files") {
        if let Some(Value::String(s)) = files.get("SKILL.md") {
            if !s.is_empty() {
                return true;
            }
        }
    }
    false
}

fn extract_version_hint(detail: &Value, requested_version: &str) -> String {
    if !requested_version.is_empty() {
        return requested_version.to_string();
    }
    let Some(obj) = detail.as_object() else {
        return String::new();
    };
    if let Some(Value::Object(latest)) = obj.get("latestVersion") {
        if let Some(Value::String(v)) = latest.get("version") {
            if !v.is_empty() {
                return v.clone();
            }
        }
    }
    if let Some(Value::Object(skill)) = obj.get("skill") {
        if let Some(Value::Object(tags)) = skill.get("tags") {
            if let Some(Value::String(v)) = tags.get("latest") {
                if !v.is_empty() {
                    return v.clone();
                }
            }
        }
    }
    String::new()
}

fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(byte as char);
            }
            _ => {
                out.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    out
}
