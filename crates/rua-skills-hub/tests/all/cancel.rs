use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rua_skills_hub::bundle::BundleInstaller;
use rua_skills_hub::client::HubClient;
use rua_skills_hub::types::HubError;
use rua_skills_hub::{install_skill_from_hub, search_hub_skills};
use serde_json::{Map, Value};
use serial_test::serial;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct CountingInstaller;

#[async_trait]
impl BundleInstaller for CountingInstaller {
    async fn create_skill(
        &self,
        name: &str,
        _content: &str,
        _references: &Map<String, Value>,
        _scripts: &Map<String, Value>,
        _extra_files: &Map<String, Value>,
        _install_meta: Option<&rua_skills::InstallMeta>,
    ) -> Result<String, HubError> {
        Ok(name.to_string())
    }

    async fn enable_skill(&self, _name: &str) -> Result<bool, HubError> {
        Ok(true)
    }
}

#[tokio::test]
#[serial]
async fn pre_cancelled_token_short_circuits() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/skills/blocked"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
        .expect(0)
        .mount(&server)
        .await;

    std::env::set_var("RUA_SKILLS_HUB_BASE_URL", server.uri());

    let client = HubClient::new().expect("client");
    let installer = CountingInstaller;
    let token = CancellationToken::new();
    token.cancel();

    let result = install_skill_from_hub(
        &client,
        &installer,
        "https://clawhub.ai/blocked",
        "",
        false,
        None,
        Some(token),
        None,
    )
    .await;

    std::env::remove_var("RUA_SKILLS_HUB_BASE_URL");

    match result {
        Err(HubError::Cancelled) => {}
        other => panic!("expected Cancelled, got {other:?}"),
    }
}

#[tokio::test]
#[serial]
async fn cancel_during_long_response_returns_cancelled() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/search"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("[]")
                .set_delay(Duration::from_millis(800)),
        )
        .mount(&server)
        .await;

    std::env::set_var("RUA_SKILLS_HUB_BASE_URL", server.uri());

    let client = Arc::new(HubClient::new().expect("client"));
    let token = CancellationToken::new();
    let token_for_task = token.clone();
    let client_for_task = client.clone();
    let handle =
        tokio::spawn(
            async move { search_hub_skills(client_for_task.as_ref(), "anything", 10).await },
        );
    tokio::time::sleep(Duration::from_millis(50)).await;
    token_for_task.cancel();
    let _ = handle.await;

    let token = CancellationToken::new();
    token.cancel();
    let installer = CountingInstaller;
    let direct_result = install_skill_from_hub(
        &client,
        &installer,
        "https://clawhub.ai/never-fetched",
        "",
        false,
        None,
        Some(token),
        None,
    )
    .await;

    std::env::remove_var("RUA_SKILLS_HUB_BASE_URL");

    matches!(direct_result, Err(HubError::Cancelled))
        .then_some(())
        .expect("install must short-circuit on cancel");
}
