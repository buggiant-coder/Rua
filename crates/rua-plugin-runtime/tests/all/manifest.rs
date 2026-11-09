use std::path::Path;

use rua_plugin_runtime::{PluginManifest, PluginManifestError};

#[test]
fn parse_full_manifest() {
    let json = r#"{
        "id": "test-plugin",
        "name": "Test Plugin",
        "version": "1.0.0",
        "description": "A test plugin",
        "author": "tester",
        "entry_point": "main.wasm",
        "dependencies": ["dep-a"],
        "min_rua_version": "0.2.0",
        "meta": {"key": "value"}
    }"#;
    let manifest = PluginManifest::from_json(json).unwrap();
    assert_eq!(manifest.id, "test-plugin");
    assert_eq!(manifest.name, "Test Plugin");
    assert_eq!(manifest.version, "1.0.0");
    assert_eq!(manifest.description, "A test plugin");
    assert_eq!(manifest.author, "tester");
    assert_eq!(manifest.entry_point, "main.wasm");
    assert_eq!(manifest.dependencies, vec!["dep-a"]);
    assert_eq!(manifest.min_rua_version, "0.2.0");
    assert_eq!(manifest.meta.get("key").unwrap(), "value");
}

#[test]
fn parse_minimal_manifest_defaults_runtime_fields() {
    let json = r#"{"id":"p1","name":"P1","version":"0.1.0"}"#;
    let manifest = PluginManifest::from_json(json).unwrap();
    assert_eq!(manifest.id, "p1");
    assert_eq!(manifest.entry_point, "main");
    assert_eq!(manifest.min_rua_version, "0.1.0");
    assert!(manifest.dependencies.is_empty());
    assert!(manifest.meta.is_empty());
    assert!(manifest.providers.is_empty());
}

#[test]
fn parse_missing_required_field_fails() {
    let json = r#"{"id":"p1","name":"P1"}"#;
    assert!(PluginManifest::from_json(json).is_err());
}

#[test]
fn parse_manifest_with_provider_declarations() {
    let json = r#"{
        "id": "my-llm-plugin",
        "name": "My LLM Plugin",
        "version": "1.0.0",
        "providers": [
            {
                "id": "my-provider",
                "label": "My Custom Provider",
                "base_url": "https://api.example.com/v1",
                "chat_model": "OpenAIChatModel",
                "preset_models": ["model-a", "model-b"],
                "require_api_key": true
            },
            {
                "id": "local-provider",
                "label": "Local LLM",
                "base_url": "http://localhost:8080",
                "require_api_key": false
            }
        ]
    }"#;
    let manifest = PluginManifest::from_json(json).unwrap();
    assert_eq!(manifest.providers.len(), 2);

    let p0 = &manifest.providers[0];
    assert_eq!(p0.id, "my-provider");
    assert_eq!(p0.label, "My Custom Provider");
    assert_eq!(p0.base_url, "https://api.example.com/v1");
    assert_eq!(p0.chat_model.as_deref(), Some("OpenAIChatModel"));
    assert_eq!(p0.preset_models, vec!["model-a", "model-b"]);
    assert!(p0.require_api_key);

    let p1 = &manifest.providers[1];
    assert_eq!(p1.id, "local-provider");
    assert_eq!(p1.chat_model, None);
    assert!(!p1.require_api_key);
}

#[test]
fn from_file_not_found_reports_io_error() {
    let err = PluginManifest::from_file(Path::new("/nonexistent/manifest.json"));
    assert!(matches!(err, Err(PluginManifestError::Io { .. })));
}
