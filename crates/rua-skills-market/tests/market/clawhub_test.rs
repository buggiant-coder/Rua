use std::sync::Arc;

use rua_skills_hub::HubClient;
use rua_skills_market::provider::MarketProvider;
use rua_skills_market::providers::ClawHubProvider;
use serde_json::json;
use serial_test::serial;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn restore_env() {
    std::env::remove_var("RUA_SKILLS_HUB_BASE_URL");
}

#[tokio::test]
#[serial]
async fn parses_items_and_maps_fields() {
    let server = MockServer::start().await;
    let body = json!({
        "items": [
            {
                "slug": "alpha",
                "name": "Alpha Skill",
                "description": "first",
                "version": "1.2.0",
                "url": "https://clawhub.ai/alpha"
            },
            {
                "slug": "beta",
                "name": "Beta Skill"
            }
        ]
    });
    Mock::given(method("GET"))
        .and(path("/api/v1/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;
    std::env::set_var("RUA_SKILLS_HUB_BASE_URL", server.uri());

    let provider = ClawHubProvider::new(Arc::new(HubClient::new().expect("client")));
    let outcome = provider.search("anything", 10, 1).await.expect("search ok");
    restore_env();

    assert_eq!(outcome.total, Some(2));
    assert!(!outcome.has_more);
    assert_eq!(outcome.results.len(), 2);

    let alpha = &outcome.results[0];
    assert_eq!(alpha.source, "clawhub");
    assert_eq!(alpha.slug, "alpha");
    assert_eq!(alpha.name, "Alpha Skill");
    assert_eq!(alpha.description.as_deref(), Some("first"));
    assert_eq!(alpha.version.as_deref(), Some("1.2.0"));
    assert_eq!(alpha.source_url, "https://clawhub.ai/alpha");

    let beta = &outcome.results[1];
    assert_eq!(beta.slug, "beta");
    assert_eq!(beta.source_url, "https://clawhub.ai/beta");
    assert_eq!(beta.description, None);
    assert_eq!(beta.version, None);
}

#[tokio::test]
#[serial]
async fn over_fetch_then_slice_page_two_has_more() {
    let server = MockServer::start().await;

    let items: Vec<_> = (0..5)
        .map(|i| json!({ "slug": format!("s{i}"), "name": format!("S{i}") }))
        .collect();
    Mock::given(method("GET"))
        .and(path("/api/v1/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "items": items })))
        .mount(&server)
        .await;
    std::env::set_var("RUA_SKILLS_HUB_BASE_URL", server.uri());

    let provider = ClawHubProvider::new(Arc::new(HubClient::new().expect("client")));
    let outcome = provider.search("q", 2, 2).await.expect("search ok");
    restore_env();

    assert_eq!(outcome.total, Some(5));
    assert!(outcome.has_more, "page 2 of 2 leaves item index 4");
    assert_eq!(outcome.results.len(), 2);
    assert_eq!(outcome.results[0].slug, "s2");
    assert_eq!(outcome.results[1].slug, "s3");
}

#[tokio::test]
#[serial]
async fn last_page_reports_no_more() {
    let server = MockServer::start().await;
    let items: Vec<_> = (0..3)
        .map(|i| json!({ "slug": format!("s{i}"), "name": format!("S{i}") }))
        .collect();
    Mock::given(method("GET"))
        .and(path("/api/v1/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "items": items })))
        .mount(&server)
        .await;
    std::env::set_var("RUA_SKILLS_HUB_BASE_URL", server.uri());

    let provider = ClawHubProvider::new(Arc::new(HubClient::new().expect("client")));

    let outcome = provider.search("q", 2, 2).await.expect("search ok");
    restore_env();

    assert_eq!(outcome.total, Some(3));
    assert!(!outcome.has_more);
    assert_eq!(outcome.results.len(), 1);
    assert_eq!(outcome.results[0].slug, "s2");
}

#[tokio::test]
async fn clawhub_provider_is_available() {
    let provider = ClawHubProvider::new(Arc::new(HubClient::new().expect("client")));
    let (available, reason) = provider.available();
    assert!(available);
    assert_eq!(reason, None);
    assert_eq!(provider.key(), "clawhub");
    assert_eq!(provider.label(), "ClawHub");
}
