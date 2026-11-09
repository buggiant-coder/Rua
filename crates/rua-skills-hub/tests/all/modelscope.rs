use std::sync::Arc;

use rua_skills_hub::client::HubClient;
use rua_skills_hub::lib_ext::install_default_router;
use rua_skills_hub::resolve_bundle_source;
use rua_skills_hub::sources::modelscope::{extract_modelscope_skill_spec, fetch_bundle};
use rua_skills_hub::sources::{fetch_bundle as dispatcher_fetch_bundle, ExtensionRouter};
use rua_skills_hub::types::{BundleSource, HubError};
use serde_json::{json, Value};
use serial_test::serial;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn shorten_retry_env() {
    std::env::set_var("RUA_SKILLS_HUB_HTTP_RETRIES", "1");
    std::env::set_var("RUA_SKILLS_HUB_HTTP_BACKOFF_BASE", "0.1");
    std::env::set_var("RUA_SKILLS_HUB_HTTP_BACKOFF_CAP", "0.5");
}

fn restore_env() {
    std::env::remove_var("RUA_SKILLS_HUB_HTTP_RETRIES");
    std::env::remove_var("RUA_SKILLS_HUB_HTTP_BACKOFF_BASE");
    std::env::remove_var("RUA_SKILLS_HUB_HTTP_BACKOFF_CAP");
    std::env::remove_var("RUA_SKILLS_HUB_MODELSCOPE_BASE");
}

#[test]
fn resolve_modelscope_url_yields_modelscope_source() {
    let url = "https://modelscope.cn/skills/@owner/bar";
    let source = resolve_bundle_source(url).expect("resolve");
    assert_eq!(source, BundleSource::ModelScope(url.into()));
}

#[test]
fn extract_spec_handles_owner_prefix_and_archive_form() {
    let (owner, skill, hint) =
        extract_modelscope_skill_spec("https://modelscope.cn/skills/@alice/awesome-skill")
            .expect("ok");
    assert_eq!(owner, "alice");
    assert_eq!(skill, "awesome-skill");
    assert_eq!(hint, "");

    let (_owner, _skill, hint) = extract_modelscope_skill_spec(
        "https://modelscope.cn/skills/@alice/awesome-skill/archive/zip/v1.0.zip",
    )
    .expect("ok");
    assert_eq!(hint, "v1.0");

    let (owner, _skill, _hint) =
        extract_modelscope_skill_spec("https://modelscope.cn/skills/bob/another").expect("ok");
    assert_eq!(owner, "bob");

    assert!(extract_modelscope_skill_spec("https://example.com/skills/@x/y").is_none());

    assert!(extract_modelscope_skill_spec("https://modelscope.cn/skills/@x").is_none());
}

#[tokio::test]
#[serial]
async fn fetch_bundle_falls_back_to_readme_content() {
    shorten_retry_env();
    let server = MockServer::start().await;
    let body = json!({
        "Data": {
            "Name": "Awesome",
            "ReadMeContent": "---\nname: awesome\n---\nbody from readme",
            "SourceURL": ""
        }
    });
    Mock::given(method("GET"))
        .and(path("/api/v1/skills/@alice/awesome"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    std::env::set_var("RUA_SKILLS_HUB_MODELSCOPE_BASE", server.uri());
    let client = HubClient::new().expect("client");
    let url = "https://modelscope.cn/skills/@alice/awesome";
    let result = fetch_bundle(&client, url, "", None).await;
    restore_env();

    let raw = result.expect("fetch ok");
    assert_eq!(raw.source_url, url);
    let payload = raw.payload.as_object().expect("object");
    assert_eq!(payload.get("name"), Some(&Value::String("Awesome".into())));
    let files = payload
        .get("files")
        .and_then(Value::as_object)
        .expect("files object");
    let skill_md = files
        .get("SKILL.md")
        .and_then(Value::as_str)
        .expect("SKILL.md present");
    assert!(skill_md.contains("body from readme"));
}

#[tokio::test]
#[serial]
async fn fetch_bundle_errors_when_neither_source_url_nor_readme_present() {
    shorten_retry_env();
    let server = MockServer::start().await;
    let body = json!({
        "Data": { "SourceURL": "", "ReadMeContent": "" }
    });
    Mock::given(method("GET"))
        .and(path("/api/v1/skills/@alice/empty"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    std::env::set_var("RUA_SKILLS_HUB_MODELSCOPE_BASE", server.uri());
    let client = HubClient::new().expect("client");
    let url = "https://modelscope.cn/skills/@alice/empty";
    let result = fetch_bundle(&client, url, "", None).await;
    restore_env();

    let err = result.unwrap_err();
    matches!(err, HubError::InvalidBundle(_))
        .then_some(())
        .expect("must be InvalidBundle");
}

#[tokio::test]
#[serial]
async fn dispatcher_routes_modelscope_through_extension_router() {
    shorten_retry_env();
    let server = MockServer::start().await;
    let body = json!({
        "Data": {
            "Name": "Routed",
            "ReadMeContent": "---\nname: routed\n---\nrouted via dispatcher",
            "SourceURL": ""
        }
    });
    Mock::given(method("GET"))
        .and(path("/api/v1/skills/@alice/routed"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    std::env::set_var("RUA_SKILLS_HUB_MODELSCOPE_BASE", server.uri());
    let client = HubClient::new().expect("client");

    let _router: Arc<dyn ExtensionRouter> = install_default_router();

    let url = "https://modelscope.cn/skills/@alice/routed";
    let source = resolve_bundle_source(url).expect("resolve");
    let result = dispatcher_fetch_bundle(&client, &source, "", None).await;
    restore_env();
    let raw = result.expect("dispatcher routed via extension router");
    assert_eq!(raw.source_url, url);
}
