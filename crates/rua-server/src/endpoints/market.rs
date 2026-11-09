use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::Extension;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rua_skills_hub::HubClient;
use rua_skills_market::{
    build_default_registry, list_providers, search_market, ByProvider, MarketResult,
    MarketSearchError, ProviderRegistry,
};
use serde::{Deserialize, Serialize};

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    let client = Arc::new(HubClient::new().expect("market: build HubClient"));
    let registry: Arc<ProviderRegistry> = Arc::new(build_default_registry(client));
    Router::new()
        .route("/market/providers", get(get_market_providers))
        .route("/market/search", post(market_search))
        .layer(Extension(registry))
}

async fn get_market_providers(Extension(registry): Extension<Arc<ProviderRegistry>>) -> Response {
    Json(list_providers(&registry)).into_response()
}

#[derive(Debug, Deserialize)]
struct MarketSearchRequest {
    #[serde(default)]
    query: String,
    #[serde(default)]
    provider_pages: BTreeMap<String, u32>,
    #[serde(default = "default_limit")]
    limit: u32,
    #[serde(default = "default_lang")]
    lang: String,
}

fn default_limit() -> u32 {
    10
}

fn default_lang() -> String {
    "en".to_string()
}

#[derive(Debug, Serialize)]
struct MarketSearchResponse {
    results: Vec<MarketResult>,
    errors: Vec<MarketSearchError>,
    by_provider: BTreeMap<String, ByProvider>,
}

async fn market_search(
    Extension(registry): Extension<Arc<ProviderRegistry>>,
    Json(body): Json<MarketSearchRequest>,
) -> Response {
    let mut unknown: Vec<String> = body
        .provider_pages
        .keys()
        .filter(|k| !registry.contains(k))
        .cloned()
        .collect();
    if !unknown.is_empty() {
        unknown.sort();
        let detail = format!("unknown providers: {unknown:?}");
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "detail": detail })),
        )
            .into_response();
    }

    let resp = search_market(
        &registry,
        &body.query,
        &body.provider_pages,
        body.limit,
        &body.lang,
    )
    .await;

    Json(MarketSearchResponse {
        results: resp.results,
        errors: resp.errors,
        by_provider: resp.by_provider,
    })
    .into_response()
}
