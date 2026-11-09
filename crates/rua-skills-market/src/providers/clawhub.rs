use std::sync::Arc;

use async_trait::async_trait;
use rua_skills_hub::{search_hub_skills, HubClient, HubSkillResult};

use crate::provider::{MarketProvider, SearchOutcome};
use crate::schema::{MarketResult, MarketSearchError};

const HOMEPAGE: &str = "https://clawhub.ai";

const OVERFETCH_LIMIT: u32 = 500;

pub struct ClawHubProvider {
    client: Arc<HubClient>,
    key: String,
    label: String,
}

impl ClawHubProvider {
    pub fn new(client: Arc<HubClient>) -> Self {
        Self {
            client,
            key: "clawhub".to_string(),
            label: "ClawHub".to_string(),
        }
    }
}

fn to_market_result(item: HubSkillResult) -> Option<MarketResult> {
    let slug = item.slug.trim().to_string();
    if slug.is_empty() {
        return None;
    }
    let source_url = if item.source_url.is_empty() {
        format!("{HOMEPAGE}/{slug}")
    } else {
        item.source_url
    };
    let name = if item.name.is_empty() {
        slug.clone()
    } else {
        item.name
    };
    Some(MarketResult {
        source: "clawhub".to_string(),
        slug,
        name,
        description: opt(item.description),
        source_url,
        version: opt(item.version),
        author: opt(item.author),
        icon_url: opt(item.icon_url),
        stats: None,
    })
}

fn opt(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

#[async_trait]
impl MarketProvider for ClawHubProvider {
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
        let raw = search_hub_skills(&self.client, query, OVERFETCH_LIMIT)
            .await
            .map_err(|e| MarketSearchError::new("clawhub", e.to_string()))?;

        let all: Vec<MarketResult> = raw.into_iter().filter_map(to_market_result).collect();
        let total = all.len();

        let page = page.max(1);
        let start = ((page - 1) as usize).saturating_mul(limit as usize);
        let end = start.saturating_add(limit as usize);
        let has_more = end < total;
        let slice: Vec<MarketResult> = all.into_iter().skip(start).take(limit as usize).collect();

        Ok(SearchOutcome::new(slice, has_more, Some(total as u32)))
    }
}
