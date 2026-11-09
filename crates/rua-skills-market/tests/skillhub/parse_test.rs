use std::sync::Arc;

use rua_skills_hub::HubClient;
use rua_skills_market::provider::MarketProvider;
use rua_skills_market::providers::SkillHubProvider;
use rua_skills_market::schema::StatValue;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const FIXTURE: &str = include_str!("../fixtures/skillhub_search.json");

async fn mount_fixture(server: &MockServer, body: Value) {
    Mock::given(method("GET"))
        .and(path("/api/skills"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

#[tokio::test]
async fn maps_fields_en() {
    let server = MockServer::start().await;
    let body: Value = serde_json::from_str(FIXTURE).expect("fixture parses");
    mount_fixture(&server, body).await;

    let provider = SkillHubProvider::with_options(
        Arc::new(HubClient::new().expect("client")),
        server.uri(),
        "en".to_string(),
    );
    let outcome = provider.search("ai", 20, 1).await.expect("search ok");

    assert_eq!(outcome.total, Some(27413));

    assert_eq!(outcome.results.len(), 2);

    let first = &outcome.results[0];
    assert_eq!(first.source, "skillhub");
    assert_eq!(first.slug, "humanize-ai-text");
    assert_eq!(first.name, "Humanize AI text");

    assert_eq!(
        first.description.as_deref(),
        Some("Transform AI-generated content into natural human writing")
    );
    assert_eq!(first.version.as_deref(), Some("1.0.1"));
    assert_eq!(first.author.as_deref(), Some("moltbro"));
    assert_eq!(
        first.icon_url.as_deref(),
        Some("https://api.skillhub.cn/icons/humanize.png")
    );
    assert_eq!(
        first.source_url,
        "https://api.skillhub.cn/moltbro/humanize-ai-text"
    );

    let stats = first.stats.as_ref().expect("stats present");
    assert_eq!(stats.get("downloads"), Some(&StatValue::Int(69793)));
    assert_eq!(stats.get("installs"), Some(&StatValue::Int(11876)));
    assert_eq!(stats.get("stars"), Some(&StatValue::Int(206)));
    assert_eq!(
        stats.get("category"),
        Some(&StatValue::Str("content-creation".to_string()))
    );

    let second = &outcome.results[1];
    assert_eq!(second.slug, "code-reviewer");
    assert_eq!(second.name, "Code Reviewer");
    assert_eq!(second.icon_url, None);
}

#[tokio::test]
async fn maps_fields_zh() {
    let server = MockServer::start().await;
    let body: Value = serde_json::from_str(FIXTURE).expect("fixture parses");
    mount_fixture(&server, body).await;

    let provider = SkillHubProvider::with_options(
        Arc::new(HubClient::new().expect("client")),
        server.uri(),
        "zh".to_string(),
    );
    let outcome = provider.search("ai", 20, 1).await.expect("search ok");

    let first = &outcome.results[0];
    assert_eq!(
        first.description.as_deref(),
        Some("将AI生成的内容转换为自然的人类写作")
    );
}

#[tokio::test]
async fn pagination_has_more() {
    let server = MockServer::start().await;
    let body: Value = serde_json::from_str(FIXTURE).expect("fixture parses");
    mount_fixture(&server, body.clone()).await;

    let provider = SkillHubProvider::with_options(
        Arc::new(HubClient::new().expect("client")),
        server.uri(),
        "en".to_string(),
    );

    let outcome = provider.search("ai", 20, 1).await.expect("search ok");
    assert!(outcome.has_more);
}

#[tokio::test]
async fn pagination_no_more() {
    let server = MockServer::start().await;
    let body = json!({
        "code": 0,
        "data": { "skills": [], "total": 5 },
        "message": "success"
    });
    mount_fixture(&server, body).await;

    let provider = SkillHubProvider::with_options(
        Arc::new(HubClient::new().expect("client")),
        server.uri(),
        "en".to_string(),
    );

    let outcome = provider.search("ai", 10, 1).await.expect("search ok");
    assert!(!outcome.has_more);
}

#[tokio::test]
async fn upstream_error_code_returns_error() {
    let server = MockServer::start().await;
    let body = json!({ "code": 1001, "message": "rate limited", "data": null });
    mount_fixture(&server, body).await;

    let provider = SkillHubProvider::with_options(
        Arc::new(HubClient::new().expect("client")),
        server.uri(),
        "en".to_string(),
    );
    let err = provider.search("x", 10, 1).await.expect_err("should fail");
    assert_eq!(err.provider, "skillhub");
    assert!(err.message.contains("rate limited"), "got: {}", err.message);
}
