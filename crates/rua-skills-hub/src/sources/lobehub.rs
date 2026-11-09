use std::io::Read;

use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::client::{http_get_bytes_with_retry, HubClient};
use crate::types::{HubError, RawBundlePayload};

pub const LOBEHUB_MAX_ZIP_BYTES: u64 = 200 * 1024 * 1024;

pub const LOBEHUB_MAX_ZIP_ENTRIES: usize = 4096;

const LOBEHUB_DEFAULT_BASE: &str = "https://market.lobehub.com";

fn lobehub_base() -> String {
    std::env::var("RUA_SKILLS_HUB_LOBEHUB_BASE").unwrap_or_else(|_| LOBEHUB_DEFAULT_BASE.into())
}

pub fn lobehub_download_url(identifier: &str) -> String {
    format!(
        "{}/api/v1/skills/{}/download",
        lobehub_base().trim_end_matches('/'),
        identifier,
    )
}

pub fn extract_lobehub_identifier(url: &str) -> Option<String> {
    let parsed = Url::parse(url.trim()).ok()?;
    let host = parsed.host_str()?.to_lowercase();
    let parts: Vec<String> = parsed
        .path_segments()?
        .filter(|s| !s.is_empty())
        .map(|s| percent_decode(s).unwrap_or_else(|| s.to_string()))
        .collect();
    if parts.is_empty() {
        return None;
    }
    if matches!(host.as_str(), "lobehub.com" | "www.lobehub.com") {
        let idx = parts.iter().position(|p| p == "skills")?;
        let cand = parts.get(idx + 1)?.trim().to_string();
        if cand.is_empty() {
            return None;
        }
        return Some(cand);
    }
    if host == "market.lobehub.com"
        && parts.len() >= 5
        && parts[0] == "api"
        && parts[1] == "v1"
        && parts[2] == "skills"
        && parts[4] == "download"
    {
        let cand = parts[3].trim().to_string();
        if cand.is_empty() {
            return None;
        }
        return Some(cand);
    }
    None
}

pub fn should_keep_lobehub_file(parts: &[String]) -> bool {
    if parts.is_empty() {
        return false;
    }
    if parts.len() == 1 && parts[0] == "SKILL.md" {
        return true;
    }
    if matches!(parts[0].as_str(), "references" | "scripts") && parts.len() > 1 {
        return true;
    }
    parts.len() == 1
}

fn safe_zip_parts(name: &str) -> Option<Vec<String>> {
    let cleaned = name.replace('\\', "/");
    if cleaned.starts_with('/') {
        return None;
    }
    let parts: Vec<String> = cleaned
        .split('/')
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect();
    if parts.is_empty() {
        return None;
    }
    for p in &parts {
        if p == "." || p == ".." {
            return None;
        }
    }
    Some(parts)
}

pub fn lobehub_zip_to_bundle(identifier: &str, payload: &[u8]) -> Result<Value, HubError> {
    lobehub_zip_to_bundle_with_limits(
        identifier,
        payload,
        LOBEHUB_MAX_ZIP_ENTRIES,
        LOBEHUB_MAX_ZIP_BYTES,
    )
}

#[doc(hidden)]
pub fn lobehub_zip_to_bundle_with_limits(
    identifier: &str,
    payload: &[u8],
    max_entries: usize,
    max_bytes: u64,
) -> Result<Value, HubError> {
    let cursor = std::io::Cursor::new(payload);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| match e {
        zip::result::ZipError::InvalidArchive(msg)
        | zip::result::ZipError::UnsupportedArchive(msg)
            if msg.contains("size") || msg.contains("limit") =>
        {
            HubError::ZipLimit
        }
        zip::result::ZipError::Io(io_err) => {
            HubError::InvalidBundle(format!("LobeHub zip read: {io_err}"))
        }
        other => HubError::InvalidBundle(format!("LobeHub zip parse: {other}")),
    })?;

    let mut files: Map<String, Value> = Map::new();
    let mut entry_count: usize = 0;
    let mut total_bytes: u64 = 0;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| HubError::InvalidBundle(format!("LobeHub zip entry {i}: {e}")))?;
        if entry.is_dir() {
            continue;
        }
        entry_count += 1;
        if entry_count > max_entries {
            return Err(HubError::ZipLimit);
        }
        total_bytes = total_bytes.saturating_add(entry.size());
        if total_bytes > max_bytes {
            return Err(HubError::ZipLimit);
        }
        let raw_name = entry.name().to_string();
        let Some(parts) = safe_zip_parts(&raw_name) else {
            continue;
        };
        if !should_keep_lobehub_file(&parts) {
            continue;
        }
        let mut buf: Vec<u8> = Vec::with_capacity(entry.size() as usize);
        entry
            .read_to_end(&mut buf)
            .map_err(|e| HubError::InvalidBundle(format!("LobeHub zip read: {e}")))?;
        let rel = parts.join("/");
        files.insert(
            rel,
            Value::String(String::from_utf8_lossy(&buf).into_owned()),
        );
    }

    if !files.contains_key("SKILL.md") {
        return Err(HubError::InvalidBundle(
            "LobeHub skill package is missing SKILL.md".into(),
        ));
    }

    let mut out = Map::new();
    out.insert("name".to_string(), Value::String(identifier.to_string()));
    out.insert("files".to_string(), Value::Object(files));
    Ok(Value::Object(out))
}

pub async fn fetch_bundle(
    client: &HubClient,
    bundle_url: &str,
    requested_version: &str,
    cancel: Option<&CancellationToken>,
) -> Result<RawBundlePayload, HubError> {
    let identifier = extract_lobehub_identifier(bundle_url)
        .ok_or_else(|| HubError::InvalidBundle("Invalid LobeHub skill URL format".into()))?;
    let mut download_url = lobehub_download_url(&identifier);
    let trimmed_version = requested_version.trim();
    if !trimmed_version.is_empty() {
        download_url.push_str("?version=");
        download_url.push_str(&percent_encode_fragment(trimmed_version));
    }
    let bytes = http_get_bytes_with_retry(client, &download_url, cancel).await?;
    let payload = lobehub_zip_to_bundle(&identifier, &bytes)?;
    Ok(RawBundlePayload {
        payload,
        source_url: bundle_url.to_string(),
    })
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

fn percent_encode_fragment(input: &str) -> String {
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

pub async fn lobehub_handle(
    client: &HubClient,
    bundle_url: &str,
    requested_version: &str,
    cancel: Option<&CancellationToken>,
) -> Result<RawBundlePayload, HubError> {
    fetch_bundle(client, bundle_url, requested_version, cancel).await
}
