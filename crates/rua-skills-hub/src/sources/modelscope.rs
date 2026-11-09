use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::client::{http_get_text_with_retry, HubClient};
use crate::sources::{clawhub, github};
use crate::types::{GithubSpec, HubError, RawBundlePayload};

const MODELSCOPE_DEFAULT_BASE: &str = "https://modelscope.cn";

fn modelscope_base() -> String {
    std::env::var("RUA_SKILLS_HUB_MODELSCOPE_BASE")
        .unwrap_or_else(|_| MODELSCOPE_DEFAULT_BASE.into())
}

pub fn extract_modelscope_skill_spec(url: &str) -> Option<(String, String, String)> {
    let parsed = Url::parse(url.trim()).ok()?;
    let host = parsed.host_str()?.to_lowercase();
    if !matches!(host.as_str(), "modelscope.cn" | "www.modelscope.cn") {
        return None;
    }
    let parts: Vec<String> = parsed
        .path_segments()?
        .filter(|s| !s.is_empty())
        .map(|s| percent_decode(s).unwrap_or_else(|| s.to_string()))
        .collect();
    if parts.len() < 3 || parts[0] != "skills" {
        return None;
    }
    let owner_part = parts[1].trim().to_string();
    let skill_name = parts[2].trim().to_string();
    if owner_part.is_empty() || skill_name.is_empty() {
        return None;
    }
    let owner = if let Some(stripped) = owner_part.strip_prefix('@') {
        stripped.trim().to_string()
    } else {
        owner_part
    };
    if owner.is_empty() {
        return None;
    }
    let mut version_hint = String::new();
    if parts.len() >= 6 && parts[3] == "archive" && parts[4] == "zip" {
        let mut archive = parts[5].trim().to_string();
        if archive.ends_with(".zip") {
            archive.truncate(archive.len() - ".zip".len());
        }
        version_hint = archive;
    }
    Some((owner, skill_name, version_hint))
}

pub async fn fetch_bundle(
    client: &HubClient,
    bundle_url: &str,
    requested_version: &str,
    cancel: Option<&CancellationToken>,
) -> Result<RawBundlePayload, HubError> {
    let (owner, skill_name, version_hint) =
        extract_modelscope_skill_spec(bundle_url).ok_or_else(|| {
            HubError::InvalidBundle(
                "Invalid ModelScope URL format. Use URL like \
                 https://modelscope.cn/skills/@owner/skill-name"
                    .into(),
            )
        })?;
    let detail_url = format!(
        "{}/api/v1/skills/@{}/{}",
        modelscope_base().trim_end_matches('/'),
        owner,
        skill_name,
    );
    let body = http_get_text_with_retry(client, &detail_url, cancel).await?;
    let detail: Value = serde_json::from_str(&body)
        .map_err(|e| HubError::InvalidBundle(format!("ModelScope detail not JSON: {e}")))?;

    let payload_obj = detail
        .get("Data")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    let source_url_str = payload_obj
        .get("SourceURL")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    let preferred_version = if !requested_version.trim().is_empty() {
        requested_version.trim().to_string()
    } else {
        version_hint
    };

    if !source_url_str.is_empty() && is_http_url(&source_url_str) {
        let lower = source_url_str.to_lowercase();
        if lower.contains("github.com") {
            let spec = parse_github_url_to_spec(&source_url_str)?;
            match github::fetch_bundle(client, &spec, &preferred_version, cancel).await {
                Ok(mut payload) => {
                    payload.source_url = bundle_url.to_string();
                    return Ok(payload);
                }
                Err(e) => {
                    tracing::debug!(
                        source_url = %source_url_str,
                        error = ?e,
                        "ModelScope GitHub SourceURL failed, trying ReadMeContent fallback",
                    );
                }
            }
        }
        if lower.contains("clawhub.ai") {
            let parsed = Url::parse(&source_url_str)
                .map_err(|e| HubError::InvalidBundle(format!("ModelScope SourceURL parse: {e}")))?;
            if let Some(slug) = clawhub::extract_slug_from_url(&parsed) {
                match clawhub::fetch_bundle(client, &slug, &preferred_version, cancel).await {
                    Ok(mut payload) => {
                        payload.source_url = bundle_url.to_string();
                        return Ok(payload);
                    }
                    Err(e) => tracing::debug!(
                        source_url = %source_url_str,
                        error = ?e,
                        "ModelScope ClawHub SourceURL failed, trying ReadMeContent fallback",
                    ),
                }
            }
        }
    }

    let readme = payload_obj
        .get("ReadMeContent")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if !readme.is_empty() {
        let fallback_name = payload_obj
            .get("Name")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| skill_name.clone());
        let mut files = Map::new();
        files.insert("SKILL.md".to_string(), Value::String(readme.to_string()));
        let mut payload = Map::new();
        payload.insert("name".to_string(), Value::String(fallback_name));
        payload.insert("files".to_string(), Value::Object(files));
        return Ok(RawBundlePayload {
            payload: Value::Object(payload),
            source_url: bundle_url.to_string(),
        });
    }

    Err(HubError::InvalidBundle(
        "ModelScope skill source is unsupported and ReadMeContent is empty. \
         Please import from the original source URL directly."
            .into(),
    ))
}

fn is_http_url(text: &str) -> bool {
    Url::parse(text.trim())
        .map(|u| {
            (u.scheme() == "http" || u.scheme() == "https")
                && u.host_str().is_some_and(|h| !h.is_empty())
        })
        .unwrap_or(false)
}

fn parse_github_url_to_spec(url_str: &str) -> Result<GithubSpec, HubError> {
    let parsed = Url::parse(url_str)
        .map_err(|e| HubError::InvalidBundle(format!("GitHub URL parse: {e}")))?;
    let parts: Vec<String> = parsed
        .path_segments()
        .map(|iter| {
            iter.filter(|s| !s.is_empty())
                .map(|s| percent_decode(s).unwrap_or_else(|| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    if parts.len() < 2 {
        return Err(HubError::InvalidBundle(
            "GitHub URL must include /owner/repo".into(),
        ));
    }
    let owner = parts[0].clone();
    let repo = parts[1].clone();
    let mut branch = String::new();
    let mut path = String::new();
    if parts.len() >= 4 && matches!(parts[2].as_str(), "tree" | "blob") {
        branch = parts[3].clone();
        if parts.len() > 4 {
            path = parts[4..].join("/");
        }
    } else if parts.len() > 2 {
        path = parts[2..].join("/");
    }
    Ok(GithubSpec {
        owner,
        repo,
        branch,
        path,
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

pub async fn modelscope_handle(
    client: &HubClient,
    bundle_url: &str,
    requested_version: &str,
    cancel: Option<&CancellationToken>,
) -> Result<RawBundlePayload, HubError> {
    fetch_bundle(client, bundle_url, requested_version, cancel).await
}
