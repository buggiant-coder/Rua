use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use rua_skills_hub::bundle::BundleInstaller;
use rua_skills_hub::client::HubClient;
use rua_skills_hub::types::{BundleSource, HubError};
use rua_skills_hub::{install_skill_from_hub, resolve_bundle_source};
use serde_json::{json, Map, Value};
use serial_test::serial;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Default)]
struct RecordingInstaller {
    last_name: std::sync::Mutex<Option<String>>,
    last_content: std::sync::Mutex<Option<String>>,
}

#[async_trait]
impl BundleInstaller for RecordingInstaller {
    async fn create_skill(
        &self,
        name: &str,
        content: &str,
        _references: &Map<String, Value>,
        _scripts: &Map<String, Value>,
        _extra_files: &Map<String, Value>,
        _install_meta: Option<&rua_skills::InstallMeta>,
    ) -> Result<String, HubError> {
        *self.last_name.lock().unwrap() = Some(name.to_string());
        *self.last_content.lock().unwrap() = Some(content.to_string());
        Ok(name.to_string())
    }

    async fn enable_skill(&self, _name: &str) -> Result<bool, HubError> {
        Ok(true)
    }
}

fn shorten_retry_env() {
    std::env::set_var("RUA_SKILLS_HUB_HTTP_RETRIES", "1");
    std::env::set_var("RUA_SKILLS_HUB_HTTP_BACKOFF_BASE", "0.1");
    std::env::set_var("RUA_SKILLS_HUB_HTTP_BACKOFF_CAP", "0.5");
}

fn restore_env() {
    std::env::remove_var("RUA_SKILLS_HUB_HTTP_RETRIES");
    std::env::remove_var("RUA_SKILLS_HUB_HTTP_BACKOFF_BASE");
    std::env::remove_var("RUA_SKILLS_HUB_HTTP_BACKOFF_CAP");
    std::env::remove_var("RUA_SKILLS_HUB_BASE_URL");
}

#[tokio::test]
async fn resolve_clawhub_url_yields_clawhub_source() {
    let source = resolve_bundle_source("https://clawhub.ai/owner/my-skill").expect("resolve");
    assert_eq!(source, BundleSource::ClawHub("my-skill".to_string()));
}

#[tokio::test]
#[serial]
async fn install_from_clawhub_full_bundle() {
    shorten_retry_env();
    let server = MockServer::start().await;

    let detail = json!({
        "skill": {"slug": "demo", "displayName": "Demo Skill"},
        "version": {
            "version": "1.0.0",
            "files": [
                {"path": "SKILL.md"},
                {"path": "references/notes.md"}
            ]
        }
    });
    Mock::given(method("GET"))
        .and(path("/api/v1/skills/demo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(detail))
        .mount(&server)
        .await;

    let skill_md = "---\nname: demo\ndescription: a demo\n---\nbody";
    Mock::given(method("GET"))
        .and(path("/api/v1/skills/demo/file"))
        .and(query_param("path", "SKILL.md"))
        .and(query_param("version", "1.0.0"))
        .respond_with(ResponseTemplate::new(200).set_body_string(skill_md))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/skills/demo/file"))
        .and(query_param("path", "references/notes.md"))
        .and(query_param("version", "1.0.0"))
        .respond_with(ResponseTemplate::new(200).set_body_string("hello"))
        .mount(&server)
        .await;

    std::env::set_var("RUA_SKILLS_HUB_BASE_URL", server.uri());

    let client = HubClient::new().expect("client");
    let installer = RecordingInstaller::default();

    let result = install_skill_from_hub(
        &client,
        &installer,
        "https://clawhub.ai/demo",
        "",
        true,
        None,
        None,
        None,
    )
    .await;

    restore_env();

    let info = result.expect("install ok");
    assert_eq!(info.name, "Demo Skill");
    assert!(info.enabled);
    assert_eq!(
        installer.last_name.lock().unwrap().as_deref(),
        Some("Demo Skill")
    );
    let recorded = installer.last_content.lock().unwrap().clone().unwrap();
    assert!(recorded.contains("name: demo"));
}

#[tokio::test]
#[serial]
async fn install_from_clawhub_retries_on_503_then_fails() {
    shorten_retry_env();
    let server = MockServer::start().await;

    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();

    use wiremock::{Request, Respond};
    struct CountingResponder {
        counter: Arc<AtomicUsize>,
    }
    impl Respond for CountingResponder {
        fn respond(&self, _req: &Request) -> ResponseTemplate {
            self.counter.fetch_add(1, Ordering::SeqCst);
            ResponseTemplate::new(503).set_body_string("upstream blew up")
        }
    }
    Mock::given(method("GET"))
        .and(path("/api/v1/skills/flaky"))
        .respond_with(CountingResponder {
            counter: counter_clone,
        })
        .mount(&server)
        .await;

    std::env::set_var("RUA_SKILLS_HUB_BASE_URL", server.uri());
    let client = HubClient::new().expect("client");
    let installer = RecordingInstaller::default();
    let result = install_skill_from_hub(
        &client,
        &installer,
        "https://clawhub.ai/flaky",
        "",
        false,
        None,
        None,
        None,
    )
    .await;
    restore_env();

    match result {
        Err(HubError::Http { status: 0, .. }) | Err(HubError::Http { status: 503, .. }) => {}
        other => panic!("expected 503-shaped error, got {other:?}"),
    }
    assert_eq!(
        counter.load(Ordering::SeqCst),
        2,
        "retries env says retries=1 → exactly 2 attempts hit the upstream"
    );
}

#[tokio::test]
async fn unsupported_source_returns_unsupported_source_error() {
    let url = "https://skills.sh/owner/repo/skill";
    let source = resolve_bundle_source(url).expect("resolve");
    assert!(matches!(source, BundleSource::SkillsSh(_)));

    let client = HubClient::new().expect("client");
    let result = rua_skills_hub::sources::fetch_bundle(&client, &source, "", None).await;
    match result {
        Err(HubError::UnsupportedSource(_)) => {}
        other => panic!("expected UnsupportedSource, got {other:?}"),
    }
}
