use std::sync::Arc;

use rua_skills_hub::HubClient;
use rua_skills_market::provider::MarketProvider;
use rua_skills_market::providers::ModelScopeProvider;
use rua_skills_market::schema::StatValue;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const FIXTURE: &str = include_str!("../fixtures/modelscope_search.json");

async fn mount_fixture(server: &MockServer, body: Value) {
    Mock::given(method("GET"))
        .and(path("/openapi/v1/skills"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

#[tokio::test]
async fn maps_fields_and_localizes_en() {
    let server = MockServer::start().await;
    let body: Value = serde_json::from_str(FIXTURE).expect("fixture parses");
    mount_fixture(&server, body).await;

    let provider = ModelScopeProvider::with_options(
        Arc::new(HubClient::new().expect("client")),
        server.uri(),
        "en".to_string(),
    );
    let outcome = provider.search("data", 20, 1).await.expect("search ok");

    assert_eq!(outcome.total, Some(42));
    assert_eq!(outcome.results.len(), 2);

    let first = &outcome.results[0];
    assert_eq!(first.source, "modelscope");
    assert_eq!(first.slug, "@acme/data-wrangler");
    assert_eq!(first.name, "Data Wrangler");

    assert_eq!(
        first.description.as_deref(),
        Some("Wrangle messy data fast")
    );
    assert_eq!(first.version.as_deref(), Some("2.1.0"));

    assert_eq!(first.author.as_deref(), Some("acme"));
    assert_eq!(
        first.icon_url.as_deref(),
        Some("https://modelscope.cn/logos/data-wrangler.png")
    );

    assert_eq!(
        first.source_url,
        "https://modelscope.cn/skills/@acme/data-wrangler"
    );

    let stats = first.stats.as_ref().expect("stats present");
    assert_eq!(stats.get("downloads"), Some(&StatValue::Int(1280)));

    assert_eq!(stats.get("views"), Some(&StatValue::Int(5400)));
    assert_eq!(
        stats.get("category"),
        Some(&StatValue::Str("Data Tools".to_string()))
    );

    let second = &outcome.results[1];
    assert_eq!(second.slug, "solo-skill");
    assert_eq!(second.name, "solo-skill");
    assert_eq!(second.author.as_deref(), Some("lone-dev"));
    assert_eq!(second.description, None);
    assert_eq!(second.source_url, "https://modelscope.cn/skills/solo-skill");
}

#[tokio::test]
async fn localizes_zh_description_and_category() {
    let server = MockServer::start().await;
    let body: Value = serde_json::from_str(FIXTURE).expect("fixture parses");
    mount_fixture(&server, body).await;

    let provider = ModelScopeProvider::with_options(
        Arc::new(HubClient::new().expect("client")),
        server.uri(),
        "zh".to_string(),
    );
    let outcome = provider.search("data", 20, 1).await.expect("search ok");

    let first = &outcome.results[0];
    assert_eq!(first.description.as_deref(), Some("快速整理杂乱数据"));
    let stats = first.stats.as_ref().expect("stats present");
    assert_eq!(
        stats.get("category"),
        Some(&StatValue::Str("数据工具".to_string()))
    );
}

#[tokio::test]
async fn source_url_percent_encodes_special_chars() {
    let server = MockServer::start().await;

    let body = json!({
        "success": true,
        "data": {
            "total": 1,
            "skills": [ { "id": "@org/skill name™" } ]
        }
    });
    mount_fixture(&server, body).await;

    let provider = ModelScopeProvider::with_options(
        Arc::new(HubClient::new().expect("client")),
        server.uri(),
        "en".to_string(),
    );
    let outcome = provider.search("", 10, 1).await.expect("search ok");

    assert_eq!(outcome.results.len(), 1);

    assert_eq!(
        outcome.results[0].source_url,
        "https://modelscope.cn/skills/@org/skill%20name%E2%84%A2"
    );
}

#[tokio::test]
async fn upstream_failure_flag_is_an_error() {
    let server = MockServer::start().await;
    let body = json!({ "success": false, "message": "rate limited" });
    mount_fixture(&server, body).await;

    let provider = ModelScopeProvider::with_options(
        Arc::new(HubClient::new().expect("client")),
        server.uri(),
        "en".to_string(),
    );
    let err = provider.search("x", 10, 1).await.expect_err("should fail");
    assert_eq!(err.provider, "modelscope");
    assert!(err.message.contains("rate limited"), "got: {}", err.message);
}
