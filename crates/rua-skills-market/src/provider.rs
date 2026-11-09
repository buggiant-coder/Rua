use async_trait::async_trait;

use crate::schema::{MarketResult, MarketSearchError};

pub const MARKET_SEARCH_TIMEOUT_S: f64 = 15.0;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchOutcome {
    pub results: Vec<MarketResult>,
    pub has_more: bool,
    pub total: Option<u32>,
}

impl SearchOutcome {
    pub fn new(results: Vec<MarketResult>, has_more: bool, total: Option<u32>) -> Self {
        Self {
            results,
            has_more,
            total,
        }
    }
}

#[async_trait]
pub trait MarketProvider: Send + Sync {
    fn key(&self) -> &str;

    fn label(&self) -> &str;

    fn available(&self) -> (bool, Option<String>);

    async fn search(
        &self,
        query: &str,
        limit: u32,
        page: u32,
    ) -> Result<SearchOutcome, MarketSearchError>;
}
