use agent_types::{ChatRequest, ContentPart, ModelDialogueItem, Role};
use llm_multiplex::providers::gemini::{GeminiConfig, GeminiProvider};
use llm_multiplex::traits::ModelInfo;

fn make_provider() -> GeminiProvider {
    GeminiProvider::new(GeminiConfig {
        provider_id: "test-gemini".into(),
        provider_name: "Test Gemini".into(),
        base_url: "https://generativelanguage.googleapis.com".into(),
        api_key: Some("fake-key".into()),
        headers: serde_json::Map::new(),
        models: vec![ModelInfo {
            id: "gemini-2.0-flash".into(),
            name: "Gemini 2.0 Flash".into(),
            context_window: Some(1_000_000),
            max_output_tokens: None,
            supports_multimodal: Some(true),
            supports_image: Some(true),
            supports_video: Some(true),
            probe_source: None,
            generate_kwargs: serde_json::Map::new(),
        }],
        timeout_secs: 30,
        generate_kwargs: serde_json::Map::new(),
    })
    .unwrap()
}

fn build_body_with_parts(parts: Vec<ContentPart>) -> serde_json::Value {
    let provider = make_provider();
    let request = ChatRequest::new(vec![ModelDialogueItem {
        id: None,
        role: Role::User,
        content: parts,
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    }]);
    provider.build_request_body(&request, "gemini-2.0-flash")
}

fn get_first_user_parts(body: &serde_json::Value) -> Vec<serde_json::Value> {
    body["contents"][0]["parts"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[test]
fn data_uri_image_becomes_inline_data() {
    let body = build_body_with_parts(vec![
        ContentPart::Text {
            text: "What is this?".into(),
        },
        ContentPart::Image {
            url: "data:image/png;base64,iVBORw0KGgo=".into(),
            media_type: None,
            alt: None,
        },
    ]);

    let parts = get_first_user_parts(&body);
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0]["text"], "What is this?");

    let inline = &parts[1]["inlineData"];
    assert_eq!(inline["mimeType"], "image/png");
    assert_eq!(inline["data"], "iVBORw0KGgo=");
}

#[test]
fn http_image_becomes_file_data() {
    let body = build_body_with_parts(vec![ContentPart::Image {
        url: "https://example.com/photo.jpg".into(),
        media_type: Some("image/jpeg".into()),
        alt: None,
    }]);

    let parts = get_first_user_parts(&body);
    assert_eq!(parts.len(), 1);

    let file = &parts[0]["fileData"];
    assert_eq!(file["mimeType"], "image/jpeg");
    assert_eq!(file["fileUri"], "https://example.com/photo.jpg");
}

#[test]
fn http_image_guesses_mime_from_extension() {
    let body = build_body_with_parts(vec![ContentPart::Image {
        url: "https://example.com/photo.webp".into(),
        media_type: None,
        alt: None,
    }]);

    let parts = get_first_user_parts(&body);
    assert_eq!(parts[0]["fileData"]["mimeType"], "image/webp");
}

#[test]
fn data_uri_video_becomes_inline_data() {
    let body = build_body_with_parts(vec![ContentPart::Video {
        url: "data:video/mp4;base64,AAAAIGZ0eXA=".into(),
        media_type: None,
    }]);

    let parts = get_first_user_parts(&body);
    assert_eq!(parts.len(), 1);

    let inline = &parts[0]["inlineData"];
    assert_eq!(inline["mimeType"], "video/mp4");
    assert_eq!(inline["data"], "AAAAIGZ0eXA=");
}

#[test]
fn http_video_becomes_file_data() {
    let body = build_body_with_parts(vec![ContentPart::Video {
        url: "https://example.com/clip.mp4".into(),
        media_type: Some("video/mp4".into()),
    }]);

    let parts = get_first_user_parts(&body);
    let file = &parts[0]["fileData"];
    assert_eq!(file["mimeType"], "video/mp4");
    assert_eq!(file["fileUri"], "https://example.com/clip.mp4");
}

#[test]
fn explicit_media_type_overrides_data_uri_mime() {
    let body = build_body_with_parts(vec![ContentPart::Image {
        url: "data:image/png;base64,abc123".into(),
        media_type: Some("image/webp".into()),
        alt: None,
    }]);

    let parts = get_first_user_parts(&body);
    assert_eq!(parts[0]["inlineData"]["mimeType"], "image/webp");
    assert_eq!(parts[0]["inlineData"]["data"], "abc123");
}

#[test]
fn uses_camel_case_field_names() {
    let body = build_body_with_parts(vec![
        ContentPart::Image {
            url: "data:image/jpeg;base64,/9j/4AAQ".into(),
            media_type: None,
            alt: None,
        },
        ContentPart::Image {
            url: "https://example.com/img.png".into(),
            media_type: None,
            alt: None,
        },
    ]);

    let parts = get_first_user_parts(&body);
    assert!(parts[0].get("inlineData").is_some());
    assert!(parts[0].get("inline_data").is_none());
    assert!(parts[0]["inlineData"].get("mimeType").is_some());
    assert!(parts[0]["inlineData"].get("mime_type").is_none());

    assert!(parts[1].get("fileData").is_some());
    assert!(parts[1].get("file_data").is_none());
    assert!(parts[1]["fileData"].get("fileUri").is_some());
    assert!(parts[1]["fileData"].get("file_uri").is_none());
}
