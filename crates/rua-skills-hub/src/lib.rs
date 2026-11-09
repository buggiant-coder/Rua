pub mod bundle;
pub mod cancel;
pub mod client;
pub mod lib_ext;
pub mod sources;
pub mod types;

pub use bundle::{BundleInstaller, BundlePoolInstaller, NormalizedBundle};
pub use cancel::ensure_not_cancelled;
pub use client::{
    convert_github_rate_limit, github_cache_ttl, http_get_bytes_with_retry,
    http_get_text_with_retry, is_github_api, GithubCache, HubClient, RETRYABLE_HTTP_STATUS,
};
pub use sources::{install_extension_router, resolve_bundle_source, ExtensionRouter};
pub use types::{
    BundleSource, GithubSpec, HubError, HubInstallResult, HubSkillResult, RawBundlePayload,
};

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

pub const DEFAULT_HUB_BASE_URL: &str = "https://clawhub.ai";

pub const DEFAULT_HUB_SEARCH_PATH: &str = "/api/v1/search";

pub const DEFAULT_HUB_DETAIL_PATH: &str = "/api/v1/skills/{slug}";

pub const DEFAULT_HUB_VERSION_PATH: &str = "/api/v1/skills/{slug}/versions/{version}";

pub const DEFAULT_HUB_FILE_PATH: &str = "/api/v1/skills/{slug}/file";

pub async fn resolve_bundle(
    client: &HubClient,
    bundle_url: &str,
    requested_version: &str,
    cancel: Option<CancellationToken>,
) -> Result<(NormalizedBundle, String), HubError> {
    let source = resolve_bundle_source(bundle_url)?;
    let raw = sources::fetch_bundle(client, &source, requested_version, cancel.as_ref()).await?;
    let normalized = bundle::normalize_bundle(&raw.payload)?;
    Ok((normalized, raw.source_url))
}

#[allow(clippy::too_many_arguments)]
pub async fn install_skill_from_hub(
    client: &HubClient,
    installer: &dyn BundleInstaller,
    bundle_url: &str,
    requested_version: &str,
    enable: bool,
    target_name: Option<&str>,
    cancel: Option<CancellationToken>,
    install_meta: Option<rua_skills::InstallMeta>,
) -> Result<HubInstallResult, HubError> {
    let (normalized, source_url) =
        resolve_bundle(client, bundle_url, requested_version, cancel.clone()).await?;

    let install_meta = install_meta.map(|mut m| {
        m.content_hash = rua_skills::compute_content_hash(&normalized.content);
        if m.source_url.is_empty() {
            m.source_url = source_url.clone();
        }
        m
    });

    cancel::ensure_not_cancelled(cancel.as_ref())?;
    let final_name = pick_install_name(&normalized.name, bundle_url, target_name);

    let created = installer
        .create_skill(
            &final_name,
            &normalized.content,
            &normalized.references,
            &normalized.scripts,
            &normalized.extra_files,
            install_meta.as_ref(),
        )
        .await?;

    cancel::ensure_not_cancelled(cancel.as_ref())?;
    let mut enabled = false;
    if enable {
        enabled = installer.enable_skill(&created).await?;
        if !enabled {
            tracing::warn!(skill = %created, "skill imported but enable failed");
        }
    }

    Ok(HubInstallResult {
        name: created,
        enabled,
        source_url,
    })
}

#[allow(clippy::too_many_arguments)]
pub async fn install_skill_to_pool_from_hub(
    client: &HubClient,
    installer: &dyn BundlePoolInstaller,
    bundle_url: &str,
    requested_version: &str,
    target_name: Option<&str>,
    cancel: Option<CancellationToken>,
    install_meta: Option<rua_skills::InstallMeta>,
) -> Result<HubInstallResult, HubError> {
    let (normalized, source_url) =
        resolve_bundle(client, bundle_url, requested_version, cancel.clone()).await?;

    let install_meta = install_meta.map(|mut m| {
        m.content_hash = rua_skills::compute_content_hash(&normalized.content);
        if m.source_url.is_empty() {
            m.source_url = source_url.clone();
        }
        m
    });

    cancel::ensure_not_cancelled(cancel.as_ref())?;
    let final_name = pick_install_name(&normalized.name, bundle_url, target_name);

    let created = installer
        .create_pool_skill(
            &final_name,
            &normalized.content,
            &normalized.references,
            &normalized.scripts,
            &normalized.extra_files,
            install_meta.as_ref(),
        )
        .await?;

    Ok(HubInstallResult {
        name: created,
        enabled: false,
        source_url,
    })
}

fn pick_install_name(bundle_name: &str, bundle_url: &str, target_name: Option<&str>) -> String {
    if let Some(target) = target_name {
        let trimmed = target.trim();
        if !trimmed.is_empty() {
            return bundle::sanitize_skill_dir_name(trimmed);
        }
    }
    if !bundle_name.is_empty() {
        return bundle::sanitize_skill_dir_name(bundle_name);
    }
    let fallback = url::Url::parse(bundle_url)
        .ok()
        .and_then(|u| {
            u.path_segments()
                .and_then(|mut s| s.next_back().map(|s| s.to_string()))
        })
        .unwrap_or_default();
    bundle::safe_fallback_name(&fallback)
}

pub async fn search_hub_skills(
    client: &HubClient,
    query: &str,
    limit: u32,
) -> Result<Vec<HubSkillResult>, HubError> {
    let base = std::env::var("RUA_SKILLS_HUB_BASE_URL")
        .unwrap_or_else(|_| DEFAULT_HUB_BASE_URL.to_string());
    let path = std::env::var("RUA_SKILLS_HUB_SEARCH_PATH")
        .unwrap_or_else(|_| DEFAULT_HUB_SEARCH_PATH.to_string());
    let mut url = format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/'),
    );
    url.push_str("?q=");
    url.push_str(&urlencoding(query));
    url.push_str("&limit=");
    url.push_str(&limit.to_string());

    let body = http_get_text_with_retry(client, &url, None).await?;
    let value: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| HubError::InvalidBundle(e.to_string()))?;
    Ok(parse_search_results(&value))
}

fn urlencoding(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                out.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    out
}

fn parse_search_results(value: &serde_json::Value) -> Vec<HubSkillResult> {
    let items: Vec<&serde_json::Value> = match value {
        serde_json::Value::Array(arr) => arr.iter().collect(),
        serde_json::Value::Object(map) => {
            for key in ["items", "skills", "results", "data"] {
                if let Some(serde_json::Value::Array(arr)) = map.get(key) {
                    return arr.iter().filter_map(parse_one_search_item).collect();
                }
            }
            if map.contains_key("name") && map.contains_key("slug") {
                return parse_one_search_item(value).into_iter().collect();
            }
            return Vec::new();
        }
        _ => return Vec::new(),
    };
    items
        .into_iter()
        .filter_map(parse_one_search_item)
        .collect()
}

fn parse_one_search_item(value: &serde_json::Value) -> Option<HubSkillResult> {
    let obj = value.as_object()?;
    let slug = obj
        .get("slug")
        .and_then(|v| v.as_str())
        .or_else(|| obj.get("name").and_then(|v| v.as_str()))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())?;
    let name = obj
        .get("name")
        .and_then(|v| v.as_str())
        .or_else(|| obj.get("displayName").and_then(|v| v.as_str()))
        .map(|s| s.to_string())
        .unwrap_or_else(|| slug.clone());
    let description = obj
        .get("description")
        .and_then(|v| v.as_str())
        .or_else(|| obj.get("summary").and_then(|v| v.as_str()))
        .map(|s| s.to_string())
        .unwrap_or_default();
    let version = obj
        .get("version")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default();
    let source_url = obj
        .get("url")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default();
    let (author, icon_url) = if let Some(owner) = obj.get("owner").and_then(|v| v.as_object()) {
        let display = owner
            .get("displayName")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let handle = owner
            .get("handle")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let image = owner
            .get("image")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let a = if display.is_empty() { handle } else { display };
        let a = if a.is_empty() {
            obj.get("ownerHandle")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string()
        } else {
            a
        };
        (a, image)
    } else {
        let handle = obj
            .get("ownerHandle")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        (handle, String::new())
    };
    Some(HubSkillResult {
        slug,
        name,
        description,
        version,
        source_url,
        author,
        icon_url,
    })
}

pub type SharedHubClient = Arc<HubClient>;
