use rua_skills_hub::bundle::normalize_bundle;
use rua_skills_hub::client::HubClient;
use rua_skills_hub::sources::skillhub::{extract_skillhub_spec, fetch_bundle};
use serial_test::serial;
use std::io::{Cursor, Write};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};
use zip::write::SimpleFileOptions;

fn restore_env() {
    std::env::remove_var("RUA_SKILLS_HUB_SKILLHUB_BASE");
}

#[test]
fn extract_spec_keeps_owner_and_slug() {
    let (owner, slug) =
        extract_skillhub_spec("https://api.skillhub.cn/alice/debugger").expect("spec");
    assert_eq!(owner, "alice");
    assert_eq!(slug, "debugger");
}

#[tokio::test]
#[serial]
async fn fetch_bundle_downloads_with_market_slug_and_normalizes_frontmatter() {
    let server = MockServer::start().await;
    let zip = zip_with_skill_md("# Debugger\n\nUse when debugging errors.");

    Mock::given(method("GET"))
        .and(path("/api/v1/download"))
        .and(query_param("slug", "debugger"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(zip))
        .mount(&server)
        .await;

    std::env::set_var("RUA_SKILLS_HUB_SKILLHUB_BASE", server.uri());
    let client = HubClient::new().expect("client");
    let raw = fetch_bundle(&client, "https://api.skillhub.cn/alice/debugger", "", None)
        .await
        .expect("fetch ok");
    restore_env();

    let normalized = normalize_bundle(&raw.payload).expect("normalized");
    assert_eq!(normalized.name, "debugger");
    assert!(normalized.content.contains("name: debugger"));
    assert!(normalized.content.contains("description:"));
    assert!(normalized.content.contains("# Debugger"));
}

fn zip_with_skill_md(skill_md: &str) -> Vec<u8> {
    let cursor = Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(cursor);
    writer
        .start_file("SKILL.md", SimpleFileOptions::default())
        .expect("start SKILL.md");
    writer
        .write_all(skill_md.as_bytes())
        .expect("write SKILL.md");
    writer.finish().expect("finish zip").into_inner()
}
