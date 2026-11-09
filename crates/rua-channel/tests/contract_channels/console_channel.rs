use rua_channel::core::config::ConsoleChannelConfig;
use rua_channel::core::console::ConsoleChatRequest;
use serde_json::json;

#[test]
fn console_chat_request_keeps_managed_media_in_ordered_user_content() {
    let request = ConsoleChatRequest {
        message: String::new(),
        chat_id: None,
        model: None,
        is_reconnect: false,
        attachments: Vec::new(),
        input: Some(vec![json!({
            "role": "user",
            "content": [
                { "type": "text", "text": "inspect this" },
                { "type": "image", "media_ref": "media/uuid-image.png" }
            ]
        })]),
        messages: None,
        extra: serde_json::Map::new(),
        channel_kind: None,
    };

    let parts = request.ordered_user_content_parts();
    assert_eq!(parts.len(), 2);
    assert!(matches!(
        &parts[1],
        agent_types::ContentPart::Image { url, .. } if url == "media/uuid-image.png"
    ));
}
#[test]
fn console_config_defaults_match_channel_contract() {
    let config: ConsoleChannelConfig = serde_json::from_value(json!({})).unwrap();

    assert!(config.is_enabled());
    assert!(config.base.enabled);
    assert!(config.base.show_thinking);
    assert!(config.base.show_tool_calls && config.base.show_tool_results);
    assert!(config.base.show_tool_details);
}
