use rua_local_model::source_ollama::{OllamaLayer, OllamaManifest};
use rua_local_model::OllamaSource;

#[test]
fn parse_ollama_manifest_picks_model_layer() {
    let json = r#"{
        "layers": [
            {"digest": "sha256:abc123", "size": 100, "mediaType": "application/vnd.ollama.image.license"},
            {"digest": "sha256:def456", "size": 4000000000, "mediaType": "application/vnd.ollama.image.model"},
            {"digest": "sha256:ghi789", "size": 500, "mediaType": "application/vnd.ollama.image.template"}
        ]
    }"#;

    let manifest: OllamaManifest = serde_json::from_str(json).unwrap();

    assert_eq!(manifest.layers.len(), 3);
    let model = manifest.model_layer().unwrap();
    assert_eq!(model.digest, "sha256:def456");
    assert_eq!(model.size, 4_000_000_000);
}

#[test]
fn ollama_blob_url_uses_library_registry_path() {
    let src = OllamaSource::new("llama3.1", Some("8b".into()));
    let url = src.blob_url("sha256:abc123");

    assert_eq!(
        url,
        "https://registry.ollama.ai/v2/library/llama3.1/blobs/sha256:abc123"
    );
}

#[test]
fn model_layer_picks_largest_model_media_type() {
    let manifest = OllamaManifest {
        layers: vec![
            OllamaLayer {
                digest: "sha256:small".into(),
                size: 1000,
                media_type: "application/vnd.ollama.image.model".into(),
            },
            OllamaLayer {
                digest: "sha256:big".into(),
                size: 5_000_000_000,
                media_type: "application/vnd.ollama.image.model".into(),
            },
        ],
    };

    let layer = manifest.model_layer().unwrap();

    assert_eq!(layer.digest, "sha256:big");
}
