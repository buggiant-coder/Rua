use std::collections::BTreeMap;

use futures::future::join_all;
use serde::{Deserialize, Serialize};

use crate::provider::MarketProvider;
use crate::registry::ProviderRegistry;
use crate::schema::{MarketResult, MarketSearchError, ProviderInfo};

pub const MAX_LIMIT: u32 = 50;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ByProvider {
    pub has_more: bool,
    pub total: u32,
}

#[derive(Debug, Clone, Default)]
pub struct MarketSearchResponse {
    pub results: Vec<MarketResult>,
    pub errors: Vec<MarketSearchError>,
    pub by_provider: BTreeMap<String, ByProvider>,
}

pub fn list_providers(registry: &ProviderRegistry) -> Vec<ProviderInfo> {
    registry
        .iter()
        .map(|p| {
            let (available, reason) = p.available();
            ProviderInfo {
                key: p.key().to_string(),
                label: p.label().to_string(),
                available,
                reason,
            }
        })
        .collect()
}

pub async fn search_market(
    registry: &ProviderRegistry,
    query: &str,
    provider_pages: &BTreeMap<String, u32>,
    limit: u32,
    lang: &str,
) -> MarketSearchResponse {
    let _ = lang;
    let capped_limit = limit.clamp(1, MAX_LIMIT);

    let selected: Vec<(&str, u32)> = provider_pages
        .iter()
        .filter(|(key, _)| registry.contains(key))
        .map(|(key, page)| (key.as_str(), (*page).max(1)))
        .collect();

    let futures = selected.iter().map(|(key, page)| {
        let provider = registry.get(key).expect("contains() checked above");
        run_one(provider.as_ref(), query, capped_limit, *page)
    });
    let outcomes = join_all(futures).await;

    let mut response = MarketSearchResponse::default();
    for ((key, _), outcome) in selected.iter().zip(outcomes) {
        match outcome {
            Ok(out) => {
                response.results.extend(out.results);
                let total = out.total.unwrap_or(0);
                response.by_provider.insert(
                    key.to_string(),
                    ByProvider {
                        has_more: out.has_more,
                        total,
                    },
                );
            }
            Err(err) => response.errors.push(err),
        }
    }
    response
}

async fn run_one(
    provider: &dyn MarketProvider,
    query: &str,
    limit: u32,
    page: u32,
) -> Result<crate::provider::SearchOutcome, MarketSearchError> {
    let (available, reason) = provider.available();
    if !available {
        return Err(MarketSearchError::new(
            provider.key(),
            reason.unwrap_or_else(|| "provider unavailable".to_string()),
        ));
    }
    provider.search(query, limit, page).await.inspect_err(|e| {
        tracing::warn!(
            provider = provider.key(),
            query = query,
            error = %e.message,
            "market provider search failed",
        );
    })
}
