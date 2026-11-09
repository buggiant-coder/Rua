use std::sync::Arc;

use rua_skills_hub::HubClient;
use rua_skills_market::provider::MarketProvider;
use rua_skills_market::providers::ModelScopeProvider;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn provider_for(server: &MockServer, body: Value) -> ModelScopeProvider {
    Mock::given(method("GET"))
        .and(path("/openapi/v1/skills"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
    ModelScopeProvider::with_options(
        Arc::new(HubClient::new().expect("client")),
        server.uri(),
        "en".to_string(),
    )
}

fn body_with_total(skill_count: usize, total: Option<u32>) -> Value {
    let skills: Vec<Value> = (0..skill_count)
        .map(|i| json!({ "id": format!("s{i}") }))
        .collect();
    let mut data = serde_json::Map::new();
    data.insert("skills".to_string(), Value::Array(skills));
    if let Some(t) = total {
        data.insert("total".to_string(), json!(t));
    }
    json!({ "success": true, "data": data })
}

#[tokio::test]
async fn has_more_when_page_times_size_below_total() {
    let server = MockServer::start().await;

    let provider = provider_for(&server, body_with_total(20, Some(42))).await;
    let outcome = provider.search("q", 20, 2).await.expect("search ok");
    assert_eq!(outcome.total, Some(42));
    assert!(outcome.has_more, "40 < 42 should leave more");
}

#[tokio::test]
async fn no_more_on_last_page() {
    let server = MockServer::start().await;

    let provider = provider_for(&server, body_with_total(2, Some(42))).await;
    let outcome = provider.search("q", 20, 3).await.expect("search ok");
    assert_eq!(outcome.total, Some(42));
    assert!(!outcome.has_more, "60 >= 42 should be the last page");
}

#[tokio::test]
async fn total_falls_back_to_result_count_when_absent() {
    let server = MockServer::start().await;

    let provider = provider_for(&server, body_with_total(3, None)).await;
    let outcome = provider.search("q", 20, 1).await.expect("search ok");
    assert_eq!(outcome.total, Some(3));
    assert!(!outcome.has_more, "20 >= 3 → no more");
    assert_eq!(outcome.results.len(), 3);
}

#[tokio::test]
async fn page_size_clamped_to_upstream_ceiling() {
    let server = MockServer::start().await;

    let provider = provider_for(&server, body_with_total(5, Some(150))).await;
    let outcome = provider.search("q", 250, 1).await.expect("search ok");
    assert!(
        outcome.has_more,
        "page_size should clamp to 100, leaving 100 < 150"
    );
}
