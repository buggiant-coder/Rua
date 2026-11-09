use rua_skills_market::provider::MarketProvider;
use rua_skills_market::providers::AliyunProvider;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fixture_body() -> &'static str {
    include_str!("../fixtures/aliyun_search.json")
}

#[tokio::test]
#[serial_test::serial]
async fn search_parses_fixture_response() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/openapi/skills"))
        .respond_with(ResponseTemplate::new(200).set_body_string(fixture_body()))
        .mount(&mock_server)
        .await;

    std::env::set_var("ALIYUN_AGENTEXPLORER_ENDPOINT", mock_server.uri());
    std::env::set_var("ALIBABA_CLOUD_ACCESS_KEY_ID", "test-ak");
    std::env::set_var("ALIBABA_CLOUD_ACCESS_KEY_SECRET", "test-sk");

    let provider = AliyunProvider::new();
    let outcome = provider.search("weather", 10, 1).await.unwrap();

    assert_eq!(outcome.results.len(), 2);
    assert!(outcome.has_more);
    assert_eq!(outcome.total, Some(42));

    let first = &outcome.results[0];
    assert_eq!(first.source, "aliyun");
    assert_eq!(first.slug, "weather-query");
    assert_eq!(first.name, "Weather Query");
    assert_eq!(
        first.description.as_deref(),
        Some("Get real-time weather information for any city")
    );
    assert!(first.source_url.contains("weather-query"));
    assert!(first.version.is_none());
    assert!(first.author.is_none());
    assert!(first.icon_url.is_none());

    let stats = first.stats.as_ref().unwrap();
    assert_eq!(
        stats.get("installs").unwrap(),
        &rua_skills_market::schema::StatValue::Int(1234)
    );
    assert_eq!(
        stats.get("likes").unwrap(),
        &rua_skills_market::schema::StatValue::Int(56)
    );
    assert_eq!(
        stats.get("category").unwrap(),
        &rua_skills_market::schema::StatValue::Str("Tools / Information".to_string())
    );

    std::env::remove_var("ALIYUN_AGENTEXPLORER_ENDPOINT");
    std::env::remove_var("ALIBABA_CLOUD_ACCESS_KEY_ID");
    std::env::remove_var("ALIBABA_CLOUD_ACCESS_KEY_SECRET");
}

#[tokio::test]
#[serial_test::serial]
async fn search_returns_empty_for_page_beyond_max_walk() {
    std::env::set_var("ALIBABA_CLOUD_ACCESS_KEY_ID", "test-ak");
    std::env::set_var("ALIBABA_CLOUD_ACCESS_KEY_SECRET", "test-sk");

    let provider = AliyunProvider::new();
    let outcome = provider.search("test", 10, 51).await.unwrap();

    assert!(outcome.results.is_empty());
    assert!(!outcome.has_more);
    assert!(outcome.total.is_none());

    std::env::remove_var("ALIBABA_CLOUD_ACCESS_KEY_ID");
    std::env::remove_var("ALIBABA_CLOUD_ACCESS_KEY_SECRET");
}

#[tokio::test]
#[serial_test::serial]
async fn search_fails_without_credentials() {
    std::env::remove_var("ALIBABA_CLOUD_ACCESS_KEY_ID");
    std::env::remove_var("ALIBABA_CLOUD_ACCESS_KEY_SECRET");

    let provider = AliyunProvider::new();
    let result = provider.search("test", 10, 1).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.provider, "aliyun");
    assert!(err.message.contains("credentials"));
}
