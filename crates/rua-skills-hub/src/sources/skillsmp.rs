use serde_json::Value;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::cancel::ensure_not_cancelled;
use crate::client::{github_cache_ttl, http_get_text_with_retry, HubClient};
use crate::sources::github;
use crate::types::{GithubSpec, HubError, RawBundlePayload};

pub fn extract_skillsmp_slug(url: &str) -> Option<String> {
    let parsed = Url::parse(url.trim()).ok()?;
    let host = parsed.host_str()?.to_lowercase();
    if !matches!(host.as_str(), "skillsmp.com" | "www.skillsmp.com") {
        return None;
    }
    let parts: Vec<String> = parsed
        .path_segments()?
        .filter(|s| !s.is_empty())
        .map(|s| percent_decode(s).unwrap_or_else(|| s.to_string()))
        .collect();
    if parts.is_empty() {
        return None;
    }
    if let Some(idx) = parts.iter().position(|p| p == "skills") {
        if let Some(next) = parts.get(idx + 1) {
            let trimmed = next.trim().to_string();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
        return None;
    }
    let last = parts.last()?.trim().to_string();
    if last.is_empty() {
        None
    } else {
        Some(last)
    }
}

pub fn extract_skillsmp_spec_conservative(url: &str) -> Option<(String, String, String)> {
    let mut slug = extract_skillsmp_slug(url)?;
    if let Some(stripped) = slug.strip_suffix("-skill-md") {
        slug = stripped.to_string();
    }
    let tokens: Vec<&str> = slug.split('-').filter(|t| !t.is_empty()).collect();
    if tokens.len() < 3 {
        return None;
    }
    let owner = tokens[0].to_string();
    let repo = tokens[1].to_string();
    let skill_hint = tokens[2..].join("-");
    Some((owner, repo, skill_hint))
}

async fn extract_skillsmp_spec_probed(
    client: &HubClient,
    url: &str,
    cancel: Option<&CancellationToken>,
) -> Option<(String, String, String)> {
    let mut slug = extract_skillsmp_slug(url)?;
    if let Some(stripped) = slug.strip_suffix("-skill-md") {
        slug = stripped.to_string();
    }
    let tokens: Vec<&str> = slug.split('-').filter(|t| !t.is_empty()).collect();
    if tokens.len() < 3 {
        return None;
    }
    let owner = tokens[0].to_string();
    let tail = &tokens[1..];
    let max_split = tail.len().min(6);
    for i in (1..=max_split).rev() {
        let repo = tail[..i].join("-");
        if repo.is_empty() {
            continue;
        }
        if github_repo_exists(client, &owner, &repo, cancel).await {
            let skill_hint = tail[i..].join("-");
            return Some((owner, repo, skill_hint));
        }
    }
    None
}

async fn github_repo_exists(
    client: &HubClient,
    owner: &str,
    repo: &str,
    cancel: Option<&CancellationToken>,
) -> bool {
    if owner.is_empty() || repo.is_empty() {
        return false;
    }
    let key = format!("repo_exists:{owner}/{repo}");
    if let Some(cached) = client.github_cache().get("repo_exists", &key) {
        return cached.as_bool().unwrap_or(false);
    }
    if ensure_not_cancelled(cancel).is_err() {
        return false;
    }
    let url = format!("https://api.github.com/repos/{owner}/{repo}");
    let exists = match http_get_text_with_retry(client, &url, cancel).await {
        Ok(body) => match serde_json::from_str::<Value>(&body) {
            Ok(value) => value
                .get("full_name")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty()),
            Err(_) => false,
        },
        Err(_) => false,
    };
    client
        .github_cache()
        .put("repo_exists", &key, Value::Bool(exists), github_cache_ttl());
    exists
}

pub async fn fetch_bundle(
    client: &HubClient,
    bundle_url: &str,
    requested_version: &str,
    cancel: Option<&CancellationToken>,
) -> Result<RawBundlePayload, HubError> {
    let resolved = match extract_skillsmp_spec_probed(client, bundle_url, cancel).await {
        Some(triple) => triple,
        None => extract_skillsmp_spec_conservative(bundle_url)
            .ok_or_else(|| HubError::InvalidBundle("Invalid skillsmp URL format".into()))?,
    };
    let (owner, repo, skill_hint) = resolved;
    let spec = GithubSpec {
        owner,
        repo,
        branch: String::new(),
        path: skill_hint,
    };
    github::fetch_bundle(client, &spec, requested_version, cancel).await
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

pub async fn skillsmp_handle(
    client: &HubClient,
    bundle_url: &str,
    requested_version: &str,
    cancel: Option<&CancellationToken>,
) -> Result<RawBundlePayload, HubError> {
    fetch_bundle(client, bundle_url, requested_version, cancel).await
}
