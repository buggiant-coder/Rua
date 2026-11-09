use std::sync::Arc;

use async_trait::async_trait;
use rua_skills_hub::{http_get_bytes_with_retry, HubClient, HubError};
use serde_json::Value;

use crate::provider::{MarketProvider, SearchOutcome};
use crate::schema::{MarketResult, MarketSearchError, StatValue};

const DEFAULT_BASE_URL: &str = "https://www.modelscope.cn";
const SEARCH_PATH: &str = "/openapi/v1/skills";

const MAX_PAGE_SIZE: u32 = 100;

const DETAIL_HOST: &str = "https://modelscope.cn";

pub struct ModelScopeProvider {
    client: Arc<HubClient>,
    key: String,
    label: String,
    base_url: String,
    lang: String,
}

impl ModelScopeProvider {
    pub fn new(client: Arc<HubClient>) -> Self {
        Self::with_options(client, DEFAULT_BASE_URL.to_string(), "en".to_string())
    }

    pub fn with_options(client: Arc<HubClient>, base_url: String, lang: String) -> Self {
        let trimmed = base_url.trim_end_matches('/').to_string();
        Self {
            client,
            key: "modelscope".to_string(),
            label: "ModelScope".to_string(),
            base_url: trimmed,
            lang,
        }
    }
}

#[async_trait]
impl MarketProvider for ModelScopeProvider {
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
        let page_size = limit.clamp(1, MAX_PAGE_SIZE);
        let page_number = page.max(1);

        let mut url = format!(
            "{}{}?page_size={}&page_number={}",
            self.base_url, SEARCH_PATH, page_size, page_number,
        );
        let needle = query.trim();
        if !needle.is_empty() {
            url.push_str("&search=");
            url.push_str(&percent_encode(needle, ""));
        }

        let bytes = http_get_bytes_with_retry(&self.client, &url, None)
            .await
            .map_err(|e| map_http_error(&e))?;

        let body: Value = serde_json::from_slice(&bytes).map_err(|_| {
            MarketSearchError::new("modelscope", "ModelScope search failed: non-JSON response")
        })?;

        let obj = body.as_object().ok_or_else(|| {
            MarketSearchError::new("modelscope", "ModelScope search failed: non-JSON response")
        })?;

        let success = obj.get("success").and_then(Value::as_bool).unwrap_or(true);
        if !success {
            let message = obj
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            return Err(MarketSearchError::new(
                "modelscope",
                format!("ModelScope search failed: {message}"),
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
            "modelscope",
            format!("ModelScope search returned HTTP {status}"),
        ),
        other => MarketSearchError::new("modelscope", format!("ModelScope search failed: {other}")),
    }
}

fn to_market_result(item: &serde_json::Map<String, Value>, lang: &str) -> Option<MarketResult> {
    let skill_id = str_field(item.get("id"));
    if skill_id.is_empty() {
        return None;
    }
    let display_name = {
        let n = str_field(item.get("display_name"));
        if n.is_empty() {
            skill_id.clone()
        } else {
            n
        }
    };
    let description =
        localized(item, "description", lang).or_else(|| opt_str(item.get("description")));

    let mut developer = str_field(item.get("developer"));
    let owner = str_field(item.get("owner"));
    if developer.is_empty() && skill_id.starts_with('@') && skill_id.contains('/') {
        developer = skill_id
            .split_once('/')
            .map(|(head, _)| head.trim_start_matches('@').to_string())
            .unwrap_or_default();
    } else if developer.is_empty() {
        developer = owner;
    }

    let quoted_id = percent_encode(&skill_id, "@/");
    let source_url = format!("{DETAIL_HOST}/skills/{quoted_id}");

    let mut stats: std::collections::BTreeMap<String, StatValue> =
        std::collections::BTreeMap::new();
    if let Some(downloads) = opt_int(item.get("downloads")) {
        stats.insert("downloads".to_string(), StatValue::Int(downloads));
    }
    if let Some(views) = opt_int(item.get("view_count")) {
        stats.insert("views".to_string(), StatValue::Int(views));
    }
    if let Some(category) =
        localized(item, "category", lang).or_else(|| opt_str(item.get("category")))
    {
        stats.insert("category".to_string(), StatValue::Str(category));
    }

    Some(MarketResult {
        source: "modelscope".to_string(),
        slug: skill_id,
        name: display_name,
        description,
        source_url,
        version: opt_str(item.get("version")),
        author: if developer.is_empty() {
            None
        } else {
            Some(developer)
        },
        icon_url: opt_str(item.get("logo_url")),
        stats: if stats.is_empty() { None } else { Some(stats) },
    })
}

fn localized(item: &serde_json::Map<String, Value>, field: &str, lang: &str) -> Option<String> {
    let locales = item.get("locales")?.as_object()?;
    let primary = if lang.to_lowercase().starts_with("zh") {
        "zh"
    } else {
        "en"
    };
    let fallback = if primary == "zh" { "en" } else { "zh" };
    for code in [primary, fallback] {
        if let Some(entry) = locales.get(code).and_then(Value::as_object) {
            if let Some(text) = opt_str(entry.get(field)) {
                return Some(text);
            }
        }
    }
    None
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

fn percent_encode(input: &str, extra_safe: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for &b in input.as_bytes() {
        let unreserved = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~');
        if unreserved || (b < 0x80 && extra_safe.as_bytes().contains(&b)) {
            out.push(b as char);
        } else {
            out.push('%');
            out.push_str(&format!("{b:02X}"));
        }
    }
    out
}
