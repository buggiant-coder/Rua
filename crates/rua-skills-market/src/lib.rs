pub mod provider;
pub mod providers;
pub mod registry;
pub mod schema;
pub mod service;

pub use provider::{MarketProvider, SearchOutcome, MARKET_SEARCH_TIMEOUT_S};
pub use providers::{AliyunProvider, ClawHubProvider};
pub use registry::{build_default_registry, ProviderRegistry};
pub use schema::{MarketResult, MarketSearchError, ProviderInfo, StatValue};
pub use service::{list_providers, search_market, ByProvider, MarketSearchResponse, MAX_LIMIT};
