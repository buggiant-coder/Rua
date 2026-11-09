use serde_json::Value;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::client::HubClient;
use crate::sources::github;
use crate::types::{GithubSpec, HubError, RawBundlePayload};

pub fn extract_skills_sh_spec(url: &str) -> Option<(String, String, String)> {
    let parsed = Url::parse(url.trim()).ok()?;
    let host = parsed.host_str()?.to_lowercase();
    if !matches!(host.as_str(), "skills.sh" | "www.skills.sh") {
        return None;
    }
    let parts: Vec<String> = parsed
        .path_segments()?
        .filter(|s| !s.is_empty())
        .map(|s| percent_decode(s).unwrap_or_else(|| s.to_string()))
        .collect();
    if parts.len() < 3 {
        return None;
    }
    let owner = parts[0].trim().to_string();
    let repo = parts[1].trim().to_string();
    let skill = parts[2].trim().to_string();
    if owner.is_empty() || repo.is_empty() || skill.is_empty() {
        return None;
    }
    Some((owner, repo, skill))
}

pub fn skills_sh_resolve(url: &str) -> Result<(GithubSpec, String), HubError> {
    let (owner, repo, skill) = extract_skills_sh_spec(url)
        .ok_or_else(|| HubError::InvalidBundle("Invalid skills.sh URL format".into()))?;
    Ok((
        GithubSpec {
            owner,
            repo,
            branch: String::new(),
            path: skill.clone(),
        },
        skill,
    ))
}

pub async fn fetch_bundle(
    client: &HubClient,
    bundle_url: &str,
    requested_version: &str,
    cancel: Option<&CancellationToken>,
) -> Result<RawBundlePayload, HubError> {
    let (spec, skill) = skills_sh_resolve(bundle_url)?;
    let mut payload = github::fetch_bundle(client, &spec, requested_version, cancel).await?;
    if let Value::Object(ref mut obj) = payload.payload {
        obj.insert("name".to_string(), Value::String(skill));
    }
    Ok(payload)
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
