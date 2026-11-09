use std::io::Write;

use rua_skills_hub::client::HubClient;
use rua_skills_hub::resolve_bundle_source;
use rua_skills_hub::sources::lobehub::{
    extract_lobehub_identifier, fetch_bundle, lobehub_download_url, lobehub_zip_to_bundle,
    lobehub_zip_to_bundle_with_limits, should_keep_lobehub_file, LOBEHUB_MAX_ZIP_BYTES,
    LOBEHUB_MAX_ZIP_ENTRIES,
};
use rua_skills_hub::types::{BundleSource, HubError};
use serde_json::Value;
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
    std::env::remove_var("RUA_SKILLS_HUB_LOBEHUB_BASE");
}

fn make_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let cursor = std::io::Cursor::new(&mut buf);
        let mut writer = zip::ZipWriter::new(cursor);
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, body) in entries {
            writer.start_file(*name, opts).expect("start file");
            writer.write_all(body).expect("write entry");
        }
        writer.finish().expect("finalise zip");
    }
    buf
}

#[test]
fn resolve_lobehub_url_yields_lobehub_source() {
    let source = resolve_bundle_source("https://lobehub.com/skills/foo").expect("resolve");
    assert_eq!(
        source,
        BundleSource::LobeHub("https://lobehub.com/skills/foo".into())
    );
}

#[test]
fn resolve_market_lobehub_url_also_dispatches_via_lobehub() {
    let url = "https://market.lobehub.com/api/v1/skills/bar/download";
    let source = resolve_bundle_source(url).expect("resolve");
    assert_eq!(source, BundleSource::LobeHub(url.into()));
}

#[test]
fn extract_identifier_handles_both_url_shapes() {
    assert_eq!(
        extract_lobehub_identifier("https://lobehub.com/skills/cool-skill"),
        Some("cool-skill".to_string()),
    );
    assert_eq!(
        extract_lobehub_identifier("https://www.lobehub.com/skills/foo"),
        Some("foo".to_string()),
    );
    assert_eq!(
        extract_lobehub_identifier("https://market.lobehub.com/api/v1/skills/abc/download"),
        Some("abc".to_string()),
    );

    assert_eq!(
        extract_lobehub_identifier("https://example.com/skills/x"),
        None
    );

    assert_eq!(
        extract_lobehub_identifier("https://lobehub.com/something/else"),
        None
    );
}

#[test]
fn should_keep_filter_matches_python_semantics() {
    let p = |s: &str| -> Vec<String> { s.split('/').map(str::to_string).collect() };
    assert!(should_keep_lobehub_file(&p("SKILL.md")));
    assert!(should_keep_lobehub_file(&p("references/note.md")));
    assert!(should_keep_lobehub_file(&p("references/sub/note.md")));
    assert!(should_keep_lobehub_file(&p("scripts/run.sh")));

    assert!(should_keep_lobehub_file(&p("README.md")));

    assert!(!should_keep_lobehub_file(&p("docs/x.md")));
    assert!(!should_keep_lobehub_file(&p("media/img.png")));
    assert!(!should_keep_lobehub_file(&[]));
}

#[test]
fn zip_to_bundle_keeps_only_allowed_files() {
    let payload = make_zip(&[
        ("SKILL.md", b"---\nname: demo\n---\nbody"),
        ("references/notes.md", b"hello"),
        ("scripts/run.sh", b"#!/bin/bash"),
        ("docs/internal.md", b"should be skipped"),
    ]);
    let bundle = lobehub_zip_to_bundle("demo", &payload).expect("ok");
    let obj = bundle.as_object().unwrap();
    assert_eq!(obj.get("name"), Some(&Value::String("demo".into())));
    let files = obj.get("files").and_then(Value::as_object).unwrap();
    assert!(files.contains_key("SKILL.md"));
    assert!(files.contains_key("references/notes.md"));
    assert!(files.contains_key("scripts/run.sh"));
    assert!(!files.contains_key("docs/internal.md"));
}

#[test]
fn zip_missing_skill_md_is_invalid_bundle() {
    let payload = make_zip(&[("references/notes.md", b"hello")]);
    let err = lobehub_zip_to_bundle("demo", &payload).unwrap_err();
    matches!(err, HubError::InvalidBundle(_))
        .then_some(())
        .expect("must be InvalidBundle");
}

#[test]
fn zip_with_too_many_entries_returns_zip_limit() {
    let max_entries = 8;
    let mut entries: Vec<(String, Vec<u8>)> = Vec::with_capacity(max_entries + 2);
    entries.push((
        "SKILL.md".to_string(),
        b"---\nname: demo\n---\nbody".to_vec(),
    ));
    for i in 0..max_entries {
        entries.push((format!("references/note-{i}.md"), b"x".to_vec()));
    }
    let entries_ref: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_slice()))
        .collect();
    let payload = make_zip(&entries_ref);

    let err =
        lobehub_zip_to_bundle_with_limits("demo", &payload, max_entries, LOBEHUB_MAX_ZIP_BYTES)
            .unwrap_err();
    matches!(err, HubError::ZipLimit)
        .then_some(())
        .expect("must be ZipLimit (entry cap)");
}

#[test]
fn zip_with_too_many_bytes_returns_zip_limit() {
    let max_bytes = 32;
    let big = vec![b'A'; (max_bytes as usize) + 1];
    let payload = make_zip(&[
        ("SKILL.md", b"---\nname: demo\n---\nbody"),
        ("references/big.md", &big),
    ]);
    let err =
        lobehub_zip_to_bundle_with_limits("demo", &payload, LOBEHUB_MAX_ZIP_ENTRIES, max_bytes)
            .unwrap_err();
    matches!(err, HubError::ZipLimit)
        .then_some(())
        .expect("must be ZipLimit (byte cap)");
}

#[test]
fn default_lobehub_limits_match_skill_package_policy() {
    assert_eq!(LOBEHUB_MAX_ZIP_ENTRIES, 4096);
    assert_eq!(LOBEHUB_MAX_ZIP_BYTES, 200 * 1024 * 1024);
}

#[tokio::test]
#[serial]
async fn fetch_bundle_end_to_end_via_wiremock() {
    shorten_retry_env();
    let server = MockServer::start().await;
    let payload = make_zip(&[
        (
            "SKILL.md",
            b"---\nname: demo\ndescription: desc\n---\nbody\n",
        ),
        ("references/notes.md", b"hello"),
    ]);
    Mock::given(method("GET"))
        .and(path("/api/v1/skills/demo/download"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(payload.clone()))
        .mount(&server)
        .await;

    std::env::set_var("RUA_SKILLS_HUB_LOBEHUB_BASE", server.uri());
    let client = HubClient::new().expect("client");
    let result = fetch_bundle(&client, "https://lobehub.com/skills/demo", "", None).await;
    restore_env();
    let raw = result.expect("fetch ok");
    assert_eq!(raw.source_url, "https://lobehub.com/skills/demo");
    let files = raw
        .payload
        .as_object()
        .unwrap()
        .get("files")
        .and_then(Value::as_object)
        .unwrap();
    assert!(files.contains_key("SKILL.md"));
    assert!(files.contains_key("references/notes.md"));
}

#[test]
fn download_url_uses_env_override() {
    std::env::set_var("RUA_SKILLS_HUB_LOBEHUB_BASE", "http://127.0.0.1:9999");
    let url = lobehub_download_url("foo");
    std::env::remove_var("RUA_SKILLS_HUB_LOBEHUB_BASE");
    assert_eq!(url, "http://127.0.0.1:9999/api/v1/skills/foo/download");
}
