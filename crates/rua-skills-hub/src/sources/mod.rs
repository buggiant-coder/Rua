pub mod aliyun;
pub mod clawhub;
pub mod github;
pub mod lobehub;
pub mod modelscope;
pub mod skillhub;
pub mod skills_sh;
pub mod skillsmp;

use std::sync::OnceLock;
use std::sync::RwLock;
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::client::HubClient;
use crate::types::{BundleSource, GithubSpec, HubError, RawBundlePayload};

#[async_trait]
pub trait ExtensionRouter: Send + Sync {
    async fn fetch(
        &self,
        client: &HubClient,
        source: &BundleSource,
        requested_version: &str,
        cancel: Option<&CancellationToken>,
    ) -> Result<Option<RawBundlePayload>, HubError>;
}

static EXTENSION_ROUTER: OnceLock<RwLock<Weak<dyn ExtensionRouter>>> = OnceLock::new();

fn router_slot() -> &'static RwLock<Weak<dyn ExtensionRouter>> {
    EXTENSION_ROUTER.get_or_init(|| RwLock::new(Weak::<NoopRouter>::new() as Weak<_>))
}

pub fn install_extension_router(router: Arc<dyn ExtensionRouter>) {
    let weak: Weak<dyn ExtensionRouter> = Arc::downgrade(&router);
    if let Ok(mut guard) = router_slot().write() {
        *guard = weak;
    }
}

fn current_extension_router() -> Option<Arc<dyn ExtensionRouter>> {
    router_slot().read().ok().and_then(|g| g.upgrade())
}

struct NoopRouter;

#[async_trait]
impl ExtensionRouter for NoopRouter {
    async fn fetch(
        &self,
        _client: &HubClient,
        _source: &BundleSource,
        _requested_version: &str,
        _cancel: Option<&CancellationToken>,
    ) -> Result<Option<RawBundlePayload>, HubError> {
        Ok(None)
    }
}

pub fn resolve_bundle_source(bundle_url: &str) -> Result<BundleSource, HubError> {
    let trimmed = bundle_url.trim();
    if trimmed.is_empty() {
        return Err(HubError::InvalidBundle("bundle URL is empty".into()));
    }
    let url =
        Url::parse(trimmed).map_err(|e| HubError::InvalidBundle(format!("invalid URL: {e}")))?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(HubError::InvalidBundle(format!(
            "URL scheme '{}' is not http/https",
            url.scheme()
        )));
    }
    let host = url.host_str().unwrap_or("").to_lowercase();

    if matches!(host.as_str(), "skills.sh" | "www.skills.sh") {
        return Ok(BundleSource::SkillsSh(trimmed.to_string()));
    }
    if matches!(host.as_str(), "github.com" | "www.github.com") {
        return Ok(BundleSource::GitHub(parse_github_spec(&url)?));
    }
    if matches!(
        host.as_str(),
        "lobehub.com" | "www.lobehub.com" | "market.lobehub.com"
    ) {
        return Ok(BundleSource::LobeHub(trimmed.to_string()));
    }
    if matches!(host.as_str(), "modelscope.cn" | "www.modelscope.cn") {
        return Ok(BundleSource::ModelScope(trimmed.to_string()));
    }
    if matches!(host.as_str(), "skillsmp.com" | "www.skillsmp.com") {
        return Ok(BundleSource::SkillsMp(trimmed.to_string()));
    }
    if matches!(
        host.as_str(),
        "api.skillhub.cn" | "skillhub.cn" | "www.skillhub.cn" | "skillhub.cloud.tencent.com"
    ) {
        return Ok(BundleSource::SkillHub(trimmed.to_string()));
    }
    if host == "api.aliyun.com" {
        let parts: Vec<&str> = url.path_segments().map(|s| s.collect()).unwrap_or_default();
        if parts.len() >= 3 && parts[0] == "agentexplorer" && parts[1] == "skills" {
            return Ok(BundleSource::Aliyun(trimmed.to_string()));
        }
    }
    if host.contains("clawhub.ai") {
        if let Some(slug) = clawhub::extract_slug_from_url(&url) {
            return Ok(BundleSource::ClawHub(slug));
        }
    }

    Ok(BundleSource::Direct(trimmed.to_string()))
}

pub async fn fetch_bundle(
    client: &HubClient,
    source: &BundleSource,
    requested_version: &str,
    cancel: Option<&CancellationToken>,
) -> Result<RawBundlePayload, HubError> {
    match source {
        BundleSource::ClawHub(slug) => {
            clawhub::fetch_bundle(client, slug, requested_version, cancel).await
        }
        BundleSource::GitHub(spec) => {
            github::fetch_bundle(client, spec, requested_version, cancel).await
        }
        BundleSource::Aliyun(url) => {
            aliyun::fetch_bundle(client, url, requested_version, cancel).await
        }
        BundleSource::Direct(url) => fetch_direct(client, url, cancel).await,
        other => unimplemented_source(client, other, requested_version, cancel).await,
    }
}

async fn fetch_direct(
    client: &HubClient,
    url: &str,
    cancel: Option<&CancellationToken>,
) -> Result<RawBundlePayload, HubError> {
    let body = crate::client::http_get_text_with_retry(client, url, cancel).await?;
    let payload: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| HubError::InvalidBundle(format!("direct bundle not JSON: {e}")))?;
    Ok(RawBundlePayload {
        payload,
        source_url: url.to_string(),
    })
}

pub(crate) async fn unimplemented_source(
    client: &HubClient,
    source: &BundleSource,
    requested_version: &str,
    cancel: Option<&CancellationToken>,
) -> Result<RawBundlePayload, HubError> {
    if let Some(router) = current_extension_router() {
        if let Some(payload) = router
            .fetch(client, source, requested_version, cancel)
            .await?
        {
            return Ok(payload);
        }
    }
    Err(HubError::UnsupportedSource(format!(
        "{source:?} — no handler registered"
    )))
}

fn parse_github_spec(url: &Url) -> Result<GithubSpec, HubError> {
    let segments: Vec<String> = url
        .path_segments()
        .map(|iter| iter.filter(|s| !s.is_empty()).map(decode_segment).collect())
        .unwrap_or_default();
    if segments.len() < 2 {
        return Err(HubError::InvalidBundle(
            "GitHub URL must include /owner/repo".into(),
        ));
    }
    let owner = segments[0].clone();
    let repo = segments[1].clone();
    let mut branch = String::new();
    let mut path = String::new();
    if segments.len() >= 4 && matches!(segments[2].as_str(), "tree" | "blob") {
        branch = segments[3].clone();
        if segments.len() > 4 {
            path = segments[4..].join("/");
        }
    } else if segments.len() > 2 {
        path = segments[2..].join("/");
    }
    Ok(GithubSpec {
        owner,
        repo,
        branch,
        path,
    })
}

fn decode_segment(segment: &str) -> String {
    percent_decode(segment).unwrap_or_else(|| segment.to_string())
}

fn percent_decode(input: &str) -> Option<String> {
    let mut out = Vec::with_capacity(input.len());
    let bytes = input.as_bytes();
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
