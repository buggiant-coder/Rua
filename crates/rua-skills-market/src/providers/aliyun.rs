use std::collections::BTreeMap;
use std::env;

use async_trait::async_trait;
use serde_json::Value;
use tracing::warn;

use super::aliyun_sign::{self, SignInput};
use crate::provider::{MarketProvider, SearchOutcome, MARKET_SEARCH_TIMEOUT_S};
use crate::schema::{MarketResult, MarketSearchError, StatValue};

const DEFAULT_ENDPOINT: &str = "agentexplorer.aliyuncs.com";
const API_VERSION: &str = "2026-03-17";
const DETAIL_BASE: &str = "https://api.aliyun.com/agentexplorer/skills";

const UPSTREAM_PAGE_SIZE: u32 = 100;
const MAX_PAGE_WALK: u32 = 50;

const ENV_AK_ID: &str = "ALIBABA_CLOUD_ACCESS_KEY_ID";
const ENV_AK_SECRET: &str = "ALIBABA_CLOUD_ACCESS_KEY_SECRET";
const ENV_ENDPOINT: &str = "ALIYUN_AGENTEXPLORER_ENDPOINT";

pub struct AliyunProvider {
    key: String,
    label: String,
    client: reqwest::Client,
}

impl AliyunProvider {
    pub fn new() -> Self {
        let client = rua_http::build_client_or_default(
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs_f64(MARKET_SEARCH_TIMEOUT_S)),
            "rua-skills-market",
        );
        Self {
            key: "aliyun".to_string(),
            label: "Aliyun".to_string(),
            client,
        }
    }

    #[cfg(test)]
    pub fn with_client(client: reqwest::Client) -> Self {
        Self {
            key: "aliyun".to_string(),
            label: "Aliyun".to_string(),
            client,
        }
    }
}

impl Default for AliyunProvider {
    fn default() -> Self {
        Self::new()
    }
}

fn endpoint() -> (String, String) {
    let raw = env::var(ENV_ENDPOINT).unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string());
    if raw.contains("://") {
        let host = raw
            .split("://")
            .nth(1)
            .unwrap_or(&raw)
            .trim_end_matches('/')
            .to_string();
        (raw.trim_end_matches('/').to_string(), host)
    } else {
        let host = raw.trim_end_matches('/').to_string();
        (format!("https://{host}"), host)
    }
}

fn env_or_store(key: &str) -> Option<String> {
    if let Ok(val) = env::var(key) {
        if !val.is_empty() {
            return Some(val);
        }
    }
    let store = rua_config::EnvStore::new(rua_config::dirs::resolve_secret_dir());
    store.get(key).ok().flatten().filter(|s| !s.is_empty())
}

fn credentials() -> Option<(String, String)> {
    let ak_id = env_or_store(ENV_AK_ID)?;
    let ak_secret = env_or_store(ENV_AK_SECRET)?;
    Some((ak_id, ak_secret))
}

#[async_trait]
impl MarketProvider for AliyunProvider {
    fn key(&self) -> &str {
        &self.key
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn available(&self) -> (bool, Option<String>) {
        let mut missing = Vec::new();
        if env_or_store(ENV_AK_ID).is_none() {
            missing.push(ENV_AK_ID);
        }
        if env_or_store(ENV_AK_SECRET).is_none() {
            missing.push(ENV_AK_SECRET);
        }
        if missing.is_empty() {
            (true, None)
        } else {
            (
                false,
                Some(format!(
                    "missing env vars: {} (set Aliyun AK/SK to enable search)",
                    missing.join(", "),
                )),
            )
        }
    }

    async fn search(
        &self,
        query: &str,
        limit: u32,
        page: u32,
    ) -> Result<SearchOutcome, MarketSearchError> {
        let (ak_id, ak_secret) = credentials().ok_or_else(|| {
            MarketSearchError::new(
                "aliyun",
                "Aliyun credentials not configured (ALIBABA_CLOUD_ACCESS_KEY_ID / _SECRET)",
            )
        })?;

        let target_page = page.max(1);
        if target_page > MAX_PAGE_WALK {
            return Ok(SearchOutcome::new(vec![], false, None));
        }

        let max_results = limit.clamp(1, UPSTREAM_PAGE_SIZE);
        let (base_url, host) = endpoint();
        let mut next_token: Option<String> = None;

        for current_page in 1..=target_page {
            let mut params: Vec<(&str, String)> = vec![("maxResults", max_results.to_string())];
            let needle = query.trim();
            if !needle.is_empty() {
                params.push(("keyword", needle.to_string()));
            }
            if let Some(ref token) = next_token {
                params.push(("nextToken", token.clone()));
            }

            let resp_body =
                call_search_skills(&self.client, &base_url, &host, &ak_id, &ak_secret, &params)
                    .await?;

            let items = extract_skill_items(&resp_body);
            let token = opt_str_from_value(resp_body.get("nextToken"));

            if current_page == target_page {
                let results: Vec<MarketResult> = items
                    .iter()
                    .filter_map(|item| to_market_result(item))
                    .collect();
                let total = resp_body
                    .get("totalCount")
                    .and_then(Value::as_u64)
                    .and_then(|t| u32::try_from(t).ok());
                let has_more = token.is_some();
                return Ok(SearchOutcome::new(results, has_more, total));
            }

            match token {
                Some(t) => next_token = Some(t),
                None => return Ok(SearchOutcome::new(vec![], false, None)),
            }
        }

        Ok(SearchOutcome::new(vec![], false, None))
    }
}

async fn call_search_skills(
    client: &reqwest::Client,
    base_url: &str,
    host: &str,
    ak_id: &str,
    ak_secret: &str,
    params: &[(&str, String)],
) -> Result<Value, MarketSearchError> {
    let date = utc_iso8601();
    let nonce = uuid::Uuid::new_v4().to_string();

    let query_pairs: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();

    let sign_input = SignInput {
        method: "GET",
        uri: "/openapi/skills",
        query_params: &query_pairs,
        host,
        action: "SearchSkills",
        version: API_VERSION,
        date: &date,
        nonce: &nonce,
        body: b"",
        access_key_id: ak_id,
        access_key_secret: ak_secret,
    };
    let auth_header = aliyun_sign::sign(&sign_input);

    let query_string: String = query_pairs
        .iter()
        .map(|(k, v)| format!("{}={}", k, percent_encode(v)))
        .collect::<Vec<_>>()
        .join("&");

    let url = format!("{}/openapi/skills?{}", base_url, query_string);

    let resp = client
        .get(&url)
        .header("Host", host)
        .header("x-acs-action", "SearchSkills")
        .header("x-acs-version", API_VERSION)
        .header("x-acs-date", &date)
        .header("x-acs-signature-nonce", &nonce)
        .header("Authorization", &auth_header)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| {
            MarketSearchError::new("aliyun", format!("Aliyun SearchSkills failed: {e}"))
        })?;

    let status = resp.status();
    if !status.is_success() {
        let body_text = resp.text().await.unwrap_or_default();
        warn!(
            status = %status,
            body = %body_text,
            "Aliyun SearchSkills returned non-2xx"
        );
        return Err(MarketSearchError::new(
            "aliyun",
            format!("Aliyun SearchSkills returned HTTP {status}"),
        ));
    }

    let body: Value = resp
        .json()
        .await
        .map_err(|_| MarketSearchError::new("aliyun", "Aliyun SearchSkills: non-JSON response"))?;

    Ok(body)
}

fn extract_skill_items(body: &Value) -> Vec<&serde_json::Map<String, Value>> {
    body.get("data")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_object).collect())
        .unwrap_or_default()
}

fn to_market_result(item: &serde_json::Map<String, Value>) -> Option<MarketResult> {
    let skill_name = str_field(item.get("skillName"));
    let display_name = str_field(item.get("displayName"));
    if skill_name.is_empty() && display_name.is_empty() {
        return None;
    }

    let slug = if skill_name.is_empty() {
        display_name.clone()
    } else {
        skill_name.clone()
    };
    let name = if display_name.is_empty() {
        skill_name
    } else {
        display_name
    };

    let source_url = format!("{}/{}", DETAIL_BASE, percent_encode(&slug));

    let mut stats: BTreeMap<String, StatValue> = BTreeMap::new();
    if let Some(v) = opt_int(item.get("installCount")) {
        stats.insert("installs".to_string(), StatValue::Int(v));
    }
    if let Some(v) = opt_int(item.get("likeCount")) {
        stats.insert("likes".to_string(), StatValue::Int(v));
    }
    let category = category_label(item);
    if !category.is_empty() {
        stats.insert("category".to_string(), StatValue::Str(category));
    }
    if let Some(v) = opt_str(item.get("updatedAt")) {
        stats.insert("updated_at".to_string(), StatValue::Str(v));
    }

    Some(MarketResult {
        source: "aliyun".to_string(),
        slug,
        name,
        description: opt_str(item.get("description")),
        source_url,
        version: None,
        author: None,
        icon_url: None,
        stats: if stats.is_empty() { None } else { Some(stats) },
    })
}

fn category_label(item: &serde_json::Map<String, Value>) -> String {
    let parent = str_field(item.get("categoryName"));
    let child = str_field(item.get("subCategoryName"));
    match (parent.is_empty(), child.is_empty()) {
        (false, false) => format!("{parent} / {child}"),
        (false, true) => parent,
        (true, false) => child,
        (true, true) => String::new(),
    }
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

fn opt_str_from_value(value: Option<&Value>) -> Option<String> {
    opt_str(value)
}

fn opt_int(value: Option<&Value>) -> Option<i64> {
    match value {
        Some(Value::Number(n)) => n.as_i64(),
        Some(Value::String(s)) => {
            let t = s.trim();
            if !t.is_empty() && t.chars().all(|c| c.is_ascii_digit()) {
                t.parse::<i64>().ok()
            } else {
                None
            }
        }
        _ => None,
    }
}

fn utc_iso8601() -> String {
    use std::time::SystemTime;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let secs_per_day: u64 = 86400;
    let secs_per_hour: u64 = 3600;
    let secs_per_min: u64 = 60;

    let days = now / secs_per_day;
    let rem = now % secs_per_day;
    let hour = rem / secs_per_hour;
    let minute = (rem % secs_per_hour) / secs_per_min;
    let second = rem % secs_per_min;

    let z = days as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for &b in input.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push_str(&format!("{b:02X}"));
        }
    }
    out
}
