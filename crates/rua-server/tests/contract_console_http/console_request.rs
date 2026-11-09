use async_trait::async_trait;
use axum::http::{Method, StatusCode};
use rua_channel::core::console::{ConsoleChatFacade, ConsoleChatRequest, ConsoleSubmitOutcome};
use rua_core::ContentPart;
use rua_server::endpoints::dto::ChatRequest;
use rua_server::services::console_chat::truncate_chat_name;
use serde_json::json;
use std::sync::Arc;

use super::support;

struct AuthoritativeControlFacade;

#[async_trait]
impl ConsoleChatFacade for AuthoritativeControlFacade {
    async fn submit_chat(
        &self,
        _agent_id: &str,
        request: ConsoleChatRequest,
    ) -> ConsoleSubmitOutcome {
        let mut outcome =
            ConsoleSubmitOutcome::started(request.chat_id.expect("Chat ID in submit"));
        outcome.control = Some(json!({
            "revision": 99,
            "authority_device_id": "remote-authority",
            "placement": {
                "authority_device_id": "remote-authority",
                "switchable": true
            },
            "queue": {
                "entries": [],
                "paused": false
            },
            "acp": {
                "active": false
            },
            "allowed_controls": {
                "stop": true,
                "queue": true,
                "interject": false,
                "edit_queued_input": false,
                "delete_queued_input": false,
                "switch_authority": false,
                "enter_acp": false,
                "exit_acp": false
            }
        }));
        outcome
    }

    async fn stop_chat(&self, _agent_id: &str, _chat_id: &str) -> bool {
        false
    }
}

fn chat_request() -> ChatRequest {
    ChatRequest {
        message: String::new(),
        chat_id: None,
        input: None,
        messages: None,
        user_id: None,
        channel: None,
        stream: None,
        model: None,
        attachments: Vec::new(),
        extra: serde_json::Map::new(),
    }
}

#[test]
fn chat_request_uses_structured_model_and_accepts_legacy_string() {
    let structured: ChatRequest = serde_json::from_value(json!({
        "message": "hello",
        "model": {
            "provider_id": "opencode",
            "model": "deepseek-v4-flash-free",
            "thinking_strength": "high"
        }
    }))
    .expect("structured model");
    let structured_model = structured.model.expect("model");
    assert_eq!(structured_model.model_id(), "deepseek-v4-flash-free");
    assert_eq!(
        structured_model
            .structured()
            .expect("structured slot")
            .provider_id,
        "opencode"
    );

    let legacy: ChatRequest = serde_json::from_value(json!({
        "message": "hello",
        "model": "deepseek-v4-flash-free"
    }))
    .expect("legacy model id");
    let legacy_model = legacy.model.expect("legacy model");
    assert_eq!(legacy_model.model_id(), "deepseek-v4-flash-free");
    assert!(legacy_model.structured().is_none());
}

#[tokio::test]
async fn submit_response_preserves_authority_control_from_facade() {
    let app = support::build_app_with_console_facade(Arc::new(AuthoritativeControlFacade));
    let response = support::request_json(
        &app.app,
        Method::POST,
        "/agents/default/chats/9b6de0e4-5a5b-4f98-9cb3-d1db20e6e1ac/console/submit",
        json!({
            "chat_id": "9b6de0e4-5a5b-4f98-9cb3-d1db20e6e1ac",
            "message": "remote submit"
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = support::read_json(response).await;
    assert_eq!(body["mode"], "started");
    assert_eq!(body["control"]["revision"], 99);
    assert_eq!(body["control"]["authority_device_id"], "remote-authority");
}

#[tokio::test]
async fn agent_chat_path_rejects_a_different_body_chat_id() {
    let app = support::build_app_with_console_facade(Arc::new(AuthoritativeControlFacade));
    let response = support::request_json(
        &app.app,
        Method::POST,
        "/agents/default/chats/00000000-0000-4000-8000-000000000411/console/submit",
        json!({
            "chat_id": "00000000-0000-4000-8000-000000000412",
            "message": "wrong chat"
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = support::read_json(response).await;
    assert_eq!(
        body["detail"],
        "Agent Chat path and request body identify different Chats"
    );
}

#[tokio::test]
async fn legacy_agent_scoped_submit_path_is_not_available() {
    let app = support::build_app_with_console_facade(Arc::new(AuthoritativeControlFacade));
    let response = support::request_json(
        &app.app,
        Method::POST,
        "/agents/default/console/chat/submit",
        json!({
            "chat_id": "00000000-0000-4000-8000-000000000413",
            "message": "legacy path"
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

fn request_with_input(content: serde_json::Value) -> ChatRequest {
    ChatRequest {
        input: Some(vec![json!({
            "role": "user",
            "content": content
        })]),
        ..chat_request()
    }
}

fn request_with_messages(content: serde_json::Value) -> ChatRequest {
    ChatRequest {
        messages: Some(vec![json!({
            "role": "user",
            "content": content
        })]),
        ..chat_request()
    }
}

#[test]
fn effective_message_extracts_text_parts_without_stringifying_content_blocks() {
    let req = request_with_input(json!([
        {"type": "image", "image_url": "file:///tmp/image.png"},
        {"type": "text", "text": "What's the weather today?"},
        {"type": "text", "text": "Use Celsius."}
    ]));

    assert_eq!(
        req.effective_message(),
        "What's the weather today?\nUse Celsius."
    );
    assert_eq!(
        truncate_chat_name(&req.effective_message()),
        "What's the weather today?\nUse Celsius."
    );
}

#[test]
fn effective_message_accepts_string_content_shapes() {
    let raw_part = request_with_input(json!(["Hello, world!", {"type": "text", "text": "Again"}]));
    assert_eq!(raw_part.effective_message(), "Hello, world!\nAgain");

    let direct_string = ChatRequest {
        input: Some(vec![json!({
            "role": "user",
            "content": "Direct string content"
        })]),
        ..chat_request()
    };
    assert_eq!(direct_string.effective_message(), "Direct string content");
}

#[test]
fn effective_message_uses_messages_alias_when_input_is_absent() {
    let req = ChatRequest {
        messages: Some(vec![json!({
            "role": "user",
            "content": [{"type": "text", "text": "Alias text"}]
        })]),
        ..chat_request()
    };

    assert_eq!(req.effective_message(), "Alias text");
}

#[test]
fn effective_message_only_uses_user_role_text_in_message_order() {
    let req = ChatRequest {
        input: Some(vec![
            json!({
                "role": "system",
                "content": [{"type": "text", "text": "Hidden system prompt"}]
            }),
            json!({
                "role": "user",
                "content": [{"type": "text", "text": "First user request"}]
            }),
            json!({
                "role": "assistant",
                "content": "Assistant draft must not become a title"
            }),
            json!({
                "role": "user",
                "content": "Second user request"
            }),
        ]),
        ..chat_request()
    };

    assert_eq!(
        req.effective_message(),
        "First user request\nSecond user request"
    );
    assert_eq!(
        truncate_chat_name(&req.effective_message()),
        "First user request\nSecond user request"
    );
}

#[test]
fn effective_message_treats_media_only_or_unknown_parts_as_empty_text() {
    for content in [
        json!([{"type": "image", "image_url": "file:///tmp/image.png"}]),
        json!([{"type": "text", "text": 123}]),
        json!([{"type": "unknown", "value": {"nested": true}}]),
        json!([]),
    ] {
        let req = request_with_input(content);
        assert_eq!(req.effective_message(), "");
        assert_eq!(truncate_chat_name(&req.effective_message()), "");
    }
}

#[test]
fn effective_message_does_not_stringify_object_content_or_unknown_messages() {
    let direct_object = request_with_input(json!({
        "type": "image",
        "image_url": "file:///tmp/image.png"
    }));
    assert_eq!(direct_object.effective_message(), "");
    assert_eq!(truncate_chat_name(&direct_object.effective_message()), "");

    let unknown_message_shape = ChatRequest {
        input: Some(vec![
            json!({"content": "missing role is ignored"}),
            json!("raw string message is ignored"),
            json!({"role": "tool", "content": {"type": "text", "text": "tool output"}}),
        ]),
        ..chat_request()
    };
    assert_eq!(unknown_message_shape.effective_message(), "");
    assert_eq!(
        truncate_chat_name(&unknown_message_shape.effective_message()),
        ""
    );
}

#[test]
fn effective_message_falls_back_to_legacy_message_when_input_has_no_text() {
    let req = ChatRequest {
        message: "legacy message".into(),
        input: Some(vec![json!({
            "role": "user",
            "content": [{"type": "image", "image_url": "file:///tmp/image.png"}]
        })]),
        ..chat_request()
    };

    assert_eq!(req.effective_message(), "legacy message");
}

#[test]
fn extract_content_parts_maps_user_file_image_video_and_audio_blocks() {
    let req = request_with_input(json!([
        {"type": "text", "text": "describe these"},
        {"type": "file", "file_url": "file:///tmp/report.pdf", "file_name": "report.pdf"},
        {"type": "image", "image_url": "file:///tmp/chart.png"},
        {"type": "video", "video_url": "file:///tmp/demo.mp4"},
        {"type": "audio", "audio_url": "file:///tmp/note.mp3"}
    ]));

    let parts = req.extract_content_parts();

    assert_eq!(parts.len(), 4);
    assert!(matches!(
        &parts[0],
        ContentPart::File {
            url,
            name,
            media_type: None,
            size: None,
        } if url == "file:///tmp/report.pdf" && name == "report.pdf"
    ));
    assert!(matches!(
        &parts[1],
        ContentPart::Image {
            url,
            media_type: None,
            alt: None,
        } if url == "file:///tmp/chart.png"
    ));
    assert!(matches!(
        &parts[2],
        ContentPart::Video {
            url,
            media_type: None,
        } if url == "file:///tmp/demo.mp4"
    ));
    assert!(matches!(
        &parts[3],
        ContentPart::Audio {
            url,
            media_type: None,
        } if url == "file:///tmp/note.mp3"
    ));
}

#[test]
fn extract_content_parts_prefers_input_alias_and_ignores_non_user_or_empty_blocks() {
    let req = ChatRequest {
        input: Some(vec![
            json!({
                "role": "assistant",
                "content": [{"type": "image", "image_url": "file:///tmp/assistant.png"}]
            }),
            json!({
                "role": "user",
                "content": [
                    {"type": "file", "url": "file:///tmp/input.txt", "name": "input.txt"},
                    {"type": "image", "image_url": ""},
                    {"type": "unknown", "url": "file:///tmp/ignored.bin"}
                ]
            }),
        ]),
        messages: Some(vec![json!({
            "role": "user",
            "content": [{"type": "video", "video_url": "file:///tmp/messages.mp4"}]
        })]),
        ..chat_request()
    };

    let parts = req.extract_content_parts();

    assert_eq!(parts.len(), 1);
    assert!(matches!(
        &parts[0],
        ContentPart::File { url, name, .. }
            if url == "file:///tmp/input.txt" && name == "input.txt"
    ));
}

#[test]
fn extract_content_parts_uses_messages_alias_when_input_is_absent() {
    let req = request_with_messages(json!([
        {"type": "video", "url": "file:///tmp/from-messages.webm"}
    ]));

    let parts = req.extract_content_parts();

    assert_eq!(parts.len(), 1);
    assert!(matches!(
        &parts[0],
        ContentPart::Video { url, .. } if url == "file:///tmp/from-messages.webm"
    ));
}

#[test]
fn truncate_chat_name_trims_and_caps_long_titles_without_byte_slicing() {
    assert_eq!(truncate_chat_name("  Hello, world!  "), "Hello, world!");

    let long = "你".repeat(100);
    let title = truncate_chat_name(&long);

    assert_eq!(title.chars().count(), 78);
    assert!(title.ends_with('…'));
}

#[test]
fn truncate_chat_name_preserves_user_text_instead_of_applying_model_title_cleanup() {
    assert_eq!(
        truncate_chat_name("  \"Trip Planning Assistant.\"  "),
        "\"Trip Planning Assistant.\""
    );
    assert_eq!(
        truncate_chat_name("Line one\nLine two\nLine three"),
        "Line one\nLine two\nLine three"
    );
    assert_eq!(
        truncate_chat_name("What now?"),
        "What now?",
        "Rust chat titles come from user text and should not strip punctuation like QwenPaw's LLM-title cleaner"
    );
}
