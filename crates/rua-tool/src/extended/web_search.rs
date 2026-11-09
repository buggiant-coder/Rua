use std::collections::BTreeMap;
use std::panic::{catch_unwind, UnwindSafe};
use std::time::Duration;

use async_trait::async_trait;
use mcp_manager::{McpCallContext, McpClient};
use rua_mcp::transports::http::{HttpConfig, HttpMcpClient};
use serde::{Deserialize, Serialize};
use serde_json::json;
use url::Url;

use rua_contract::Tool;
use rua_core::tool::{ToolContext, ToolResult, ToolSchema};

use crate::annotations::{ApprovalPolicy, ExtendedToolAnnotations};
use crate::error::ToolError;
use crate::schema::ToolSpec;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

#[async_trait]
pub trait SearchProvider: Send + Sync {
    fn name(&self) -> &str;

    async fn search(&self, query: &str, max_results: usize)
        -> Result<Vec<SearchResult>, ToolError>;
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

fn web_search_config(ctx: &ToolContext) -> Option<&serde_json::Map<String, serde_json::Value>> {
    ctx.metadata
        .get("tool_config")
        .and_then(|value| value.as_object())
        .or_else(|| {
            ctx.metadata
                .get("tool_configs")
                .and_then(|value| value.get("web_search"))
                .and_then(|value| value.as_object())
        })
}

fn config_provider_id(provider: &str) -> &'static [&'static str] {
    match provider {
        "exa" => &["exa"],
        "tavily" => &["tavily"],
        "serper" => &["serper"],
        "brave" | "brave_search" => &["brave", "brave_search"],
        "searxng" | "searx" => &["searxng", "searx"],
        _ => &[],
    }
}

fn config_provider_field(ctx: &ToolContext, provider: &str, field: &str) -> Option<String> {
    let ids = config_provider_id(provider);
    web_search_config(ctx)?
        .get("providers")?
        .as_array()?
        .iter()
        .find_map(|item| {
            let obj = item.as_object()?;
            let id = obj.get("id")?.as_str()?;
            if !ids.contains(&id) {
                return None;
            }
            obj.get(field)
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
        })
}

fn first_text(item: &serde_json::Value, keys: &[&str]) -> String {
    for key in keys {
        if let Some(value) = item.get(*key).and_then(|v| v.as_str()) {
            let value = value.trim();
            if !value.is_empty() {
                return value.to_string();
            }
        }
    }
    String::new()
}

fn highlights_text(item: &serde_json::Value) -> String {
    item.get("highlights")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_default()
}

fn response_status_error(provider: &str, status: reqwest::StatusCode) -> ToolError {
    ToolError::WebSearch {
        message: format!("{provider} search returned status {status}"),
    }
}

fn parse_json_error(provider: &str, error: reqwest::Error) -> ToolError {
    ToolError::WebSearch {
        message: format!("Failed to parse {provider} response: {error}"),
    }
}

fn request_error(provider: &str, error: reqwest::Error) -> ToolError {
    ToolError::WebSearch {
        message: format!("{provider} search request failed: {error}"),
    }
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_string();
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    "unknown panic".to_string()
}

fn build_http_client<F>(provider: &str, configure: F) -> Result<reqwest::Client, ToolError>
where
    F: FnOnce(reqwest::ClientBuilder) -> reqwest::ClientBuilder + UnwindSafe,
{
    match catch_unwind(|| rua_http::build_client(configure(reqwest::Client::builder()))) {
        Ok(Ok(client)) => Ok(client),
        Ok(Err(error)) => Err(ToolError::WebSearch {
            message: format!("Failed to build {provider} HTTP client: {error}"),
        }),
        Err(payload) => Err(ToolError::WebSearch {
            message: format!(
                "Failed to build {provider} HTTP client: {}",
                panic_message(payload)
            ),
        }),
    }
}

fn default_http_client(provider: &str) -> Result<reqwest::Client, ToolError> {
    build_http_client(provider, |builder| builder)
}

pub struct NoSearchProvider;

#[async_trait]
impl SearchProvider for NoSearchProvider {
    fn name(&self) -> &str {
        "none"
    }

    async fn search(
        &self,
        _query: &str,
        _max_results: usize,
    ) -> Result<Vec<SearchResult>, ToolError> {
        Err(ToolError::WebSearch {
            message: "No search provider configured. \
                      Set a search provider (Brave Search, SearXNG, etc.) in your configuration \
                      with the required API key."
                .to_string(),
        })
    }
}

pub struct BraveSearchProvider {
    api_key: String,
    base_url: String,
}

impl BraveSearchProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: "https://api.search.brave.com/res/v1/web/search".to_string(),
        }
    }

    pub fn with_base_url(api_key: String, base_url: String) -> Self {
        Self { api_key, base_url }
    }
}

pub struct ExaSearchProvider {
    api_key: String,
    base_url: String,
}

impl ExaSearchProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: "https://api.exa.ai/search".to_string(),
        }
    }

    pub fn with_base_url(api_key: String, base_url: String) -> Self {
        Self { api_key, base_url }
    }
}

#[async_trait]
impl SearchProvider for ExaSearchProvider {
    fn name(&self) -> &str {
        "exa"
    }

    async fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchResult>, ToolError> {
        let client = default_http_client("Exa")?;
        let response = client
            .post(&self.base_url)
            .header("x-api-key", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&json!({
                "query": query,
                "numResults": max_results,
                "contents": {
                    "text": false,
                    "highlights": true
                }
            }))
            .send()
            .await
            .map_err(|e| request_error("Exa", e))?;

        if !response.status().is_success() {
            return Err(response_status_error("Exa", response.status()));
        }

        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| parse_json_error("Exa", e))?;
        let results = body
            .get("results")
            .and_then(|r| r.as_array())
            .into_iter()
            .flatten()
            .take(max_results)
            .map(|item| {
                let snippet = {
                    let highlights = highlights_text(item);
                    if highlights.is_empty() {
                        first_text(item, &["summary", "text"])
                    } else {
                        highlights
                    }
                };
                SearchResult {
                    title: first_text(item, &["title"]),
                    url: first_text(item, &["url", "id"]),
                    snippet,
                }
            })
            .filter(|r| !r.title.is_empty() && !r.url.is_empty())
            .collect();

        Ok(results)
    }
}

const DEFAULT_EXA_MCP_URL: &str = "https://mcp.exa.ai/mcp?tools=web_search_exa";

pub struct ExaMcpSearchProvider {
    base_url: String,
    agent_id: String,
    session_id: String,
    cancel: Option<tokio::sync::watch::Receiver<bool>>,
    allow_private_ip: bool,
}

impl ExaMcpSearchProvider {
    pub fn new() -> Self {
        Self {
            base_url: DEFAULT_EXA_MCP_URL.to_string(),
            agent_id: String::new(),
            session_id: String::new(),
            cancel: None,
            allow_private_ip: false,
        }
    }

    pub fn with_base_url(base_url: String) -> Self {
        Self {
            base_url,
            agent_id: String::new(),
            session_id: String::new(),
            cancel: None,
            allow_private_ip: true,
        }
    }

    fn from_context(base_url: String, ctx: &ToolContext) -> Self {
        Self {
            base_url,
            agent_id: ctx.agent_id.clone(),
            session_id: ctx.session_id.clone(),
            cancel: ctx.cancel.clone(),
            allow_private_ip: false,
        }
    }
}

impl Default for ExaMcpSearchProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SearchProvider for ExaMcpSearchProvider {
    fn name(&self) -> &str {
        "exa"
    }

    async fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchResult>, ToolError> {
        let client = HttpMcpClient::new(HttpConfig {
            server_key: "exa-keyless".to_string(),
            server_name: "Exa hosted MCP".to_string(),
            url: self.base_url.clone(),
            headers: BTreeMap::new(),
            timeout: Duration::from_secs(5),
            call_timeout: Duration::from_secs(15),
            allow_private_ip: self.allow_private_ip,
        });
        client
            .connect()
            .await
            .map_err(|error| ToolError::WebSearch {
                message: format!("Exa hosted MCP connection failed: {error}"),
            })?;

        let result = client
            .call_tool(
                "web_search_exa",
                json!({
                    "query": query,
                    "numResults": max_results,
                }),
                McpCallContext {
                    agent_id: self.agent_id.clone(),
                    session_id: self.session_id.clone(),
                    cancel: self.cancel.clone(),
                    timeout: Some(Duration::from_secs(15)),
                    ..Default::default()
                },
            )
            .await
            .map_err(|error| ToolError::WebSearch {
                message: format!("Exa hosted MCP search failed: {error}"),
            })?;

        if result.is_error {
            return Err(ToolError::WebSearch {
                message: format!("Exa hosted MCP search failed: {}", result.text()),
            });
        }

        Ok(parse_exa_mcp_results(&result.text(), max_results))
    }
}

fn parse_exa_mcp_results(text: &str, max_results: usize) -> Vec<SearchResult> {
    text.split("\n\n---\n\n")
        .filter_map(|block| {
            let mut title = String::new();
            let mut url = String::new();
            let mut highlights = Vec::new();
            let mut in_highlights = false;

            for line in block.lines() {
                if let Some(value) = line.strip_prefix("Title:") {
                    title = value.trim().to_string();
                    in_highlights = false;
                } else if let Some(value) = line.strip_prefix("URL:") {
                    url = value.trim().to_string();
                    in_highlights = false;
                } else if let Some(value) = line.strip_prefix("Highlights:") {
                    in_highlights = true;
                    let value = value.trim();
                    if !value.is_empty() {
                        highlights.push(value.to_string());
                    }
                } else if in_highlights && !line.trim().is_empty() {
                    highlights.push(line.trim().to_string());
                }
            }

            (!title.is_empty() && is_public_http_url(&url)).then(|| SearchResult {
                title,
                url,
                snippet: highlights.join(" "),
            })
        })
        .take(max_results)
        .collect()
}

pub struct TavilySearchProvider {
    api_key: Option<String>,
    base_url: String,
}

impl TavilySearchProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key: Some(api_key),
            base_url: "https://api.tavily.com/search".to_string(),
        }
    }

    pub fn with_base_url(api_key: String, base_url: String) -> Self {
        Self {
            api_key: Some(api_key),
            base_url,
        }
    }

    pub fn keyless() -> Self {
        Self {
            api_key: None,
            base_url: "https://api.tavily.com/search".to_string(),
        }
    }

    pub fn keyless_with_base_url(base_url: String) -> Self {
        Self {
            api_key: None,
            base_url,
        }
    }
}

#[async_trait]
impl SearchProvider for TavilySearchProvider {
    fn name(&self) -> &str {
        "tavily"
    }

    async fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchResult>, ToolError> {
        let client = default_http_client("Tavily")?;
        let mut request = client
            .post(&self.base_url)
            .header("Content-Type", "application/json")
            .json(&json!({
                "query": query,
                "max_results": max_results,
                "search_depth": "basic",
                "include_answer": false,
                "include_raw_content": false
            }));
        request = match &self.api_key {
            Some(api_key) => request.bearer_auth(api_key),
            None => request.header("X-Tavily-Access-Mode", "keyless"),
        };
        let response = request
            .send()
            .await
            .map_err(|e| request_error("Tavily", e))?;

        if !response.status().is_success() {
            return Err(response_status_error("Tavily", response.status()));
        }

        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| parse_json_error("Tavily", e))?;
        let results = body
            .get("results")
            .and_then(|r| r.as_array())
            .into_iter()
            .flatten()
            .take(max_results)
            .map(|item| SearchResult {
                title: first_text(item, &["title"]),
                url: first_text(item, &["url"]),
                snippet: first_text(item, &["content", "raw_content"]),
            })
            .filter(|r| !r.title.is_empty() && !r.url.is_empty())
            .collect();

        Ok(results)
    }
}

pub struct SerperSearchProvider {
    api_key: String,
    base_url: String,
}

impl SerperSearchProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: "https://google.serper.dev/search".to_string(),
        }
    }

    pub fn with_base_url(api_key: String, base_url: String) -> Self {
        Self { api_key, base_url }
    }
}

#[async_trait]
impl SearchProvider for SerperSearchProvider {
    fn name(&self) -> &str {
        "serper"
    }

    async fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchResult>, ToolError> {
        let client = default_http_client("Serper")?;
        let response = client
            .post(&self.base_url)
            .header("X-API-KEY", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&json!({
                "q": query,
                "num": max_results
            }))
            .send()
            .await
            .map_err(|e| request_error("Serper", e))?;

        if !response.status().is_success() {
            return Err(response_status_error("Serper", response.status()));
        }

        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| parse_json_error("Serper", e))?;
        let results = body
            .get("organic")
            .and_then(|r| r.as_array())
            .into_iter()
            .flatten()
            .take(max_results)
            .map(|item| SearchResult {
                title: first_text(item, &["title"]),
                url: first_text(item, &["link"]),
                snippet: first_text(item, &["snippet"]),
            })
            .filter(|r| !r.title.is_empty() && !r.url.is_empty())
            .collect();

        Ok(results)
    }
}

#[async_trait]
impl SearchProvider for BraveSearchProvider {
    fn name(&self) -> &str {
        "brave"
    }

    async fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchResult>, ToolError> {
        let client = default_http_client("Brave")?;

        let response = client
            .get(&self.base_url)
            .header("X-Subscription-Token", &self.api_key)
            .header("Accept", "application/json")
            .query(&[("q", query), ("count", &max_results.to_string())])
            .send()
            .await
            .map_err(|e| ToolError::WebSearch {
                message: format!("Brave search request failed: {e}"),
            })?;

        if !response.status().is_success() {
            return Err(ToolError::WebSearch {
                message: format!("Brave search returned status {}", response.status()),
            });
        }

        let body: serde_json::Value = response.json().await.map_err(|e| ToolError::WebSearch {
            message: format!("Failed to parse Brave response: {e}"),
        })?;

        let mut results = Vec::new();
        if let Some(web) = body.get("web").and_then(|w| w.get("results")) {
            if let Some(arr) = web.as_array() {
                for item in arr.iter().take(max_results) {
                    results.push(SearchResult {
                        title: item
                            .get("title")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string(),
                        url: item
                            .get("url")
                            .and_then(|u| u.as_str())
                            .unwrap_or("")
                            .to_string(),
                        snippet: item
                            .get("description")
                            .and_then(|d| d.as_str())
                            .unwrap_or("")
                            .to_string(),
                    });
                }
            }
        }

        Ok(results)
    }
}

pub struct FallbackSearchProvider {
    providers: Vec<Box<dyn SearchProvider>>,
}

impl FallbackSearchProvider {
    pub fn new(providers: Vec<Box<dyn SearchProvider>>) -> Self {
        Self { providers }
    }
}

#[async_trait]
impl SearchProvider for FallbackSearchProvider {
    fn name(&self) -> &str {
        "fallback"
    }

    async fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchResult>, ToolError> {
        let mut failures = Vec::new();
        for provider in &self.providers {
            match provider.search(query, max_results).await {
                Ok(results) if !results.is_empty() => return Ok(results),
                Ok(_) => failures.push(format!("{} returned no results", provider.name())),
                Err(error) => failures.push(format!("{}: {error}", provider.name())),
            }
        }

        Err(ToolError::WebSearch {
            message: format!(
                "all fallback search providers failed: {}",
                failures.join("; ")
            ),
        })
    }
}

fn is_public_http_url(url: &str) -> bool {
    Url::parse(url)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https"))
        .and_then(|url| url.host_str().map(str::to_string))
        .is_some()
}

pub struct SearxngProvider {
    base_url: String,
}

impl SearxngProvider {
    pub fn new(base_url: String) -> Self {
        Self { base_url }
    }
}

fn normalize_provider_name(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace(['-', ' '], "_")
}

fn provider_api_key(ctx: &ToolContext, provider: &str) -> Option<String> {
    if let Some(value) = config_provider_field(ctx, provider, "api_key") {
        return Some(value);
    }
    match provider {
        "exa" => metadata_or_env(
            ctx,
            &["exa_api_key", "search_provider_api_key"],
            &["EXA_API_KEY", "RUA_EXA_API_KEY", "RUA_SEARCH_API_KEY"],
        ),
        "tavily" => metadata_or_env(
            ctx,
            &["tavily_api_key", "search_provider_api_key"],
            &["TAVILY_API_KEY", "RUA_TAVILY_API_KEY", "RUA_SEARCH_API_KEY"],
        ),
        "serper" => metadata_or_env(
            ctx,
            &["serper_api_key", "search_provider_api_key"],
            &["SERPER_API_KEY", "RUA_SERPER_API_KEY", "RUA_SEARCH_API_KEY"],
        ),
        "brave" | "brave_search" => metadata_or_env(
            ctx,
            &[
                "brave_search_api_key",
                "brave_api_key",
                "search_provider_api_key",
            ],
            &[
                "BRAVE_SEARCH_API_KEY",
                "RUA_BRAVE_SEARCH_API_KEY",
                "RUA_SEARCH_API_KEY",
            ],
        ),
        _ => None,
    }
}

fn provider_specific_key(ctx: &ToolContext, provider: &str) -> Option<String> {
    match provider {
        "exa" => metadata_or_env(ctx, &["exa_api_key"], &["EXA_API_KEY", "RUA_EXA_API_KEY"]),
        "tavily" => metadata_or_env(
            ctx,
            &["tavily_api_key"],
            &["TAVILY_API_KEY", "RUA_TAVILY_API_KEY"],
        ),
        "serper" => metadata_or_env(
            ctx,
            &["serper_api_key"],
            &["SERPER_API_KEY", "RUA_SERPER_API_KEY"],
        ),
        "brave" | "brave_search" => metadata_or_env(
            ctx,
            &[
                "brave_search_api_key",
                "brave_api_key",
                "search_provider_api_key",
            ],
            &["BRAVE_SEARCH_API_KEY", "RUA_BRAVE_SEARCH_API_KEY"],
        ),
        _ => None,
    }
}

fn provider_base_url(ctx: &ToolContext, provider: &str) -> Option<String> {
    if let Some(value) = config_provider_field(ctx, provider, "base_url") {
        return Some(value);
    }
    match provider {
        "exa" => metadata_or_env(ctx, &["exa_base_url"], &["RUA_EXA_BASE_URL"]),
        "tavily" => metadata_or_env(ctx, &["tavily_base_url"], &["RUA_TAVILY_BASE_URL"]),
        "serper" => metadata_or_env(ctx, &["serper_base_url"], &["RUA_SERPER_BASE_URL"]),
        "brave" | "brave_search" => metadata_or_env(
            ctx,
            &["brave_search_base_url", "brave_base_url"],
            &["RUA_BRAVE_SEARCH_BASE_URL"],
        ),
        "searxng" | "searx" => metadata_or_env(
            ctx,
            &["searxng_url", "searxng_base_url"],
            &["SEARXNG_URL", "RUA_SEARXNG_URL"],
        ),
        _ => None,
    }
}

fn exa_mcp_url(ctx: &ToolContext) -> String {
    config_provider_field(ctx, "exa", "mcp_url")
        .or_else(|| metadata_or_env(ctx, &["exa_mcp_url"], &["RUA_EXA_MCP_URL"]))
        .unwrap_or_else(|| DEFAULT_EXA_MCP_URL.to_string())
}

fn build_provider(ctx: &ToolContext, provider: &str) -> Result<Box<dyn SearchProvider>, ToolError> {
    match normalize_provider_name(provider).as_str() {
        "exa" => {
            if let Some(key) = provider_api_key(ctx, "exa") {
                Ok(match provider_base_url(ctx, "exa") {
                    Some(url) => Box::new(ExaSearchProvider::with_base_url(key, url)),
                    None => Box::new(ExaSearchProvider::new(key)),
                })
            } else {
                Ok(Box::new(ExaMcpSearchProvider::from_context(
                    exa_mcp_url(ctx),
                    ctx,
                )))
            }
        }
        "tavily" => {
            let key = provider_api_key(ctx, "tavily");
            Ok(match (key, provider_base_url(ctx, "tavily")) {
                (Some(key), Some(url)) => Box::new(TavilySearchProvider::with_base_url(key, url)),
                (Some(key), None) => Box::new(TavilySearchProvider::new(key)),
                (None, Some(url)) => Box::new(TavilySearchProvider::keyless_with_base_url(url)),
                (None, None) => Box::new(TavilySearchProvider::keyless()),
            })
        }
        "serper" => {
            let key = provider_api_key(ctx, "serper").ok_or_else(|| ToolError::WebSearch {
                message:
                    "Serper search selected but no SERPER_API_KEY/RUA_SERPER_API_KEY is configured"
                        .to_string(),
            })?;
            Ok(match provider_base_url(ctx, "serper") {
                Some(url) => Box::new(SerperSearchProvider::with_base_url(key, url)),
                None => Box::new(SerperSearchProvider::new(key)),
            })
        }
        "brave" | "brave_search" => {
            let key = provider_api_key(ctx, "brave").ok_or_else(|| ToolError::WebSearch {
                message:
                    "Brave search selected but no BRAVE_SEARCH_API_KEY/RUA_BRAVE_SEARCH_API_KEY is configured"
                        .to_string(),
            })?;
            Ok(match provider_base_url(ctx, "brave") {
                Some(url) => Box::new(BraveSearchProvider::with_base_url(key, url)),
                None => Box::new(BraveSearchProvider::new(key)),
            })
        }
        "searxng" | "searx" => {
            let url = provider_base_url(ctx, "searxng").ok_or_else(|| ToolError::WebSearch {
                message: "SearXNG search selected but no SEARXNG_URL/RUA_SEARXNG_URL is configured"
                    .to_string(),
            })?;
            Ok(Box::new(SearxngProvider::new(url)))
        }
        other => Err(ToolError::WebSearch {
            message: format!("unknown web search provider: {other}"),
        }),
    }
}

fn resolve_search_provider(ctx: &ToolContext) -> Result<Box<dyn SearchProvider>, ToolError> {
    if let Some(provider) = metadata_or_env(
        ctx,
        &["search_provider", "web_search_provider"],
        &["RUA_SEARCH_PROVIDER"],
    ) {
        return build_provider(ctx, &provider);
    }

    if let Some(config) = web_search_config(ctx) {
        if let Some(providers) = config.get("providers").and_then(|value| value.as_array()) {
            let mut chain = Vec::new();
            let mut failures = Vec::new();
            let mut saw_enabled = false;
            for provider in providers {
                if provider
                    .get("enabled")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false)
                {
                    saw_enabled = true;
                    let Some(id) = provider.get("id").and_then(|value| value.as_str()) else {
                        continue;
                    };
                    let provider_name = match id {
                        "exa" => "exa",
                        "tavily" => "tavily",
                        "serper" => "serper",
                        "brave" | "brave_search" => "brave",
                        "searxng" | "searx" => "searxng",
                        _ => {
                            failures.push(format!("{id}: unsupported provider"));
                            continue;
                        }
                    };
                    match build_provider(ctx, provider_name) {
                        Ok(provider) => chain.push(provider),
                        Err(error) => failures.push(format!("{provider_name}: {error}")),
                    }
                }
            }
            if !chain.is_empty() {
                return Ok(Box::new(FallbackSearchProvider::new(chain)));
            }
            if saw_enabled {
                return Err(ToolError::WebSearch {
                    message: format!(
                        "configured web search providers are unavailable: {}",
                        failures.join("; ")
                    ),
                });
            }
            return Err(ToolError::WebSearch {
                message: "no web search providers are enabled".to_string(),
            });
        }
    }

    for provider in ["tavily", "exa", "serper", "brave"] {
        if provider_specific_key(ctx, provider).is_some() {
            return build_provider(ctx, provider);
        }
    }

    if provider_base_url(ctx, "searxng").is_some() {
        return build_provider(ctx, "searxng");
    }

    Ok(Box::new(FallbackSearchProvider::new(vec![
        build_provider(ctx, "tavily")?,
        build_provider(ctx, "exa")?,
    ])))
}

#[async_trait]
impl SearchProvider for SearxngProvider {
    fn name(&self) -> &str {
        "searxng"
    }

    async fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchResult>, ToolError> {
        let client = default_http_client("SearXNG")?;
        let url = format!("{}/search", self.base_url.trim_end_matches('/'));

        let response = client
            .get(&url)
            .query(&[("q", query), ("format", "json"), ("pageno", "1")])
            .send()
            .await
            .map_err(|e| ToolError::WebSearch {
                message: format!("SearXNG request failed: {e}"),
            })?;

        if !response.status().is_success() {
            return Err(ToolError::WebSearch {
                message: format!("SearXNG returned status {}", response.status()),
            });
        }

        let body: serde_json::Value = response.json().await.map_err(|e| ToolError::WebSearch {
            message: format!("Failed to parse SearXNG response: {e}"),
        })?;

        let mut results = Vec::new();
        if let Some(arr) = body.get("results").and_then(|r| r.as_array()) {
            for item in arr.iter().take(max_results) {
                results.push(SearchResult {
                    title: item
                        .get("title")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string(),
                    url: item
                        .get("url")
                        .and_then(|u| u.as_str())
                        .unwrap_or("")
                        .to_string(),
                    snippet: item
                        .get("content")
                        .and_then(|c| c.as_str())
                        .unwrap_or("")
                        .to_string(),
                });
            }
        }

        Ok(results)
    }
}

#[derive(Debug, Deserialize)]
struct WebSearchArgs {
    query: String,
    #[serde(default)]
    max_results: Option<usize>,
}

const DEFAULT_MAX_RESULTS: usize = 10;

pub struct WebSearchTool;

impl WebSearchTool {
    pub fn new() -> Self {
        Self
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "web_search".to_string(),
            description: "Search the web using the configured search provider. \
                 Defaults to Tavily keyless search, then Exa hosted MCP. \
                 User API keys and provider order can be configured per agent. \
                 Supports Exa, Tavily, Serper, Brave Search, and SearXNG."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of results (default: 10)"
                    }
                },
                "required": ["query"]
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

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
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
        let args: WebSearchArgs =
            serde_json::from_value(args).map_err(|e| rua_core::RuaError::Tool {
                tool: "web_search".into(),
                message: e.to_string(),
            })?;

        let max_results = args.max_results.unwrap_or(DEFAULT_MAX_RESULTS);

        let provider = resolve_search_provider(&ctx).map_err(rua_core::RuaError::from)?;

        let results = provider
            .search(&args.query, max_results)
            .await
            .map_err(rua_core::RuaError::from)?;

        if results.is_empty() {
            return Ok(ToolResult::text(
                "web_search",
                format!("No results found for: {}", args.query),
            ));
        }

        let parts: Vec<String> = results
            .iter()
            .enumerate()
            .map(|(i, r)| {
                format!(
                    "{}. {}\n   URL: {}\n   Snippet: {}",
                    i + 1,
                    r.title,
                    r.url,
                    r.snippet
                )
            })
            .collect();
        let output = parts.join("\n\n");

        Ok(ToolResult::text("web_search", output))
    }
}
