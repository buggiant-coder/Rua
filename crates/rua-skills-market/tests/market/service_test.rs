use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use rua_skills_market::provider::{MarketProvider, SearchOutcome};
use rua_skills_market::registry::ProviderRegistry;
use rua_skills_market::schema::{MarketResult, MarketSearchError};
use rua_skills_market::service::{list_providers, search_market, MAX_LIMIT};

struct MockProvider {
    key: String,
    label: String,
    available: bool,
    reason: Option<String>,

    fail_with: Option<String>,

    count: usize,

    seen_limit: Arc<std::sync::Mutex<Option<u32>>>,
}

impl MockProvider {
    fn ok(key: &str, count: usize) -> Self {
        Self {
            key: key.to_string(),
            label: key.to_uppercase(),
            available: true,
            reason: None,
            fail_with: None,
            count,
            seen_limit: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    fn failing(key: &str, msg: &str) -> Self {
        Self {
            key: key.to_string(),
            label: key.to_uppercase(),
            available: true,
            reason: None,
            fail_with: Some(msg.to_string()),
            count: 0,
            seen_limit: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    fn unavailable(key: &str, reason: &str) -> Self {
        Self {
            key: key.to_string(),
            label: key.to_uppercase(),
            available: false,
            reason: Some(reason.to_string()),
            fail_with: None,
            count: 0,
            seen_limit: Arc::new(std::sync::Mutex::new(None)),
        }
    }
}

#[async_trait]
impl MarketProvider for MockProvider {
    fn key(&self) -> &str {
        &self.key
    }
    fn label(&self) -> &str {
        &self.label
    }
    fn available(&self) -> (bool, Option<String>) {
        (self.available, self.reason.clone())
    }
    async fn search(
        &self,
        _query: &str,
        limit: u32,
        _page: u32,
    ) -> Result<SearchOutcome, MarketSearchError> {
        *self.seen_limit.lock().unwrap() = Some(limit);
        if let Some(msg) = &self.fail_with {
            return Err(MarketSearchError::new(&self.key, msg));
        }
        let results: Vec<MarketResult> = (0..self.count)
            .map(|i| MarketResult {
                source: self.key.clone(),
                slug: format!("{}-{i}", self.key),
                name: format!("{} {i}", self.key),
                description: None,
                source_url: format!("https://example.com/{}/{i}", self.key),
                version: None,
                author: None,
                icon_url: None,
                stats: None,
            })
            .collect();
        Ok(SearchOutcome::new(results, false, Some(self.count as u32)))
    }
}

fn pages(entries: &[(&str, u32)]) -> BTreeMap<String, u32> {
    entries.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

#[tokio::test]
async fn concats_results_from_multiple_providers_no_dedup() {
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(MockProvider::ok("a", 2)));
    registry.register(Arc::new(MockProvider::ok("b", 3)));

    let resp = search_market(&registry, "q", &pages(&[("a", 1), ("b", 1)]), 10, "en").await;

    assert_eq!(resp.results.len(), 5, "2 + 3 concatenated, no dedup");
    assert!(resp.errors.is_empty());
    assert_eq!(resp.by_provider.len(), 2);
    assert_eq!(resp.by_provider["a"].total, 2);
    assert_eq!(resp.by_provider["b"].total, 3);
}

#[tokio::test]
async fn single_provider_failure_degrades_to_errors() {
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(MockProvider::ok("good", 2)));
    registry.register(Arc::new(MockProvider::failing("bad", "boom")));

    let resp = search_market(&registry, "q", &pages(&[("good", 1), ("bad", 1)]), 10, "en").await;

    assert_eq!(resp.results.len(), 2);
    assert_eq!(resp.errors.len(), 1);
    assert_eq!(resp.errors[0].provider, "bad");
    assert_eq!(resp.errors[0].message, "boom");

    assert!(resp.by_provider.contains_key("good"));
    assert!(!resp.by_provider.contains_key("bad"));
}

#[tokio::test]
async fn unavailable_provider_folds_into_errors_without_calling_search() {
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(MockProvider::unavailable("down", "no creds")));

    let resp = search_market(&registry, "q", &pages(&[("down", 1)]), 10, "en").await;

    assert!(resp.results.is_empty());
    assert_eq!(resp.errors.len(), 1);
    assert_eq!(resp.errors[0].provider, "down");
    assert_eq!(resp.errors[0].message, "no creds");
}

#[tokio::test]
async fn unknown_provider_key_is_skipped() {
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(MockProvider::ok("a", 1)));

    let resp = search_market(&registry, "q", &pages(&[("a", 1), ("ghost", 1)]), 10, "en").await;

    assert_eq!(resp.results.len(), 1);
    assert!(resp.errors.is_empty());
    assert_eq!(resp.by_provider.len(), 1);
}

#[tokio::test]
async fn limit_is_capped_at_max() {
    let provider = Arc::new(MockProvider::ok("a", 1));
    let seen = provider.seen_limit.clone();
    let mut registry = ProviderRegistry::new();
    registry.register(provider);

    let _ = search_market(&registry, "q", &pages(&[("a", 1)]), 9999, "en").await;

    assert_eq!(*seen.lock().unwrap(), Some(MAX_LIMIT));
}

#[tokio::test]
async fn limit_floor_is_one() {
    let provider = Arc::new(MockProvider::ok("a", 1));
    let seen = provider.seen_limit.clone();
    let mut registry = ProviderRegistry::new();
    registry.register(provider);

    let _ = search_market(&registry, "q", &pages(&[("a", 1)]), 0, "en").await;

    assert_eq!(*seen.lock().unwrap(), Some(1));
}

#[tokio::test]
async fn list_providers_reports_availability() {
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(MockProvider::ok("up", 0)));
    registry.register(Arc::new(MockProvider::unavailable("down", "missing creds")));

    let infos = list_providers(&registry);
    assert_eq!(infos.len(), 2);

    assert_eq!(infos[0].key, "up");
    assert!(infos[0].available);
    assert_eq!(infos[0].reason, None);
    assert_eq!(infos[1].key, "down");
    assert!(!infos[1].available);
    assert_eq!(infos[1].reason.as_deref(), Some("missing creds"));
}
