use std::io::Cursor;

use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::client::{http_get_text_with_retry, HubClient};
use crate::types::{HubError, RawBundlePayload};

const SKILLHUB_DEFAULT_BASE: &str = "https://api.skillhub.cn";

fn skillhub_base() -> String {
    std::env::var("RUA_SKILLS_HUB_SKILLHUB_BASE").unwrap_or_else(|_| SKILLHUB_DEFAULT_BASE.into())
}

pub fn extract_skillhub_spec(url: &str) -> Option<(String, String)> {
    let parsed = Url::parse(url.trim()).ok()?;
    let host = parsed.host_str()?.to_lowercase();
    if !matches!(
        host.as_str(),
        "api.skillhub.cn" | "skillhub.cn" | "www.skillhub.cn" | "skillhub.cloud.tencent.com"
    ) {
        return None;
    }
    let parts: Vec<String> = parsed
        .path_segments()?
        .filter(|s| !s.is_empty())
        .map(|s| percent_decode(s).unwrap_or_else(|| s.to_string()))
        .collect();
    if parts.len() < 2 {
        return None;
    }
    let owner = parts[0].trim().to_string();
    let slug = parts[1].trim().to_string();
    if owner.is_empty() || slug.is_empty() {
        return None;
    }
    Some((owner, slug))
}

pub async fn fetch_bundle(
    client: &HubClient,
    bundle_url: &str,
    _requested_version: &str,
    cancel: Option<&CancellationToken>,
) -> Result<RawBundlePayload, HubError> {
    let (_owner, slug) = extract_skillhub_spec(bundle_url).ok_or_else(|| {
        HubError::InvalidBundle(
            "Invalid SkillHub URL format. Use URL like \
             https://api.skillhub.cn/{owner}/{slug}"
                .into(),
        )
    })?;

    let base = skillhub_base();

    let download_url = format!(
        "{}/api/v1/download?slug={}",
        base.trim_end_matches('/'),
        percent_encode(&slug),
    );

    let tmp = std::env::temp_dir().join(format!(
        "skillhub_dl_{}_{}.zip",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    ));
    let curl_result = tokio::process::Command::new("curl")
        .args(["-sL", "-o"])
        .arg(&tmp)
        .args(["--max-time", "30"])
        .arg(&download_url)
        .status()
        .await;
    match curl_result {
        Ok(status) if status.success() => {
            let read_result = tokio::fs::read(&tmp).await;
            let _ = tokio::fs::remove_file(&tmp).await;
            if let Ok(bytes) = read_result {
                if is_zip(&bytes) {
                    match zip_bytes_to_bundle(&bytes, &slug, bundle_url) {
                        Ok(payload) => return Ok(payload),
                        Err(e) => {
                            tracing::debug!(error = ?e, "SkillHub ZIP extraction failed, trying detail");
                        }
                    }
                } else if let Ok(text) = std::str::from_utf8(&bytes) {
                    if let Ok(value) = serde_json::from_str::<Value>(text) {
                        if let Some(payload) = try_extract_bundle(&value, &slug, bundle_url) {
                            return Ok(payload);
                        }
                    }
                }
            }
        }
        Ok(status) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            tracing::debug!(url = %download_url, exit_code = ?status.code(), "SkillHub curl download failed, trying detail");
        }
        Err(e) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            tracing::debug!(url = %download_url, error = ?e, "SkillHub curl command failed, trying detail");
        }
    }

    let detail_url = format!(
        "{}/api/v1/skills/{}",
        base.trim_end_matches('/'),
        percent_encode(&slug),
    );

    match http_get_text_with_retry(client, &detail_url, cancel).await {
        Ok(body) => {
            if let Ok(value) = serde_json::from_str::<Value>(&body) {
                if let Some(payload) = try_extract_bundle(&value, &slug, bundle_url) {
                    return Ok(payload);
                }
            }
        }
        Err(e) => {
            tracing::debug!(url = %detail_url, error = ?e, "SkillHub detail endpoint failed");
        }
    }

    Err(HubError::InvalidBundle(
        "SkillHub skill content not available. \
         Neither the download nor detail endpoint returned usable content."
            .into(),
    ))
}

fn try_extract_bundle(value: &Value, slug: &str, source_url: &str) -> Option<RawBundlePayload> {
    let obj = value.as_object()?;

    if let Some(Value::Object(files)) = obj.get("files") {
        if files.contains_key("SKILL.md") {
            return Some(RawBundlePayload {
                payload: value.clone(),
                source_url: source_url.to_string(),
            });
        }
    }

    let data = obj
        .get("data")
        .or(obj.get("Data"))
        .and_then(Value::as_object);

    let readme = data
        .and_then(|d| {
            d.get("content")
                .or(d.get("readme"))
                .or(d.get("ReadMeContent"))
                .or(d.get("skillMd"))
        })
        .or(obj.get("content"))
        .or(obj.get("readme"))
        .or(obj.get("skillMd"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let readme = readme?;

    let name = data
        .and_then(|d| d.get("name").or(d.get("Name")))
        .or(obj.get("name"))
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| slug.to_string());

    let mut files = Map::new();
    files.insert("SKILL.md".to_string(), Value::String(readme.to_string()));
    let mut payload = Map::new();
    payload.insert("name".to_string(), Value::String(name));
    payload.insert("files".to_string(), Value::Object(files));
    Some(RawBundlePayload {
        payload: Value::Object(payload),
        source_url: source_url.to_string(),
    })
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

fn is_zip(data: &[u8]) -> bool {
    data.len() >= 2 && data[0] == b'P' && data[1] == b'K'
}

fn zip_bytes_to_bundle(
    data: &[u8],
    slug: &str,
    source_url: &str,
) -> Result<RawBundlePayload, HubError> {
    let reader = Cursor::new(data);
    let mut archive = zip::ZipArchive::new(reader)
        .map_err(|e| HubError::InvalidBundle(format!("failed to read ZIP archive: {e}")))?;

    let mut files = Map::new();
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| HubError::InvalidBundle(format!("ZIP entry error: {e}")))?;

        if entry.is_dir() {
            continue;
        }

        let name = entry.name().to_string();
        if name.starts_with("__MACOSX") || name.starts_with('.') {
            continue;
        }

        let mut contents = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut contents)
            .map_err(|e| HubError::InvalidBundle(format!("ZIP read error for {name}: {e}")))?;

        if let Ok(text) = String::from_utf8(contents) {
            files.insert(name, Value::String(text));
        }
    }

    if !files.contains_key("SKILL.md") {
        return Err(HubError::InvalidBundle(
            "ZIP archive does not contain SKILL.md".into(),
        ));
    }

    let mut payload = Map::new();
    payload.insert("name".to_string(), Value::String(slug.to_string()));
    payload.insert("files".to_string(), Value::Object(files));

    Ok(RawBundlePayload {
        payload: Value::Object(payload),
        source_url: source_url.to_string(),
    })
}
