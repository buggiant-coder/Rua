use std::sync::Arc;

use async_trait::async_trait;
use rua_skills_hub::{http_get_bytes_with_retry, HubClient, HubError};
use serde_json::Value;

use crate::provider::{MarketProvider, SearchOutcome};
use crate::schema::{MarketResult, MarketSearchError, StatValue};

const DEFAULT_BASE_URL: &str = "https://api.skillhub.cn";
const SEARCH_PATH: &str = "/api/skills";

pub struct SkillHubProvider {
    client: Arc<HubClient>,
    key: String,
    label: String,
    base_url: String,
    lang: String,
}

impl SkillHubProvider {
    pub fn new(client: Arc<HubClient>) -> Self {
        Self::with_options(client, DEFAULT_BASE_URL.to_string(), "en".to_string())
    }

    pub fn with_options(client: Arc<HubClient>, base_url: String, lang: String) -> Self {
        let trimmed = base_url.trim_end_matches('/').to_string();
        Self {
            client,
            key: "skillhub".to_string(),
            label: "SkillHub".to_string(),
            base_url: trimmed,
            lang,
        }
    }
}

#[async_trait]
impl MarketProvider for SkillHubProvider {
    fn key(&self) -> &str {
        &self.key
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn available(&self) -> (bool, Option<String>) {
        (true, None)
    }

    async fn search(
        &self,
        query: &str,
        limit: u32,
        page: u32,
    ) -> Result<SearchOutcome, MarketSearchError> {
        let page_size = limit.max(1);
        let page_number = page.max(1);

        let mut url = format!(
            "{}{}?page={}&pageSize={}",
            self.base_url, SEARCH_PATH, page_number, page_size,
        );
        let needle = query.trim();
        if !needle.is_empty() {
            url.push_str("&keyword=");
            url.push_str(&percent_encode(needle));
        }

        let bytes = http_get_bytes_with_retry(&self.client, &url, None)
            .await
            .map_err(|e| map_http_error(&e))?;

        let body: Value = serde_json::from_slice(&bytes).map_err(|_| {
            MarketSearchError::new("skillhub", "SkillHub search failed: non-JSON response")
        })?;

        let obj = body.as_object().ok_or_else(|| {
            MarketSearchError::new("skillhub", "SkillHub search failed: non-JSON response")
        })?;

        let code = obj.get("code").and_then(Value::as_i64).unwrap_or(-1);
        if code != 0 {
            let message = obj
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            return Err(MarketSearchError::new(
                "skillhub",
                format!("SkillHub search failed: {message}"),
            ));
        }

        let data = obj.get("data").and_then(Value::as_object);
        let mut results: Vec<MarketResult> = Vec::new();
        let mut upstream_total: Option<u32> = None;
        if let Some(data) = data {
            if let Some(skills) = data.get("skills").and_then(Value::as_array) {
                for item in skills {
                    if let Some(item) = item.as_object() {
                        if let Some(r) = to_market_result(item, &self.lang) {
                            results.push(r);
                        }
                    }
                }
            }
            upstream_total = data
                .get("total")
                .and_then(Value::as_u64)
                .and_then(|t| u32::try_from(t).ok());
        }

        let total = upstream_total.unwrap_or(results.len() as u32);
        let has_more = page_number.saturating_mul(page_size) < total;
        Ok(SearchOutcome::new(results, has_more, Some(total)))
    }
}

fn map_http_error(e: &HubError) -> MarketSearchError {
    match e {
        HubError::Http { status, .. } => MarketSearchError::new(
            "skillhub",
            format!("SkillHub search returned HTTP {status}"),
        ),
        other => MarketSearchError::new("skillhub", format!("SkillHub search failed: {other}")),
    }
}

fn to_market_result(item: &serde_json::Map<String, Value>, lang: &str) -> Option<MarketResult> {
    let slug = str_field(item.get("slug"));
    if slug.is_empty() {
        return None;
    }

    let name = {
        let n = str_field(item.get("name"));
        if n.is_empty() {
            slug.clone()
        } else {
            n
        }
    };

    let description = if lang.to_lowercase().starts_with("zh") {
        opt_str(item.get("description_zh")).or_else(|| opt_str(item.get("description")))
    } else {
        opt_str(item.get("description")).or_else(|| opt_str(item.get("description_zh")))
    };

    let owner_name = str_field(item.get("ownerName"));
    let source_url = format!("{DEFAULT_BASE_URL}/{owner_name}/{slug}");

    let mut stats: std::collections::BTreeMap<String, StatValue> =
        std::collections::BTreeMap::new();
    if let Some(downloads) = opt_int(item.get("downloads")) {
        stats.insert("downloads".to_string(), StatValue::Int(downloads));
    }
    if let Some(installs) = opt_int(item.get("installs")) {
        stats.insert("installs".to_string(), StatValue::Int(installs));
    }
    if let Some(stars) = opt_int(item.get("stars")) {
        stats.insert("stars".to_string(), StatValue::Int(stars));
    }
    if let Some(category) = opt_str(item.get("category")) {
        stats.insert("category".to_string(), StatValue::Str(category));
    }

    Some(MarketResult {
        source: "skillhub".to_string(),
        slug,
        name,
        description,
        source_url,
        version: opt_str(item.get("version")),
        author: if owner_name.is_empty() {
            None
        } else {
            Some(owner_name)
        },
        icon_url: opt_str(item.get("iconUrl")),
        stats: if stats.is_empty() { None } else { Some(stats) },
    })
}

fn str_field(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn opt_str(value: Option<&Value>) -> Option<String> {
    let s = str_field(value);
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn opt_int(value: Option<&Value>) -> Option<i64> {
    match value {
        Some(Value::Number(n)) => n.as_i64(),
        Some(Value::String(s)) => {
            let t = s.trim();
            if !t.is_empty()
                && t.trim_start_matches('-')
                    .chars()
                    .all(|c| c.is_ascii_digit())
            {
                t.parse::<i64>().ok()
            } else {
                None
            }
        }
        _ => None,
    }
}

fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for &b in input.as_bytes() {
        let unreserved = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~');
        if unreserved {
            out.push(b as char);
        } else {
            out.push('%');
            out.push_str(&format!("{b:02X}"));
        }
    }
    out
}
