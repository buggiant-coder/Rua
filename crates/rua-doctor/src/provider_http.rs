use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::registry::{register_network_check, CheckContext, CheckResult, CheckStatus};

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const CACHE_TTL: Duration = Duration::from_secs(60);

static CACHE: Mutex<Option<HashMap<String, CachedProbe>>> = Mutex::new(None);

struct CachedProbe {
    result: ProviderProbeEntry,
    checked_at: Instant,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderHttpDetails {
    pub providers: Vec<ProviderProbeEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderProbeEntry {
    pub provider_id: String,
    pub url: String,
    pub reachable: bool,
    pub status_code: Option<u16>,
    pub error: Option<String>,
}

pub fn clear_cache() {
    let mut cache = CACHE.lock().unwrap();
    *cache = None;
}

fn probe_provider(
    client: &reqwest::blocking::Client,
    base_url: &str,
) -> (bool, Option<u16>, Option<String>) {
    let models_url = format!("{}/models", base_url.trim_end_matches('/'));

    match client.get(&models_url).send() {
        Ok(resp) => {
            let code = resp.status().as_u16();
            (true, Some(code), None)
        }
        Err(get_err) => match client.head(base_url).send() {
            Ok(resp) => {
                let code = resp.status().as_u16();
                (true, Some(code), None)
            }
            Err(head_err) => (
                false,
                None,
                Some(format!(
                    "GET {models_url}: {get_err}; HEAD {base_url}: {head_err}"
                )),
            ),
        },
    }
}

fn check_provider_http(ctx: &CheckContext) -> CheckResult {
    let providers_dir = match crate::find_providers_dir(&ctx.paths.secret_dir) {
        Some(d) => d,
        None => {
            return CheckResult {
                name: "provider_http".into(),
                status: CheckStatus::Pass,
                message: "no providers directory found".into(),
                details: None,
            };
        }
    };

    let read_dir = match std::fs::read_dir(&providers_dir) {
        Ok(rd) => rd,
        Err(e) => {
            return CheckResult {
                name: "provider_http".into(),
                status: CheckStatus::Warn,
                message: format!("cannot read providers dir: {e}"),
                details: None,
            };
        }
    };

    let mut entries = Vec::new();
    let mut cache = CACHE.lock().unwrap();
    let cache_map = cache.get_or_insert_with(HashMap::new);
    let now = Instant::now();

    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let provider: rua_provider::ProviderConfig = match serde_json::from_str(&content) {
            Ok(p) => p,
            Err(_) => continue,
        };

        if provider.base_url.is_empty() {
            continue;
        }

        if let Some(cached) = cache_map.get(&provider.id) {
            if now.duration_since(cached.checked_at) < CACHE_TTL {
                entries.push(cached.result.clone());
                continue;
            }
        }

        let (reachable, status_code, error) =
            match crate::http_proxy::blocking_client_for_url(&provider.base_url, PROBE_TIMEOUT) {
                Ok(client) => probe_provider(&client, &provider.base_url),
                Err(e) => (
                    false,
                    None,
                    Some(format!("failed to build HTTP client: {e}")),
                ),
            };
        let probe_entry = ProviderProbeEntry {
            provider_id: provider.id.clone(),
            url: provider.base_url.clone(),
            reachable,
            status_code,
            error,
        };

        cache_map.insert(
            provider.id,
            CachedProbe {
                result: probe_entry.clone(),
                checked_at: now,
            },
        );
        entries.push(probe_entry);
    }

    let reachable_count = entries.iter().filter(|e| e.reachable).count();
    let total = entries.len();
    let all_reachable = total == 0 || entries.iter().all(|e| e.reachable);

    let status = if all_reachable {
        CheckStatus::Pass
    } else {
        CheckStatus::Fail
    };

    CheckResult {
        name: "provider_http".into(),
        status,
        message: format!("{}/{} provider endpoints reachable", reachable_count, total),
        details: Some(serde_json::to_value(&ProviderHttpDetails { providers: entries }).unwrap()),
    }
}

inventory::submit! {
    register_network_check("provider_http", check_provider_http)
}
