use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use crate::cancel::ensure_not_cancelled;
use crate::client::{github_cache_ttl, http_get_text_with_retry, HubClient};
use crate::types::{GithubSpec, HubError, RawBundlePayload};

const MAX_TREE_FILES: usize = 4096;

pub async fn fetch_bundle(
    client: &HubClient,
    spec: &GithubSpec,
    requested_version: &str,
    cancel: Option<&CancellationToken>,
) -> Result<RawBundlePayload, HubError> {
    let mut path_hint = spec.path.trim_matches('/').to_string();
    if path_hint.ends_with("/SKILL.md") {
        path_hint.truncate(path_hint.len() - "/SKILL.md".len());
    } else if path_hint == "SKILL.md" {
        path_hint.clear();
    }

    let preferred_branch = if !requested_version.trim().is_empty() {
        requested_version.trim().to_string()
    } else {
        spec.branch.trim().to_string()
    };

    let default_branch = github_default_branch(client, &spec.owner, &spec.repo, cancel)
        .await
        .unwrap_or_else(|_| "main".to_string());

    let branch_candidates = if !preferred_branch.is_empty() {
        vec![preferred_branch.clone()]
    } else {
        let mut out = vec![default_branch.clone()];
        for b in ["main", "master"] {
            if !out.contains(&b.to_string()) {
                out.push(b.to_string());
            }
        }
        out
    };

    let (selected_root, selected_branch, skill_md_entry) =
        find_skill_md(client, spec, &branch_candidates, &path_hint, cancel).await?;
    let skill_md_content = read_file_entry(client, &skill_md_entry, cancel).await?;
    let mut files: Map<String, Value> = Map::new();
    files.insert("SKILL.md".to_string(), Value::String(skill_md_content));
    let extras = collect_tree_files(client, spec, &selected_branch, &selected_root, cancel).await?;
    for (rel, body) in extras {
        files.entry(rel).or_insert(Value::String(body));
    }

    let skill_name = if !path_hint.is_empty() {
        path_hint
            .rsplit('/')
            .next()
            .unwrap_or(&spec.repo)
            .to_string()
    } else {
        spec.repo.clone()
    };

    let mut payload = Map::new();
    payload.insert("name".to_string(), Value::String(skill_name));
    payload.insert("files".to_string(), Value::Object(files));
    let source_url = format!("https://github.com/{}/{}", spec.owner, spec.repo);
    Ok(RawBundlePayload {
        payload: Value::Object(payload),
        source_url,
    })
}

async fn github_default_branch(
    client: &HubClient,
    owner: &str,
    repo: &str,
    cancel: Option<&CancellationToken>,
) -> Result<String, HubError> {
    let key = format!("default_branch:{owner}/{repo}");
    if let Some(cached) = client.github_cache().get("default_branch", &key) {
        return Ok(cached.as_str().unwrap_or("main").to_string());
    }
    ensure_not_cancelled(cancel)?;
    let url = github_api_url(owner, repo, "");
    let body = http_get_text_with_retry(client, &url, cancel).await?;
    let parsed: Value = serde_json::from_str(&body)
        .map_err(|e| HubError::InvalidBundle(format!("github repo meta not JSON: {e}")))?;
    let branch = parsed
        .get("default_branch")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("main")
        .to_string();
    client.github_cache().put(
        "default_branch",
        &key,
        Value::String(branch.clone()),
        github_cache_ttl(),
    );
    Ok(branch)
}

async fn find_skill_md(
    client: &HubClient,
    spec: &GithubSpec,
    branch_candidates: &[String],
    skill_hint: &str,
    cancel: Option<&CancellationToken>,
) -> Result<(String, String, Value), HubError> {
    let owner = spec.owner.as_str();
    let repo = spec.repo.as_str();
    let skill = skill_hint.trim().to_string();
    let mut last_err: Option<HubError> = None;

    for branch in branch_candidates {
        let mut roots: Vec<String> = Vec::new();
        if !skill.is_empty() {
            roots.push(format!("skills/{skill}"));
            roots.push(skill.clone());
        }
        roots.push(String::new());
        for root in &roots {
            let path = if root.is_empty() {
                "SKILL.md".to_string()
            } else {
                format!("{}/SKILL.md", root.trim_end_matches('/'))
            };
            match get_content_entry(client, owner, repo, &path, branch, cancel).await {
                Ok(entry) => {
                    if entry
                        .get("type")
                        .and_then(Value::as_str)
                        .map(|t| t == "file")
                        .unwrap_or(false)
                    {
                        return Ok((root.clone(), branch.clone(), entry));
                    }
                }
                Err(HubError::Http { status: 404, .. }) => continue,
                Err(err) => {
                    last_err = Some(err);
                }
            }
        }
    }

    Err(last_err.unwrap_or_else(|| {
        HubError::InvalidBundle(format!(
            "could not find SKILL.md in https://github.com/{}/{} (path hint: {:?}, tried branches: {:?})",
            spec.owner, spec.repo, skill_hint, branch_candidates,
        ))
    }))
}

async fn get_content_entry(
    client: &HubClient,
    owner: &str,
    repo: &str,
    path: &str,
    ref_name: &str,
    cancel: Option<&CancellationToken>,
) -> Result<Value, HubError> {
    let key = format!("content:{owner}/{repo}/{path}@{ref_name}");
    if let Some(cached) = client.github_cache().get("content", &key) {
        return Ok(cached);
    }
    ensure_not_cancelled(cancel)?;
    let encoded = encode_path(path);
    let suffix = if encoded.is_empty() {
        "contents".to_string()
    } else {
        format!("contents/{encoded}")
    };
    let url = format!(
        "{}?ref={}",
        github_api_url(owner, repo, &suffix),
        urlencode(ref_name),
    );
    let body = http_get_text_with_retry(client, &url, cancel).await?;
    let value: Value = serde_json::from_str(&body)
        .map_err(|e| HubError::InvalidBundle(format!("github contents not JSON: {e}")))?;
    if !value.is_object() {
        return Err(HubError::InvalidBundle(format!(
            "unexpected GitHub contents shape for {path}"
        )));
    }
    client
        .github_cache()
        .put("content", &key, value.clone(), github_cache_ttl());
    Ok(value)
}

async fn get_dir_entries(
    client: &HubClient,
    owner: &str,
    repo: &str,
    path: &str,
    ref_name: &str,
    cancel: Option<&CancellationToken>,
) -> Result<Vec<Value>, HubError> {
    let key = format!("dir:{owner}/{repo}/{path}@{ref_name}");
    if let Some(cached) = client.github_cache().get("dir", &key) {
        if let Some(arr) = cached.as_array() {
            return Ok(arr.clone());
        }
    }
    ensure_not_cancelled(cancel)?;
    let encoded = encode_path(path);
    let suffix = if encoded.is_empty() {
        "contents".to_string()
    } else {
        format!("contents/{encoded}")
    };
    let url = format!(
        "{}?ref={}",
        github_api_url(owner, repo, &suffix),
        urlencode(ref_name),
    );
    let body = http_get_text_with_retry(client, &url, cancel).await?;
    let value: Value = serde_json::from_str(&body)
        .map_err(|e| HubError::InvalidBundle(format!("github dir not JSON: {e}")))?;
    let arr = value.as_array().cloned().unwrap_or_default();
    client
        .github_cache()
        .put("dir", &key, Value::Array(arr.clone()), github_cache_ttl());
    Ok(arr)
}

async fn collect_tree_files(
    client: &HubClient,
    spec: &GithubSpec,
    branch: &str,
    root: &str,
    cancel: Option<&CancellationToken>,
) -> Result<Vec<(String, String)>, HubError> {
    let owner = spec.owner.as_str();
    let repo = spec.repo.as_str();
    let mut pending: Vec<String> = vec![root.to_string()];
    let mut out: Vec<(String, String)> = Vec::new();
    let mut visited: usize = 0;
    while let Some(current) = pending.pop() {
        ensure_not_cancelled(cancel)?;
        let entries = get_dir_entries(client, owner, repo, &current, branch, cancel).await?;
        for entry in entries {
            ensure_not_cancelled(cancel)?;
            let entry_type = entry
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let entry_path = entry
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if entry_path.is_empty() {
                continue;
            }
            if entry_type == "dir" {
                pending.push(entry_path);
                continue;
            }
            if entry_type != "file" {
                continue;
            }

            if relative_from_root(&entry_path, root) == "SKILL.md" {
                continue;
            }
            let body = read_file_entry(client, &entry, cancel).await?;
            out.push((relative_from_root(&entry_path, root), body));
            visited += 1;
            if visited >= MAX_TREE_FILES {
                tracing::warn!(
                    cap = MAX_TREE_FILES,
                    "github file collection capped — truncating extras",
                );
                return Ok(out);
            }
        }
    }
    Ok(out)
}

fn relative_from_root(full: &str, root: &str) -> String {
    if root.is_empty() {
        return full.trim_start_matches('/').to_string();
    }
    let prefix = format!("{}/", root.trim_end_matches('/'));
    if let Some(stripped) = full.strip_prefix(&prefix) {
        stripped.to_string()
    } else {
        full.to_string()
    }
}

async fn read_file_entry(
    client: &HubClient,
    entry: &Value,
    cancel: Option<&CancellationToken>,
) -> Result<String, HubError> {
    if let Some(download) = entry.get("download_url").and_then(Value::as_str) {
        if !download.is_empty() {
            return http_get_text_with_retry(client, download, cancel).await;
        }
    }
    if let Some(content) = entry.get("content").and_then(Value::as_str) {
        let normalized: String = content.chars().filter(|c| *c != '\n').collect();
        if !normalized.is_empty() {
            let bytes = STANDARD
                .decode(normalized.as_bytes())
                .map_err(|e| HubError::InvalidBundle(format!("github base64 decode: {e}")))?;
            return Ok(String::from_utf8_lossy(&bytes).into_owned());
        }
    }
    Err(HubError::InvalidBundle(
        "github entry has no usable content".into(),
    ))
}

fn github_api_url(owner: &str, repo: &str, suffix: &str) -> String {
    let base = format!("https://api.github.com/repos/{owner}/{repo}");
    let cleaned = suffix.trim_start_matches('/');
    if cleaned.is_empty() {
        base
    } else {
        format!("{base}/{cleaned}")
    }
}

fn encode_path(path: &str) -> String {
    let cleaned = path.trim_matches('/');
    if cleaned.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(cleaned.len());
    for byte in cleaned.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn urlencode(input: &str) -> String {
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
