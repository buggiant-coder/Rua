use std::net::IpAddr;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use url::Url;

use rua_contract::Tool;
use rua_core::tool::{ToolContext, ToolResult, ToolSchema};

use crate::annotations::{ApprovalPolicy, ExtendedToolAnnotations};
use crate::error::ToolError;
use crate::schema::ToolSpec;

const DEFAULT_TIMEOUT_SECS: u64 = 30;

const MAX_RESPONSE_BYTES: u64 = 5 * 1024 * 1024;

const MAX_REDIRECTS: usize = 5;

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
                          AppleWebKit/537.36 (KHTML, like Gecko) \
                          Chrome/133.0.0.0 Safari/537.36";
const ACCEPT_HEADER: &str = "text/html,application/xhtml+xml,application/xml;q=0.9,\
                             text/plain;q=0.8,application/json;q=0.8,*/*;q=0.7";
const ACCEPT_LANGUAGE_HEADER: &str = "en-US,en;q=0.9";
const SEC_CH_UA_HEADER: &str =
    "\"Not(A:Brand\";v=\"99\", \"Google Chrome\";v=\"133\", \"Chromium\";v=\"133\"";

const DEFAULT_JINA_READER_BASE_URL: &str = "https://r.jina.ai";

pub fn is_private_or_reserved(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || (v6.segments()[0] & 0xffc0) == 0xfe80
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || v6
                    .to_ipv4_mapped()
                    .is_some_and(|v4| is_private_or_reserved(&IpAddr::V4(v4)))
        }
    }
}

pub async fn validate_url_ssrf(url: &Url, allow_private: bool) -> Result<(), ToolError> {
    match url.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(ToolError::SsrfBlocked {
                address: format!("scheme '{scheme}' is not allowed"),
            });
        }
    }

    let host = url.host_str().unwrap_or("");

    if !allow_private {
        let lower = host.to_lowercase();
        if lower == "localhost"
            || lower == "127.0.0.1"
            || lower == "::1"
            || lower == "[::1]"
            || lower == "0.0.0.0"
        {
            return Err(ToolError::SsrfBlocked {
                address: host.to_string(),
            });
        }
    }

    if !allow_private {
        if let Ok(ip) = host.parse::<IpAddr>() {
            if is_private_or_reserved(&ip) {
                return Err(ToolError::SsrfBlocked {
                    address: ip.to_string(),
                });
            }
        } else {
            let port = url.port_or_known_default().unwrap_or(80);
            let lookup_addr = format!("{host}:{port}");
            let resolved: Vec<std::net::SocketAddr> =
                match tokio::net::lookup_host(&lookup_addr).await {
                    Ok(addrs) => addrs.collect(),
                    Err(e) => {
                        return Err(ToolError::SsrfBlocked {
                            address: format!("DNS resolution failed for {host}: {e}"),
                        });
                    }
                };
            for addr in &resolved {
                if is_private_or_reserved(&addr.ip()) {
                    return Err(ToolError::SsrfBlocked {
                        address: format!("{host} resolves to private address {}", addr.ip()),
                    });
                }
            }
        }
    }

    Ok(())
}

#[derive(Debug, Deserialize)]
struct WebFetchArgs {
    url: String,
    #[serde(default)]
    timeout_secs: Option<u64>,
    #[serde(default)]
    max_bytes: Option<u64>,
    #[serde(default)]
    allow_private: Option<bool>,
    #[serde(default)]
    reader: Option<String>,
}

struct FetchResponse {
    success: bool,
    content_type: String,
    body: Vec<u8>,
    truncated: bool,
    original_len: Option<u64>,
}

fn metadata_string(ctx: &ToolContext, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(value) = ctx.metadata.get(*key).and_then(|v| v.as_str()) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn env_string(keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Ok(value) = std::env::var(key) {
            let value = value.trim().to_string();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

fn metadata_or_env(ctx: &ToolContext, metadata_keys: &[&str], env_keys: &[&str]) -> Option<String> {
    metadata_string(ctx, metadata_keys).or_else(|| env_string(env_keys))
}

fn normalize_reader_mode(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace(['-', ' '], "_")
}

fn reader_mode(args: &WebFetchArgs, ctx: &ToolContext) -> String {
    args.reader
        .clone()
        .or_else(|| {
            metadata_or_env(
                ctx,
                &["web_fetch_reader", "reader", "reader_provider"],
                &["RUA_WEB_FETCH_READER"],
            )
        })
        .map(|v| normalize_reader_mode(&v))
        .unwrap_or_else(|| "auto".to_string())
}

fn jina_reader_base_url(ctx: &ToolContext) -> String {
    metadata_or_env(
        ctx,
        &["jina_reader_base_url", "jina_base_url"],
        &["RUA_JINA_READER_BASE_URL"],
    )
    .unwrap_or_else(|| DEFAULT_JINA_READER_BASE_URL.to_string())
}

fn jina_api_key(ctx: &ToolContext) -> Option<String> {
    metadata_or_env(
        ctx,
        &["jina_api_key", "jina_reader_api_key"],
        &["JINA_API_KEY", "RUA_JINA_API_KEY"],
    )
}

#[cfg(feature = "browser-tools")]
fn browser_cdp_url(ctx: &ToolContext) -> Option<String> {
    metadata_or_env(
        ctx,
        &["browser_cdp_url", "browser_debug_url", "chrome_cdp_url"],
        &["RUA_BROWSER_CDP_URL", "BROWSER_CDP_URL", "CHROME_CDP_URL"],
    )
}

fn can_use_external_reader(url: &Url, allow_private: bool) -> bool {
    !allow_private
        && matches!(url.scheme(), "http" | "https")
        && url.username().is_empty()
        && url.password().is_none()
}

fn jina_reader_url(base_url: &str, target_url: &Url) -> String {
    format!("{}/{}", base_url.trim_end_matches('/'), target_url.as_str())
}

fn browser_navigation_headers(request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    request
        .header("User-Agent", USER_AGENT)
        .header("Accept", ACCEPT_HEADER)
        .header("Accept-Language", ACCEPT_LANGUAGE_HEADER)
        .header("Cache-Control", "no-cache")
        .header("Pragma", "no-cache")
        .header("Upgrade-Insecure-Requests", "1")
        .header("Sec-Fetch-Site", "none")
        .header("Sec-Fetch-Mode", "navigate")
        .header("Sec-Fetch-User", "?1")
        .header("Sec-Fetch-Dest", "document")
        .header("Sec-CH-UA", SEC_CH_UA_HEADER)
        .header("Sec-CH-UA-Mobile", "?0")
        .header("Sec-CH-UA-Platform", "\"Windows\"")
}

pub struct WebFetchTool;

impl WebFetchTool {
    pub fn new() -> Self {
        Self
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "web_fetch".to_string(),
            description: "Fetch content from a URL. Supports HTTP/HTTPS. \
                          Returns text content or a reference for binary content."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "URL to fetch (HTTP or HTTPS)"
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "Timeout in seconds (default: 30)"
                    },
                    "max_bytes": {
                        "type": "integer",
                        "description": "Maximum response size in bytes (default: 5MB)"
                    },
                    "allow_private": {
                        "type": "boolean",
                        "description": "Allow fetching from private/localhost addresses (default: false)"
                    },
                    "reader": {
                        "type": "string",
                        "description": "Reader mode for public pages: auto, jina, native, or browser (default: auto)"
                    }
                },
                "required": ["url"]
            }),
            annotations: ExtendedToolAnnotations {
                read_only: true,
                requires_network: true,
                idempotent: true,
                requires_approval: ApprovalPolicy::ToolGuardDecides,
                allowed_on_mobile: true,
                ..Default::default()
            },
            namespace: "builtin".into(),
        }
    }
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn schema(&self) -> ToolSchema {
        let spec = Self::spec();
        ToolSchema {
            name: spec.name,
            description: spec.description,
            parameters: spec.parameters,
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
    ) -> rua_core::Result<ToolResult> {
        let args: WebFetchArgs =
            serde_json::from_value(args).map_err(|e| rua_core::RuaError::Tool {
                tool: "web_fetch".into(),
                message: e.to_string(),
            })?;

        let timeout_secs = args.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
        let max_bytes = args.max_bytes.unwrap_or(MAX_RESPONSE_BYTES);
        let allow_private = args.allow_private.unwrap_or(false);
        let reader = reader_mode(&args, &ctx);

        let url = Url::parse(&args.url).map_err(|e| rua_core::RuaError::Tool {
            tool: "web_fetch".into(),
            message: format!("invalid URL: {e}"),
        })?;

        validate_url_ssrf(&url, allow_private)
            .await
            .map_err(rua_core::RuaError::from)?;

        let response = if reader == "jina" || reader == "jina_reader" {
            if !can_use_external_reader(&url, allow_private) {
                return Err(rua_core::RuaError::Tool {
                    tool: "web_fetch".into(),
                    message:
                        "Jina reader is only used for public HTTP/HTTPS URLs without credentials"
                            .into(),
                });
            }
            fetch_with_jina_reader(&url, timeout_secs, max_bytes, &ctx).await?
        } else if matches!(
            reader.as_str(),
            "browser" | "desktop_browser" | "chrome" | "cdp"
        ) {
            fetch_with_browser_reader(&url, timeout_secs, max_bytes, allow_private, &ctx).await?
        } else if reader == "native" {
            fetch_native(&url, timeout_secs, max_bytes, allow_private).await?
        } else if can_use_external_reader(&url, allow_private) {
            match fetch_with_jina_reader(&url, timeout_secs, max_bytes, &ctx).await {
                Ok(response) => response,
                Err(e) => {
                    tracing::debug!(error = %e, url = %url, "jina reader failed; falling back to native fetch");
                    match fetch_native(&url, timeout_secs, max_bytes, allow_private).await {
                        Ok(response) => response,
                        Err(native_error) => {
                            #[cfg(feature = "browser-tools")]
                            {
                                tracing::debug!(error = %native_error, url = %url, "native fetch failed; trying browser reader");
                                fetch_with_browser_reader(
                                    &url,
                                    timeout_secs,
                                    max_bytes,
                                    allow_private,
                                    &ctx,
                                )
                                .await?
                            }
                            #[cfg(not(feature = "browser-tools"))]
                            {
                                return Err(native_error);
                            }
                        }
                    }
                }
            }
        } else {
            fetch_native(&url, timeout_secs, max_bytes, allow_private).await?
        };

        format_fetch_response(response, max_bytes)
    }
}

async fn fetch_native(
    url: &Url,
    timeout_secs: u64,
    max_bytes: u64,
    allow_private: bool,
) -> rua_core::Result<FetchResponse> {
    let client = rua_http::build_client(
        reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(USER_AGENT),
    )
    .map_err(|e| rua_core::RuaError::Tool {
        tool: "web_fetch".into(),
        message: format!("failed to build HTTP client: {e}"),
    })?;

    let mut current_url = url.clone();
    let mut redirects_remaining = MAX_REDIRECTS;
    let response = loop {
        let resp = browser_navigation_headers(client.get(current_url.as_str()))
            .send()
            .await
            .map_err(|e| rua_core::RuaError::Tool {
                tool: "web_fetch".into(),
                message: format!("request failed: {e}"),
            })?;

        if resp.status().is_redirection() {
            if redirects_remaining == 0 {
                return Err(rua_core::RuaError::Tool {
                    tool: "web_fetch".into(),
                    message: format!("too many redirects (max {MAX_REDIRECTS})"),
                });
            }
            redirects_remaining -= 1;

            let location = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| rua_core::RuaError::Tool {
                    tool: "web_fetch".into(),
                    message: "redirect without Location header".into(),
                })?;

            let next_url = current_url
                .join(location)
                .map_err(|e| rua_core::RuaError::Tool {
                    tool: "web_fetch".into(),
                    message: format!("invalid redirect URL: {e}"),
                })?;

            validate_url_ssrf(&next_url, allow_private)
                .await
                .map_err(rua_core::RuaError::from)?;

            current_url = next_url;
        } else {
            break resp;
        }
    };

    let success = response.status().is_success();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();

    let (body, truncated, original_len) = read_response_prefix(response, max_bytes).await?;

    Ok(FetchResponse {
        success,
        content_type,
        body,
        truncated,
        original_len,
    })
}

async fn fetch_with_jina_reader(
    url: &Url,
    timeout_secs: u64,
    max_bytes: u64,
    ctx: &ToolContext,
) -> rua_core::Result<FetchResponse> {
    let client = rua_http::build_client(
        reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
            .user_agent(USER_AGENT),
    )
    .map_err(|e| rua_core::RuaError::Tool {
        tool: "web_fetch".into(),
        message: format!("failed to build HTTP client: {e}"),
    })?;

    let reader_url = jina_reader_url(&jina_reader_base_url(ctx), url);
    let mut request = client
        .get(&reader_url)
        .header("Accept", "text/plain, text/markdown, */*");
    if let Some(api_key) = jina_api_key(ctx) {
        request = request.bearer_auth(api_key);
    }

    let response = request.send().await.map_err(|e| rua_core::RuaError::Tool {
        tool: "web_fetch".into(),
        message: format!("Jina reader request failed: {e}"),
    })?;

    if !response.status().is_success() {
        return Err(rua_core::RuaError::Tool {
            tool: "web_fetch".into(),
            message: format!("Jina reader returned status {}", response.status()),
        });
    }

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/markdown")
        .to_string();

    let (body, truncated, original_len) = read_response_prefix(response, max_bytes).await?;

    Ok(FetchResponse {
        success: true,
        content_type,
        body,
        truncated,
        original_len,
    })
}

async fn read_response_prefix(
    mut response: reqwest::Response,
    max_bytes: u64,
) -> rua_core::Result<(Vec<u8>, bool, Option<u64>)> {
    let content_len = response.content_length();
    let limit = usize::try_from(max_bytes).unwrap_or(usize::MAX);
    if limit == 0 {
        return Ok((Vec::new(), content_len != Some(0), content_len));
    }

    let mut body = Vec::with_capacity(limit.min(16 * 1024));
    let mut observed = 0u64;
    let mut truncated = content_len.is_some_and(|len| len > max_bytes);

    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| rua_core::RuaError::Tool {
            tool: "web_fetch".into(),
            message: format!("failed to read response: {e}"),
        })?
    {
        observed = observed.saturating_add(chunk.len() as u64);
        if body.len() < limit {
            let remaining = limit - body.len();
            let take = remaining.min(chunk.len());
            body.extend_from_slice(&chunk[..take]);
        }
        if body.len() >= limit {
            if observed > max_bytes || content_len.is_some_and(|len| len > max_bytes) {
                truncated = true;
            } else if content_len.is_none() {
                truncated = response
                    .chunk()
                    .await
                    .map_err(|e| rua_core::RuaError::Tool {
                        tool: "web_fetch".into(),
                        message: format!("failed to read response: {e}"),
                    })?
                    .is_some();
            }
            break;
        }
    }

    Ok((body, truncated, content_len))
}

#[cfg(feature = "browser-tools")]
async fn fetch_with_browser_reader(
    url: &Url,
    timeout_secs: u64,
    max_bytes: u64,
    allow_private: bool,
    ctx: &ToolContext,
) -> rua_core::Result<FetchResponse> {
    use chromiumoxide::browser::{Browser, BrowserConfig};
    use futures::StreamExt;

    let cdp_url = browser_cdp_url(ctx);
    let (mut browser, mut handler, launched_browser) = if let Some(cdp_url) = cdp_url {
        let (browser, handler) =
            Browser::connect(cdp_url.clone())
                .await
                .map_err(|e| rua_core::RuaError::Tool {
                    tool: "web_fetch".into(),
                    message: format!("failed to connect to browser CDP endpoint {cdp_url}: {e}"),
                })?;
        (browser, handler, false)
    } else {
        let config = BrowserConfig::builder()
            .no_sandbox()
            .new_headless_mode()
            .hide()
            .request_timeout(Duration::from_secs(timeout_secs))
            .launch_timeout(Duration::from_secs(timeout_secs))
            .arg("--disable-gpu")
            .arg("--disable-dev-shm-usage")
            .arg("--disable-blink-features=AutomationControlled")
            .build()
            .map_err(|e| rua_core::RuaError::Tool {
                tool: "web_fetch".into(),
                message: format!("failed to build browser config: {e}"),
            })?;
        let (browser, handler) =
            Browser::launch(config)
                .await
                .map_err(|e| rua_core::RuaError::Tool {
                    tool: "web_fetch".into(),
                    message: format!("failed to launch browser: {e}"),
                })?;
        (browser, handler, true)
    };

    let handler_task = tokio::spawn(async move {
        while let Some(result) = handler.next().await {
            if result.is_err() {
                break;
            }
        }
    });

    let result = async {
        let page = browser
            .new_page(url.as_str())
            .await
            .map_err(|e| rua_core::RuaError::Tool {
                tool: "web_fetch".into(),
                message: format!("failed to create browser page: {e}"),
            })?;

        page.wait_for_navigation()
            .await
            .map_err(|e| rua_core::RuaError::Tool {
                tool: "web_fetch".into(),
                message: format!("browser navigation failed: {e}"),
            })?;

        if let Some(final_url) = page.url().await.map_err(|e| rua_core::RuaError::Tool {
            tool: "web_fetch".into(),
            message: format!("failed to read browser page URL: {e}"),
        })? {
            let parsed = Url::parse(&final_url).map_err(|e| rua_core::RuaError::Tool {
                tool: "web_fetch".into(),
                message: format!("browser ended at invalid URL: {e}"),
            })?;
            validate_url_ssrf(&parsed, allow_private)
                .await
                .map_err(rua_core::RuaError::from)?;
        }

        let text = page
            .evaluate(
                "document.body ? (document.body.innerText || document.body.textContent || '') : ''",
            )
            .await
            .ok()
            .and_then(|result| result.into_value::<String>().ok())
            .unwrap_or_default();

        let (content_type, content) = if text.trim().is_empty() {
            let html = page.content().await.map_err(|e| rua_core::RuaError::Tool {
                tool: "web_fetch".into(),
                message: format!("failed to read browser page content: {e}"),
            })?;
            ("text/html".to_string(), html)
        } else {
            ("text/plain".to_string(), text)
        };

        let _ = page.close().await;

        let body = content.into_bytes();
        let original_len = body.len() as u64;

        Ok(FetchResponse {
            success: true,
            content_type,
            body,
            truncated: false,
            original_len: Some(original_len),
        })
    }
    .await;

    if launched_browser {
        let _ = browser.close().await;
        let _ = browser.wait().await;
        let _ = handler_task.await;
    } else {
        handler_task.abort();
    }

    result
}

#[cfg(not(feature = "browser-tools"))]
async fn fetch_with_browser_reader(
    _url: &Url,
    _timeout_secs: u64,
    _max_bytes: u64,
    _allow_private: bool,
    _ctx: &ToolContext,
) -> rua_core::Result<FetchResponse> {
    Err(rua_core::RuaError::Tool {
        tool: "web_fetch".into(),
        message: "browser reader is not available in this build; enable browser-tools or use reader=native".into(),
    })
}

fn format_fetch_response(response: FetchResponse, max_bytes: u64) -> rua_core::Result<ToolResult> {
    let original_len = response.original_len;
    let was_truncated_while_reading = response.truncated;
    let (body_bytes, truncated) = truncate_bytes(response.body, max_bytes as usize);
    let truncated = was_truncated_while_reading || truncated;

    let mut content = if response.content_type.contains("text/html") {
        let html = String::from_utf8_lossy(&body_bytes);
        html_to_markdown(&html)
    } else if response.content_type.contains("application/json")
        || response.content_type.starts_with("text/")
        || response.content_type.contains("markdown")
    {
        String::from_utf8_lossy(&body_bytes).into_owned()
    } else {
        format!(
            "[Binary content ({}, {} bytes). Use a file download tool to save this content.]",
            response.content_type,
            body_bytes.len()
        )
    };
    if truncated {
        let suffix = match original_len {
            Some(original_len) if original_len > max_bytes => format!(
                "\n\n[web_fetch truncated response at {max_bytes} bytes; original response was {original_len} bytes. Increase max_bytes to retrieve more content.]"
            ),
            _ => format!(
                "\n\n[web_fetch truncated response at {max_bytes} bytes; additional response content was omitted. Increase max_bytes to retrieve more content.]"
            ),
        };
        content.push_str(&suffix);
    }

    let result = if response.success {
        ToolResult::text("web_fetch", content)
    } else {
        ToolResult::error("web_fetch", content)
    };
    Ok(result)
}

fn truncate_bytes(mut bytes: Vec<u8>, max_bytes: usize) -> (Vec<u8>, bool) {
    if max_bytes == 0 {
        return (Vec::new(), !bytes.is_empty());
    }
    if bytes.len() <= max_bytes {
        return (bytes, false);
    }
    let mut end = max_bytes.min(bytes.len());
    while end > 0 && std::str::from_utf8(&bytes[..end]).is_err() {
        end -= 1;
    }
    if end == 0 {
        end = max_bytes.min(bytes.len());
    }
    bytes.truncate(end);
    (bytes, true)
}

pub fn html_to_markdown(html: &str) -> String {
    let mut output = String::with_capacity(html.len());
    let mut pos = 0;
    let bytes = html.as_bytes();
    let len = bytes.len();

    let mut in_script = false;
    let mut in_style = false;
    let mut in_pre = false;
    let mut ol_counter: Vec<u32> = Vec::new();
    let mut ul_depth: usize = 0;

    while pos < len {
        if bytes[pos] == b'<' {
            let tag_start = pos;
            if let Some(tag_end) = find_tag_end(html, pos) {
                let tag_content = &html[tag_start + 1..tag_end];
                let (tag_name, attrs) = parse_tag(tag_content);
                let tag_lower = tag_name.to_lowercase();
                let is_closing = tag_lower.starts_with('/');
                let clean_tag = tag_lower.trim_start_matches('/');

                match clean_tag {
                    "script" => {
                        in_script = !is_closing;
                    }
                    "style" => {
                        in_style = !is_closing;
                    }
                    _ if in_script || in_style => {}
                    "h1" if !is_closing => output.push_str("\n\n# "),
                    "h2" if !is_closing => output.push_str("\n\n## "),
                    "h3" if !is_closing => output.push_str("\n\n### "),
                    "h4" if !is_closing => output.push_str("\n\n#### "),
                    "h5" if !is_closing => output.push_str("\n\n##### "),
                    "h6" if !is_closing => output.push_str("\n\n###### "),
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" if is_closing => {
                        output.push_str("\n\n");
                    }
                    "p" if !is_closing => output.push_str("\n\n"),
                    "p" if is_closing => output.push_str("\n\n"),
                    "br" => output.push('\n'),
                    "hr" => output.push_str("\n\n---\n\n"),
                    "a" if !is_closing => {
                        let href = extract_attr(attrs, "href").unwrap_or_default();
                        let link_text = extract_inner_text(html, tag_end + 1, "a");
                        if !href.is_empty() && !link_text.is_empty() {
                            output.push_str(&format!("[{}]({})", link_text.trim(), href));
                        } else if !link_text.is_empty() {
                            output.push_str(link_text.trim());
                        }

                        if let Some(close_pos) = find_closing_tag(html, tag_end + 1, "a") {
                            pos = close_pos + 1;
                            continue;
                        }
                    }
                    "strong" | "b" if !is_closing => output.push_str("**"),
                    "strong" | "b" if is_closing => output.push_str("**"),
                    "em" | "i" if !is_closing => output.push('*'),
                    "em" | "i" if is_closing => output.push('*'),
                    "code" if !is_closing && !in_pre => output.push('`'),
                    "code" if is_closing && !in_pre => output.push('`'),
                    "pre" if !is_closing => {
                        in_pre = true;
                        output.push_str("\n\n```\n");
                    }
                    "pre" if is_closing => {
                        in_pre = false;
                        output.push_str("\n```\n\n");
                    }
                    "ul" if !is_closing => {
                        ul_depth += 1;
                        output.push('\n');
                    }
                    "ul" if is_closing => {
                        ul_depth = ul_depth.saturating_sub(1);
                        output.push('\n');
                    }
                    "ol" if !is_closing => {
                        ol_counter.push(0);
                        output.push('\n');
                    }
                    "ol" if is_closing => {
                        ol_counter.pop();
                        output.push('\n');
                    }
                    "li" if !is_closing => {
                        if let Some(counter) = ol_counter.last_mut() {
                            *counter += 1;
                            output.push_str(&format!("{}. ", counter));
                        } else if ul_depth > 0 {
                            output.push_str("- ");
                        }
                    }
                    "li" if is_closing => output.push('\n'),
                    "blockquote" if !is_closing => output.push_str("\n\n> "),
                    "blockquote" if is_closing => output.push_str("\n\n"),
                    "div" | "section" | "article" | "header" | "footer" | "main" | "nav"
                        if !is_closing =>
                    {
                        output.push('\n');
                    }
                    "img" if !is_closing => {
                        let alt = extract_attr(attrs, "alt").unwrap_or_default();
                        let src = extract_attr(attrs, "src").unwrap_or_default();
                        if !src.is_empty() {
                            output.push_str(&format!("![{}]({})", alt, src));
                        }
                    }
                    _ => {}
                }
                pos = tag_end + 1;
                continue;
            }
        }

        if !in_script && !in_style {
            output.push(html.as_bytes()[pos] as char);
        }
        pos += 1;
    }

    let decoded = decode_html_entities(&output);
    collapse_blank_lines(&decoded)
}

fn find_tag_end(html: &str, start: usize) -> Option<usize> {
    let bytes = html.as_bytes();
    let mut i = start + 1;
    let mut in_quote = false;
    let mut quote_char = b'"';
    while i < bytes.len() {
        if in_quote {
            if bytes[i] == quote_char {
                in_quote = false;
            }
        } else if bytes[i] == b'"' || bytes[i] == b'\'' {
            in_quote = true;
            quote_char = bytes[i];
        } else if bytes[i] == b'>' {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn parse_tag(tag_content: &str) -> (&str, &str) {
    let trimmed = tag_content.trim();
    if let Some(space_pos) = trimmed.find(|c: char| c.is_whitespace()) {
        (&trimmed[..space_pos], &trimmed[space_pos..])
    } else {
        (trimmed, "")
    }
}

fn extract_attr(attrs: &str, name: &str) -> Option<String> {
    let pattern = format!("{}=\"", name);
    if let Some(start) = attrs.to_lowercase().find(&pattern) {
        let val_start = start + pattern.len();
        let remaining = &attrs[val_start..];
        if let Some(end) = remaining.find('"') {
            return Some(remaining[..end].to_string());
        }
    }
    let pattern_single = format!("{}='", name);
    if let Some(start) = attrs.to_lowercase().find(&pattern_single) {
        let val_start = start + pattern_single.len();
        let remaining = &attrs[val_start..];
        if let Some(end) = remaining.find('\'') {
            return Some(remaining[..end].to_string());
        }
    }
    None
}

fn extract_inner_text(html: &str, start: usize, tag: &str) -> String {
    let close_tag = format!("</{}", tag);
    if let Some(close_pos) = html[start..].to_lowercase().find(&close_tag) {
        let inner = &html[start..start + close_pos];

        let mut result = String::new();
        let mut in_tag = false;
        for ch in inner.chars() {
            if ch == '<' {
                in_tag = true;
            } else if ch == '>' {
                in_tag = false;
            } else if !in_tag {
                result.push(ch);
            }
        }
        decode_html_entities(&result)
    } else {
        String::new()
    }
}

fn find_closing_tag(html: &str, start: usize, tag: &str) -> Option<usize> {
    let close_tag = format!("</{}", tag);
    let lower = html[start..].to_lowercase();
    if let Some(pos) = lower.find(&close_tag) {
        let abs_pos = start + pos;
        find_tag_end(html, abs_pos)
    } else {
        None
    }
}

fn decode_html_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&#x2F;", "/")
        .replace("&nbsp;", " ")
}

fn collapse_blank_lines(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut blank_count = 0;
    for line in s.lines() {
        if line.trim().is_empty() {
            blank_count += 1;
            if blank_count <= 2 {
                result.push('\n');
            }
        } else {
            blank_count = 0;
            result.push_str(line);
            result.push('\n');
        }
    }
    result.trim().to_string()
}
