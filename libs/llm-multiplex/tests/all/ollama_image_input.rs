use agent_types::{ChatRequest, ContentPart, ModelDialogueItem, Role};
use llm_multiplex::providers::ollama::{OllamaConfig, OllamaProvider};

fn make_provider() -> OllamaProvider {
    OllamaProvider::new(OllamaConfig {
        provider_id: "test-ollama".into(),
        provider_name: "Test Ollama".into(),
        base_url: "http://localhost:11434".into(),
        models: vec![],
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
    provider.build_request_body(&request, "llava")
}

#[test]
fn data_uri_image_produces_images_array() {
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

    let msg = &body["messages"][0];
    assert_eq!(msg["content"], "What is this?");

    let images = msg["images"].as_array().expect("images array should exist");
    assert_eq!(images.len(), 1);
    assert_eq!(images[0], "iVBORw0KGgo=");
}

#[test]
fn no_data_prefix_in_images() {
    let body = build_body_with_parts(vec![ContentPart::Image {
        url: "data:image/jpeg;base64,/9j/4AAQ".into(),
        media_type: None,
        alt: None,
    }]);

    let msg = &body["messages"][0];
    let images = msg["images"].as_array().unwrap();
    let img = images[0].as_str().unwrap();
    assert!(
        !img.starts_with("data:"),
        "images must be raw base64, no data: prefix"
    );
    assert_eq!(img, "/9j/4AAQ");
}

#[test]
fn multiple_images_collected() {
    let body = build_body_with_parts(vec![
        ContentPart::Text {
            text: "Compare these".into(),
        },
        ContentPart::Image {
            url: "data:image/png;base64,AAAA".into(),
            media_type: None,
            alt: None,
        },
        ContentPart::Image {
            url: "data:image/jpeg;base64,BBBB".into(),
            media_type: None,
            alt: None,
        },
    ]);

    let msg = &body["messages"][0];
    let images = msg["images"].as_array().unwrap();
    assert_eq!(images.len(), 2);
    assert_eq!(images[0], "AAAA");
    assert_eq!(images[1], "BBBB");
}

#[test]
fn http_url_skipped_no_images_field() {
    let body = build_body_with_parts(vec![
        ContentPart::Text {
            text: "hello".into(),
        },
        ContentPart::Image {
            url: "https://example.com/photo.jpg".into(),
            media_type: None,
            alt: None,
        },
    ]);

    let msg = &body["messages"][0];
    assert!(msg.get("images").is_none() || msg["images"].as_array().unwrap().is_empty());
}

#[test]
fn text_only_message_no_images_field() {
    let body = build_body_with_parts(vec![ContentPart::Text {
        text: "just text".into(),
    }]);

    let msg = &body["messages"][0];
    assert!(msg.get("images").is_none());
}
