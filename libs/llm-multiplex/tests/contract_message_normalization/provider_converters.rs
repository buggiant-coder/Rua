use agent_types::{ChatRequest, ContentPart, ModelDialogueItem, Role, ToolCall, ToolResult};
use llm_multiplex::{
    AnthropicConfig, AnthropicProvider, GeminiConfig, GeminiProvider, OpenAiCompatibleConfig,
    OpenAiCompatibleProvider,
};
use serde_json::{json, Value};

fn openai_provider() -> OpenAiCompatibleProvider {
    OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        provider_id: "openai-compatible".into(),
        provider_name: "OpenAI-compatible".into(),
        base_url: "https://llm.example.test/v1".into(),
        api_key: None,
        api_keys: Vec::new(),
        headers: serde_json::Map::new(),
        models: Vec::new(),
        timeout_secs: 30,
        generate_kwargs: serde_json::Map::new(),
        support_model_discovery: false,
        support_connection_check: false,
        is_local: false,
    })
    .expect("openai provider")
}

fn anthropic_provider() -> AnthropicProvider {
    AnthropicProvider::new(AnthropicConfig {
        provider_id: "anthropic".into(),
        provider_name: "Anthropic".into(),
        base_url: "https://anthropic.example.test".into(),
        api_key: None,
        headers: serde_json::Map::new(),
        models: Vec::new(),
        timeout_secs: 30,
        generate_kwargs: serde_json::Map::new(),
        support_connection_check: true,
        api_version: "2023-06-01".into(),
        auth_mode: Default::default(),
    })
    .expect("anthropic provider")
}

fn gemini_provider() -> GeminiProvider {
    GeminiProvider::new(GeminiConfig {
        provider_id: "gemini".into(),
        provider_name: "Gemini".into(),
        base_url: "https://generativelanguage.googleapis.com".into(),
        api_key: None,
        headers: serde_json::Map::new(),
        models: Vec::new(),
        timeout_secs: 30,
        generate_kwargs: serde_json::Map::new(),
    })
    .expect("gemini provider")
}

fn assistant_tool_call(id: &str, name: &str) -> ModelDialogueItem {
    ModelDialogueItem {
        id: None,
        role: Role::Assistant,
        content: vec![ContentPart::ToolUse {
            call: ToolCall::new(id, name, json!({})),
        }],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    }
}

fn assistant_tool_use_with_provider_metadata() -> ModelDialogueItem {
    let mut call = ToolCall::new("tc_gemini_1", "get_weather", json!({"city": "Tokyo"}));
    call.metadata.insert(
        "extra_content".into(),
        json!({"thought_signature": "gemini_sig_abc"}),
    );
    ModelDialogueItem {
        id: None,
        role: Role::Assistant,
        content: vec![ContentPart::ToolUse { call }],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    }
}

fn assistant_tool_use_from_flat_qwenpaw_wire() -> ModelDialogueItem {
    serde_json::from_value(json!({
        "role": "assistant",
        "content": [{
            "type": "tool_use",
            "id": "tc_flat_1",
            "name": "search",
            "input": {"query": "rua"},
            "extra_content": {
                "thought_signature": "flat_sig_123"
            }
        }]
    }))
    .expect("flat QwenPaw-style assistant message should deserialize")
}

fn assistant_multi_tool_use_with_provider_metadata() -> ModelDialogueItem {
    let mut call_a = ToolCall::new("tc_a", "fn_a", json!({}));
    call_a.metadata.insert(
        "extra_content".into(),
        json!({"thought_signature": "sig_a"}),
    );
    let mut call_b = ToolCall::new("tc_b", "fn_b", json!({"value": 2}));
    call_b.metadata.insert(
        "extra_content".into(),
        json!({"thought_signature": "sig_b"}),
    );

    ModelDialogueItem {
        id: None,
        role: Role::Assistant,
        content: vec![
            ContentPart::ToolUse { call: call_a },
            ContentPart::ToolUse { call: call_b },
        ],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    }
}

fn assistant_with_duplicate_tool_use_ids() -> ModelDialogueItem {
    ModelDialogueItem {
        id: None,
        role: Role::Assistant,
        content: vec![
            ContentPart::ToolUse {
                call: ToolCall::new("tc_dup", "search", json!({"query": "first"})),
            },
            ContentPart::ToolUse {
                call: ToolCall::new("tc_dup", "search", json!({"query": "duplicate"})),
            },
            ContentPart::ToolUse {
                call: ToolCall::new("tc_other", "read_file", json!({"path": "README.md"})),
            },
        ],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    }
}

fn assistant_with_invalid_and_valid_tool_uses() -> ModelDialogueItem {
    ModelDialogueItem {
        id: None,
        role: Role::Assistant,
        content: vec![
            ContentPart::ToolUse {
                call: ToolCall::new("", "Bash", json!({"command": "pwd"})),
            },
            ContentPart::ToolUse {
                call: ToolCall::new("tc_missing_name", "", json!({})),
            },
            ContentPart::ToolUse {
                call: ToolCall::new("tc_valid", "Read", json!({"path": "README.md"})),
            },
        ],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    }
}

fn tool_message_with_invalid_and_valid_results() -> ModelDialogueItem {
    ModelDialogueItem {
        id: None,
        role: Role::Tool,
        content: vec![
            ContentPart::ToolResult {
                result: ToolResult::text("", "orphan result"),
            },
            ContentPart::ToolResult {
                result: ToolResult {
                    name: Some("Read".into()),
                    ..ToolResult::text("tc_valid", "ok")
                },
            },
        ],
        name: Some("Read".into()),
        created_at: None,
        metadata: serde_json::Map::new(),
    }
}

fn assistant_tool_use_with_empty_input_and_raw_arguments() -> ModelDialogueItem {
    let mut call = ToolCall::new("tc_raw_1", "search", json!({}));
    call.raw_arguments = Some(r#"{"query":"hello"}"#.into());

    ModelDialogueItem {
        id: None,
        role: Role::Assistant,
        content: vec![ContentPart::ToolUse { call }],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    }
}

fn assistant_tool_use_with_empty_input_and_unusable_raw_arguments(raw: &str) -> ModelDialogueItem {
    let mut call = ToolCall::new("tc_raw_bad", "search", json!({}));
    call.raw_arguments = Some(raw.into());

    ModelDialogueItem {
        id: None,
        role: Role::Assistant,
        content: vec![ContentPart::ToolUse { call }],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    }
}

fn tool_result_with_media() -> ModelDialogueItem {
    let result = ToolResult {
        call_id: "tc_view_1".into(),
        name: Some("view_image".into()),
        content: vec![
            ContentPart::Text {
                text: "Rendered image:".into(),
            },
            ContentPart::Image {
                url: "https://cdn.example.test/demo.png".into(),
                media_type: Some("image/png".into()),
                alt: None,
            },
        ],
        is_error: false,
        duration_ms: None,
        attachments: None,
        truncation: None,
        metadata: serde_json::Map::new(),
    };
    ModelDialogueItem {
        id: None,
        role: Role::Tool,
        content: vec![ContentPart::ToolResult { result }],
        name: Some("view_image".into()),
        created_at: None,
        metadata: serde_json::Map::new(),
    }
}

fn tool_result_with_flat_qwenpaw_source_media() -> ModelDialogueItem {
    serde_json::from_value(json!({
        "role": "tool",
        "name": "view_image",
        "content": [{
            "type": "tool_result",
            "id": "tc_view_flat",
            "name": "view_image",
            "output": [
                {"type": "text", "text": "Rendered image:"},
                {
                    "type": "image",
                    "source": {
                        "type": "url",
                        "url": "https://cdn.example.test/flat.png",
                        "media_type": "image/png"
                    }
                }
            ]
        }]
    }))
    .expect("flat QwenPaw-style tool result message should deserialize")
}

fn mixed_multimodal_user_message() -> ModelDialogueItem {
    ModelDialogueItem {
        id: None,
        role: Role::User,
        content: vec![
            ContentPart::Text {
                text: "Look at this:".into(),
            },
            ContentPart::Image {
                url: "https://cdn.example.test/input.png".into(),
                media_type: Some("image/png".into()),
                alt: None,
            },
            ContentPart::Video {
                url: "https://cdn.example.test/input.mp4".into(),
                media_type: Some("video/mp4".into()),
            },
        ],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    }
}

fn assistant_with_thinking(signature: Option<&str>) -> ModelDialogueItem {
    ModelDialogueItem {
        id: None,
        role: Role::Assistant,
        content: vec![
            ContentPart::Thinking {
                text: "Let me consider the request.".into(),
                signature: signature.map(str::to_owned),
            },
            ContentPart::Text {
                text: "Here is the answer.".into(),
            },
        ],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    }
}

fn provider_specific_metadata_does_not_leak(body: &Value) {
    let encoded = serde_json::to_string(body).expect("json encodes");
    assert!(!encoded.contains("extra_content"));
    assert!(!encoded.contains("thought_signature"));
    assert!(!encoded.contains("gemini_sig_abc"));
}

#[test]
fn openai_compatible_converter_downgrades_developer_role_to_system() {
    let messages = openai_provider().convert_messages(&[ModelDialogueItem::text(
        Role::Developer,
        "Application instructions",
    )]);

    assert_eq!(messages[0]["role"], "system");
}

#[test]
fn openai_instruction_sections_merge_into_one_leading_system_message() {
    let mut application = ModelDialogueItem::text(Role::Developer, "Project instructions");
    application.content.push(ContentPart::Text {
        text: "Execution environment".into(),
    });
    let request = ChatRequest::new(vec![
        ModelDialogueItem::text(Role::User, "First question"),
        application,
        ModelDialogueItem::text(Role::System, "Core instructions"),
        ModelDialogueItem::text(Role::Assistant, "Answer"),
        ModelDialogueItem::text(Role::Developer, "Collaboration mode"),
        ModelDialogueItem::text(Role::System, "Additional core instructions"),
        ModelDialogueItem::text(Role::Developer, ""),
        ModelDialogueItem::text(Role::User, "Follow-up"),
    ])
    .with_model("test");
    let original = request.messages.clone();
    let responses = llm_multiplex::OpenAiResponsesProvider::new(OpenAiCompatibleConfig {
        provider_id: "responses".into(),
        provider_name: "Responses".into(),
        base_url: "https://llm.example.test/v1".into(),
        api_key: None,
        api_keys: Vec::new(),
        headers: Default::default(),
        models: Vec::new(),
        timeout_secs: 30,
        generate_kwargs: Default::default(),
        support_model_discovery: false,
        support_connection_check: false,
        is_local: false,
    })
    .unwrap();

    for (body, key) in [
        (openai_provider().build_request_body(&request), "messages"),
        (responses.build_request_body(&request), "input"),
    ] {
        assert_eq!(
            body[key],
            json!([
                {"role": "system", "content": "Core instructions\n\nAdditional core instructions\n\nProject instructions\n\nExecution environment\n\nCollaboration mode"},
                {"role": "user", "content": "First question"},
                {"role": "assistant", "content": "Answer"},
                {"role": "user", "content": "Follow-up"},
            ])
        );
    }
    assert_eq!(request.messages, original);
}

#[test]
fn openai_converter_does_not_add_system_message_without_instructions() {
    let messages = openai_provider().convert_messages(&[
        ModelDialogueItem::text(Role::User, "Question"),
        ModelDialogueItem::text(Role::Assistant, "Answer"),
    ]);
    assert_eq!(
        messages,
        json!([
            {"role": "user", "content": "Question"},
            {"role": "assistant", "content": "Answer"},
        ])
        .as_array()
        .unwrap()
        .clone()
    );
}

#[test]
fn openai_instruction_merge_preserves_non_text_content_parts() {
    let instructions = ModelDialogueItem {
        content: vec![
            ContentPart::Text {
                text: "Before".into(),
            },
            ContentPart::Image {
                url: "https://llm.example.test/instructions.png".into(),
                media_type: Some("image/png".into()),
                alt: None,
            },
            ContentPart::Text {
                text: "After".into(),
            },
        ],
        ..ModelDialogueItem::text(Role::System, "")
    };
    let messages = openai_provider().convert_messages(&[
        instructions,
        ModelDialogueItem::text(Role::Developer, "Application"),
        ModelDialogueItem::text(Role::User, "Question"),
    ]);
    assert_eq!(
        messages[0],
        json!({
            "role": "system",
            "content": [
                {"type": "text", "text": "Before"},
                {"type": "image_url", "image_url": {"url": "https://llm.example.test/instructions.png"}},
                {"type": "text", "text": "After\n\nApplication"},
            ],
        })
    );
    assert_eq!(messages.len(), 2);
}

#[test]
fn anthropic_converter_drops_unsigned_thinking_but_keeps_signed_thinking() {
    let unsigned_message = assistant_with_thinking(None);
    let unsigned = anthropic_provider().convert_message(&unsigned_message);
    assert_eq!(
        unsigned["content"],
        json!([{"type": "text", "text": "Here is the answer."}]),
        "Anthropic rejects thinking blocks that do not carry a provider signature"
    );

    let empty_signature_message = assistant_with_thinking(Some(""));
    let empty_signature = anthropic_provider().convert_message(&empty_signature_message);
    assert_eq!(
        empty_signature["content"],
        json!([{"type": "text", "text": "Here is the answer."}]),
        "An empty signature is also unsigned and must not reach Anthropic"
    );

    let signed_message = assistant_with_thinking(Some("sig-from-anthropic"));
    let signed = anthropic_provider().convert_message(&signed_message);
    assert_eq!(
        signed["content"][0],
        json!({
            "type": "thinking",
            "thinking": "Let me consider the request.",
            "signature": "sig-from-anthropic"
        })
    );
    assert_eq!(
        signed["content"][1],
        json!({"type": "text", "text": "Here is the answer."})
    );

    let openai = openai_provider().convert_messages(&[assistant_with_thinking(None)]);
    assert_eq!(
        openai[0]["content"],
        json!("Here is the answer."),
        "OpenAI-compatible adapters must keep visible assistant text in content"
    );
    assert_eq!(
        openai[0]["reasoning_content"],
        json!("Let me consider the request."),
        "OpenAI-compatible adapters must preserve thinking in reasoning_content"
    );
}

#[test]
fn provider_converters_isolate_tool_call_metadata_by_provider_without_mutating_history() {
    let request = ChatRequest::new(vec![
        ModelDialogueItem::text(Role::User, "Find the weather in Tokyo"),
        assistant_tool_use_with_provider_metadata(),
    ])
    .with_model("provider-test-model");
    let original = request.clone();

    let openai_body = openai_provider().build_request_body(&request);
    provider_specific_metadata_does_not_leak(&openai_body);
    assert_eq!(
        openai_body["messages"][1]["tool_calls"][0]["function"]["name"],
        "get_weather"
    );
    assert_eq!(
        openai_body["messages"][1]["tool_calls"][0]["function"]["arguments"],
        r#"{"city":"Tokyo"}"#
    );

    let anthropic_body = anthropic_provider().build_request_body(&request);
    provider_specific_metadata_does_not_leak(&anthropic_body);
    assert_eq!(
        anthropic_body["messages"][1]["content"][0],
        json!({
            "type": "tool_use",
            "id": "tc_gemini_1",
            "name": "get_weather",
            "input": {"city": "Tokyo"}
        })
    );

    let gemini_body = gemini_provider().build_request_body(&request, "provider-test-model");
    assert_eq!(
        gemini_body["contents"][1]["parts"][0],
        json!({
            "functionCall": {
                "name": "get_weather",
                "args": {"city": "Tokyo"}
            },
            "thoughtSignature": "gemini_sig_abc"
        })
    );

    assert_eq!(request.messages, original.messages);
}

#[test]
fn provider_converters_restore_flat_qwenpaw_metadata_only_for_gemini() {
    let message = assistant_tool_use_from_flat_qwenpaw_wire();
    let ContentPart::ToolUse { call } = &message.content[0] else {
        panic!("flat wire should become typed tool use");
    };
    assert_eq!(
        call.metadata
            .get("extra_content")
            .and_then(|value| value.get("thought_signature"))
            .and_then(|value| value.as_str()),
        Some("flat_sig_123"),
        "agent-types should preserve provider-private metadata before projection"
    );

    let request = ChatRequest::new(vec![message]).with_model("provider-test-model");
    let original = request.clone();

    let openai_body = openai_provider().build_request_body(&request);
    provider_specific_metadata_does_not_leak(&openai_body);
    assert_eq!(
        openai_body["messages"][0]["tool_calls"][0]["function"]["arguments"],
        r#"{"query":"rua"}"#
    );

    let anthropic_body = anthropic_provider().build_request_body(&request);
    provider_specific_metadata_does_not_leak(&anthropic_body);
    assert_eq!(
        anthropic_body["messages"][0]["content"][0],
        json!({
            "type": "tool_use",
            "id": "tc_flat_1",
            "name": "search",
            "input": {"query": "rua"}
        })
    );

    let gemini_body = gemini_provider().build_request_body(&request, "provider-test-model");
    assert_eq!(
        gemini_body["contents"][0]["parts"][0],
        json!({
            "functionCall": {
                "name": "search",
                "args": {"query": "rua"}
            },
            "thoughtSignature": "flat_sig_123"
        })
    );

    assert_eq!(request.messages, original.messages);
}

#[test]
fn provider_converters_accept_python_string_content_messages() {
    let user: ModelDialogueItem = serde_json::from_value(json!({
        "role": "user",
        "content": "Say hello"
    }))
    .expect("Python-style string user content should deserialize");
    let assistant: ModelDialogueItem = serde_json::from_value(json!({
        "role": "assistant",
        "content": "Hello!"
    }))
    .expect("Python-style string assistant content should deserialize");

    let request = ChatRequest::new(vec![user, assistant]).with_model("provider-test-model");
    let original = request.clone();

    let openai_body = openai_provider().build_request_body(&request);
    assert_eq!(openai_body["messages"][0]["content"], "Say hello");
    assert_eq!(openai_body["messages"][1]["content"], "Hello!");

    let anthropic_body = anthropic_provider().build_request_body(&request);
    assert_eq!(
        anthropic_body["messages"][0]["content"],
        json!([{"type": "text", "text": "Say hello"}])
    );
    assert_eq!(
        anthropic_body["messages"][1]["content"],
        json!([{"type": "text", "text": "Hello!"}])
    );

    let gemini_body = gemini_provider().build_request_body(&request, "provider-test-model");
    assert_eq!(gemini_body["contents"][0]["parts"][0]["text"], "Say hello");
    assert_eq!(gemini_body["contents"][1]["parts"][0]["text"], "Hello!");

    assert_eq!(request.messages, original.messages);
}

#[test]
fn provider_converters_repair_empty_tool_input_from_raw_arguments_without_mutating_history() {
    let request = ChatRequest::new(vec![assistant_tool_use_with_empty_input_and_raw_arguments()])
        .with_model("provider-test-model");
    let original = request.clone();

    let openai_body = openai_provider().build_request_body(&request);
    assert_eq!(
        openai_body["messages"][0]["tool_calls"][0]["function"]["arguments"],
        r#"{"query":"hello"}"#
    );

    let anthropic_body = anthropic_provider().build_request_body(&request);
    assert_eq!(
        anthropic_body["messages"][0]["content"][0],
        json!({
            "type": "tool_use",
            "id": "tc_raw_1",
            "name": "search",
            "input": {"query": "hello"}
        })
    );

    let gemini_body = gemini_provider().build_request_body(&request, "provider-test-model");
    assert_eq!(
        gemini_body["contents"][0]["parts"][0],
        json!({
            "functionCall": {
                "name": "search",
                "args": {"query": "hello"}
            }
        })
    );

    assert_eq!(request.messages, original.messages);
}

#[test]
fn provider_converters_keep_empty_input_when_raw_arguments_are_unusable() {
    for raw in ["", "{}", "not json"] {
        let request = ChatRequest::new(vec![
            assistant_tool_use_with_empty_input_and_unusable_raw_arguments(raw),
        ])
        .with_model("provider-test-model");
        let original = request.clone();

        let openai_body = openai_provider().build_request_body(&request);
        assert_eq!(
            openai_body["messages"][0]["tool_calls"][0]["function"]["arguments"], "{}",
            "OpenAI-compatible projection should fall back to typed empty input for raw={raw:?}"
        );

        let anthropic_body = anthropic_provider().build_request_body(&request);
        assert_eq!(
            anthropic_body["messages"][0]["content"][0],
            json!({
                "type": "tool_use",
                "id": "tc_raw_bad",
                "name": "search",
                "input": {}
            }),
            "Anthropic projection should not repair from raw={raw:?}"
        );

        let gemini_body = gemini_provider().build_request_body(&request, "provider-test-model");
        assert_eq!(
            gemini_body["contents"][0]["parts"][0],
            json!({
                "functionCall": {
                    "name": "search",
                    "args": {}
                }
            }),
            "Gemini projection should not repair from raw={raw:?}"
        );

        assert_eq!(request.messages, original.messages);
    }
}

#[test]
fn provider_converters_isolate_metadata_for_every_tool_call_in_message() {
    let request = ChatRequest::new(vec![assistant_multi_tool_use_with_provider_metadata()])
        .with_model("provider-test-model");
    let original = request.clone();

    let openai_body = openai_provider().build_request_body(&request);
    provider_specific_metadata_does_not_leak(&openai_body);
    let encoded_openai = serde_json::to_string(&openai_body).expect("json encodes");
    assert!(!encoded_openai.contains("sig_a"));
    assert!(!encoded_openai.contains("sig_b"));
    let openai_calls = openai_body["messages"][0]["tool_calls"]
        .as_array()
        .expect("openai tool calls");
    assert_eq!(openai_calls.len(), 2);
    assert_eq!(openai_calls[0]["function"]["name"], "fn_a");
    assert_eq!(openai_calls[1]["function"]["name"], "fn_b");

    let anthropic_body = anthropic_provider().build_request_body(&request);
    provider_specific_metadata_does_not_leak(&anthropic_body);
    let encoded_anthropic = serde_json::to_string(&anthropic_body).expect("json encodes");
    assert!(!encoded_anthropic.contains("sig_a"));
    assert!(!encoded_anthropic.contains("sig_b"));
    assert_eq!(
        anthropic_body["messages"][0]["content"],
        json!([
            {"type": "tool_use", "id": "tc_a", "name": "fn_a", "input": {}},
            {"type": "tool_use", "id": "tc_b", "name": "fn_b", "input": {"value": 2}}
        ])
    );

    let gemini_body = gemini_provider().build_request_body(&request, "provider-test-model");
    assert_eq!(
        gemini_body["contents"][0]["parts"],
        json!([
            {
                "functionCall": {"name": "fn_a", "args": {}},
                "thoughtSignature": "sig_a"
            },
            {
                "functionCall": {"name": "fn_b", "args": {"value": 2}},
                "thoughtSignature": "sig_b"
            }
        ])
    );

    assert_eq!(request.messages, original.messages);
}

#[test]
fn provider_converters_skip_invalid_tool_blocks_without_mutating_history() {
    let request = ChatRequest::new(vec![
        assistant_with_invalid_and_valid_tool_uses(),
        tool_message_with_invalid_and_valid_results(),
    ])
    .with_model("provider-test-model");
    let original = request.clone();

    let openai_body = openai_provider().build_request_body(&request);
    let openai_messages = openai_body["messages"].as_array().expect("messages");
    assert_eq!(
        openai_messages[0]["tool_calls"],
        json!([{
            "id": "tc_valid",
            "type": "function",
            "function": {
                "name": "Read",
                "arguments": r#"{"path":"README.md"}"#
            }
        }])
    );
    assert_eq!(openai_messages[1]["tool_call_id"], "tc_valid");
    assert_eq!(openai_messages[1]["content"], "ok");

    let anthropic_body = anthropic_provider().build_request_body(&request);
    assert_eq!(
        anthropic_body["messages"][0]["content"],
        json!([{
            "type": "tool_use",
            "id": "tc_valid",
            "name": "Read",
            "input": {"path": "README.md"}
        }])
    );
    assert_eq!(
        anthropic_body["messages"][1]["content"],
        json!([{
            "type": "tool_result",
            "tool_use_id": "tc_valid",
            "content": [{"type": "text", "text": "ok"}],
            "is_error": false
        }])
    );

    let gemini_body = gemini_provider().build_request_body(&request, "provider-test-model");
    assert_eq!(
        gemini_body["contents"][0]["parts"],
        json!([{
            "functionCall": {
                "name": "Read",
                "args": {"path": "README.md"}
            }
        }])
    );
    assert_eq!(
        gemini_body["contents"][1]["parts"],
        json!([{
            "functionResponse": {
                "name": "Read",
                "response": {"result": "ok"}
            }
        }])
    );

    for encoded in [
        serde_json::to_string(&openai_body).expect("openai body encodes"),
        serde_json::to_string(&anthropic_body).expect("anthropic body encodes"),
        serde_json::to_string(&gemini_body).expect("gemini body encodes"),
    ] {
        assert!(!encoded.contains("tc_missing_name"));
        assert!(!encoded.contains("orphan result"));
    }
    assert_eq!(request.messages, original.messages);
}

#[test]
fn provider_converters_drop_duplicate_tool_use_ids_without_mutating_history() {
    let request = ChatRequest::new(vec![assistant_with_duplicate_tool_use_ids()])
        .with_model("provider-test-model");
    let original = request.clone();

    let openai_body = openai_provider().build_request_body(&request);
    let openai_calls = openai_body["messages"][0]["tool_calls"]
        .as_array()
        .expect("openai tool calls");
    assert_eq!(openai_calls.len(), 2);
    assert_eq!(openai_calls[0]["id"], "tc_dup");
    assert_eq!(
        openai_calls[0]["function"]["arguments"], r#"{"query":"first"}"#,
        "first valid duplicate id wins"
    );
    assert_eq!(openai_calls[1]["id"], "tc_other");

    let anthropic_body = anthropic_provider().build_request_body(&request);
    assert_eq!(
        anthropic_body["messages"][0]["content"],
        json!([
            {"type": "tool_use", "id": "tc_dup", "name": "search", "input": {"query": "first"}},
            {"type": "tool_use", "id": "tc_other", "name": "read_file", "input": {"path": "README.md"}}
        ])
    );

    let gemini_body = gemini_provider().build_request_body(&request, "provider-test-model");
    assert_eq!(
        gemini_body["contents"][0]["parts"],
        json!([
            {"functionCall": {"name": "search", "args": {"query": "first"}}},
            {"functionCall": {"name": "read_file", "args": {"path": "README.md"}}}
        ])
    );

    assert_eq!(
        request.messages, original.messages,
        "provider request sanitization must not mutate stored history"
    );
    assert_eq!(
        request.messages[0].tool_calls().len(),
        3,
        "the stored typed message keeps all original blocks for audit/repair"
    );
}

#[test]
fn multimodal_blocks_are_converted_by_provider_contract_without_mutating_history() {
    let request = ChatRequest::new(vec![mixed_multimodal_user_message()]);
    let original = request.clone();

    let openai_messages = openai_provider().convert_messages(&request.messages);
    assert_eq!(
        openai_messages[0]["content"][0],
        json!({"type": "text", "text": "Look at this:"})
    );
    assert_eq!(
        openai_messages[0]["content"][1],
        json!({
            "type": "image_url",
            "image_url": {"url": "https://cdn.example.test/input.png"}
        })
    );
    assert_eq!(
        openai_messages[0]["content"][2],
        json!({
            "type": "video_url",
            "video_url": {"url": "https://cdn.example.test/input.mp4"}
        })
    );

    let anthropic_message = anthropic_provider().convert_message(&request.messages[0]);
    assert_eq!(
        anthropic_message["content"][1],
        json!({
            "type": "image",
            "source": {
                "type": "url",
                "url": "https://cdn.example.test/input.png"
            }
        })
    );
    assert!(
        anthropic_message["content"]
            .as_array()
            .expect("anthropic content array")
            .iter()
            .all(|part| part.get("type").and_then(Value::as_str) != Some("video")),
        "Anthropic adapter does not expose video blocks directly"
    );

    let gemini_body = gemini_provider().build_request_body(&request, "gemini-test");
    assert_eq!(
        gemini_body["contents"][0]["parts"][1],
        json!({
            "fileData": {
                "mimeType": "image/png",
                "fileUri": "https://cdn.example.test/input.png"
            }
        })
    );
    assert_eq!(
        gemini_body["contents"][0]["parts"][2],
        json!({
            "fileData": {
                "mimeType": "video/mp4",
                "fileUri": "https://cdn.example.test/input.mp4"
            }
        })
    );

    assert_eq!(request.messages, original.messages);
}

#[test]
fn tool_result_media_uses_rust_provider_specific_projection() {
    let request = ChatRequest::new(vec![
        assistant_tool_call("tc_view_1", "view_image"),
        tool_result_with_media(),
    ]);
    let original = request.clone();

    let openai_messages = openai_provider().convert_messages(&request.messages);
    assert_eq!(openai_messages.len(), 3);
    assert_eq!(openai_messages[1]["role"], "tool");
    assert_eq!(openai_messages[1]["content"], "Rendered image:");
    assert_eq!(openai_messages[1]["tool_call_id"], "tc_view_1");
    assert_eq!(openai_messages[2]["role"], "user");
    assert_eq!(
        openai_messages[2]["content"][1],
        json!({
            "type": "image_url",
            "image_url": {"url": "https://cdn.example.test/demo.png"}
        })
    );

    let anthropic_message = anthropic_provider().convert_message(&request.messages[1]);
    assert_eq!(anthropic_message["role"], "user");
    assert_eq!(anthropic_message["content"][0]["type"], "tool_result");
    assert_eq!(
        anthropic_message["content"][0]["content"][1],
        json!({
            "type": "image",
            "source": {
                "type": "url",
                "url": "https://cdn.example.test/demo.png"
            }
        })
    );

    let gemini_body = gemini_provider().build_request_body(&request, "gemini-test");
    assert_eq!(
        gemini_body["contents"][1]["parts"][0],
        json!({
            "functionResponse": {
                "name": "view_image",
                "response": {
                    "result": "Rendered image:\n[Image: https://cdn.example.test/demo.png]"
                }
            }
        })
    );

    assert_eq!(
        request.messages, original.messages,
        "provider-specific media projection must not mutate stored tool-result history"
    );
}

#[test]
fn flat_qwenpaw_tool_result_media_reaches_provider_projection_without_mutation() {
    let request = ChatRequest::new(vec![
        assistant_tool_call("tc_view_flat", "view_image"),
        tool_result_with_flat_qwenpaw_source_media(),
    ]);
    let original = request.clone();

    let ContentPart::ToolResult { result } = &request.messages[1].content[0] else {
        panic!("flat wire should become typed tool result");
    };
    assert_eq!(result.content.len(), 2);
    assert_eq!(
        result.content[1],
        ContentPart::Image {
            url: "https://cdn.example.test/flat.png".into(),
            media_type: Some("image/png".into()),
            alt: None,
        }
    );

    let openai_body = openai_provider().build_request_body(&request);
    let openai_messages = openai_body["messages"].as_array().expect("openai messages");
    assert_eq!(openai_messages[1]["role"], "tool");
    assert_eq!(openai_messages[1]["content"], "Rendered image:");
    assert_eq!(
        openai_messages[2]["content"][1],
        json!({
            "type": "image_url",
            "image_url": {"url": "https://cdn.example.test/flat.png"}
        })
    );

    let anthropic_message = anthropic_provider().convert_message(&request.messages[1]);
    assert_eq!(
        anthropic_message["content"][0]["content"][1]["type"],
        "image"
    );
    assert_eq!(
        anthropic_message["content"][0]["content"][1]["source"]["url"],
        "https://cdn.example.test/flat.png"
    );

    let gemini_body = gemini_provider().build_request_body(&request, "provider-test-model");
    assert_eq!(
        gemini_body["contents"][1]["parts"][0],
        json!({
            "functionResponse": {
                "name": "view_image",
                "response": {
                    "result": "Rendered image:\n[Image: https://cdn.example.test/flat.png]"
                }
            }
        })
    );

    assert_eq!(request.messages, original.messages);
}

#[test]
fn provider_converters_repair_order_missing_results_and_orphans_without_mutating_history() {
    let first = ToolCall::new("call-a", "read", json!({"path": "README.md"}));
    let second = ToolCall::new("call-b", "glob", json!({"pattern": "docs/**"}));
    let third = ToolCall::new("call-c", "grep", json!({"pattern": "ADR"}));
    let assistant = ModelDialogueItem {
        id: None,
        role: Role::Assistant,
        content: vec![
            ContentPart::ToolUse {
                call: first.clone(),
            },
            ContentPart::ToolUse {
                call: second.clone(),
            },
            ContentPart::ToolUse {
                call: third.clone(),
            },
        ],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    };
    let result_message = |id: &str, name: &str, output: &str| ModelDialogueItem {
        id: None,
        role: Role::Tool,
        content: vec![ContentPart::ToolResult {
            result: ToolResult {
                name: Some(name.to_string()),
                ..ToolResult::text(id, output)
            },
        }],
        name: Some(name.to_string()),
        created_at: None,
        metadata: serde_json::Map::new(),
    };
    let request = ChatRequest::new(vec![
        assistant,
        result_message("call-b", "glob", "glob output"),
        result_message("orphan", "unused", "orphan output"),
        result_message("call-a", "read", "read output"),
    ])
    .with_model("provider-test-model");
    let original = request.clone();

    let openai_body = openai_provider().build_request_body(&request);
    let openai_messages = openai_body["messages"].as_array().expect("OpenAI messages");
    assert_eq!(openai_messages.len(), 4);
    assert_eq!(
        openai_messages[0]["tool_calls"]
            .as_array()
            .expect("tool calls")
            .iter()
            .map(|call| call["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["call-a", "call-b", "call-c"]
    );
    assert_eq!(
        openai_messages[1..]
            .iter()
            .map(|message| message["tool_call_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["call-a", "call-b", "call-c"]
    );
    assert!(openai_messages[3]["content"]
        .as_str()
        .unwrap()
        .contains("interrupted"));
    assert!(!serde_json::to_string(&openai_body)
        .unwrap()
        .contains("orphan output"));

    let anthropic_body = anthropic_provider().build_request_body(&request);
    let anthropic_result_ids = anthropic_body["messages"]
        .as_array()
        .expect("Anthropic messages")
        .iter()
        .flat_map(|message| {
            message["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|part| part["tool_use_id"].as_str())
        })
        .collect::<Vec<_>>();
    assert_eq!(anthropic_result_ids, vec!["call-a", "call-b", "call-c"]);

    let gemini_body = gemini_provider().build_request_body(&request, "provider-test-model");
    let gemini_responses = gemini_body["contents"][1]["parts"]
        .as_array()
        .expect("Gemini function responses");
    assert_eq!(
        gemini_responses
            .iter()
            .map(|part| part["functionResponse"]["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["read", "glob", "grep"]
    );
    assert!(
        gemini_responses[2]["functionResponse"]["response"]["result"]
            .as_str()
            .unwrap()
            .contains("interrupted")
    );

    assert_eq!(request.messages, original.messages);
}

#[test]
fn interleaved_runtime_observations_preserve_real_results_across_providers() {
    let mut wait = assistant_tool_call("wait-local", "task_await");
    if let ContentPart::ToolUse { call } = &mut wait.content[0] {
        call.provider_call_id = Some("wait-wire".into());
    }
    let mut note = assistant_tool_call("note-1", "Task_Notification");
    note.metadata
        .insert("runtime_generated".into(), json!(true));
    let result = |id: &str, text: &str, failed: bool| ModelDialogueItem {
        content: vec![ContentPart::ToolResult {
            result: if failed {
                ToolResult::error(id, text)
            } else {
                ToolResult::text(id, text)
            },
        }],
        metadata: if id == "note-1" {
            [("runtime_generated".into(), json!(true))]
                .into_iter()
                .collect()
        } else {
            Default::default()
        },
        ..ModelDialogueItem::text(Role::Tool, "")
    };
    for failed in [false, true] {
        let request = ChatRequest::new(vec![
            wait.clone(),
            note.clone(),
            result("note-1", "terminal notification", false),
            result("wait-local", "actual await outcome", failed),
            ModelDialogueItem::text(Role::User, "next turn"),
        ])
        .with_model("test");
        let original = request.messages.clone();
        let responses =
            llm_multiplex::OpenAiResponsesProvider::new(llm_multiplex::OpenAiCompatibleConfig {
                provider_id: "responses".into(),
                provider_name: "Responses".into(),
                base_url: "http://127.0.0.1:9/v1".into(),
                api_key: None,
                api_keys: vec![],
                headers: Default::default(),
                models: vec![],
                timeout_secs: 1,
                generate_kwargs: Default::default(),
                support_model_discovery: false,
                support_connection_check: false,
                is_local: true,
            })
            .unwrap();
        let ollama = llm_multiplex::OllamaProvider::new(Default::default()).unwrap();
        for (name, body) in [
            ("openai", openai_provider().build_request_body(&request)),
            ("responses", responses.build_request_body(&request)),
            (
                "anthropic",
                anthropic_provider().build_request_body(&request),
            ),
            (
                "gemini",
                gemini_provider().build_request_body(&request, "test"),
            ),
            ("ollama", ollama.build_request_body(&request, "test")),
        ] {
            let wire = body.to_string();
            assert_eq!(
                wire.matches("actual await outcome").count(),
                1,
                "{name}: {wire}"
            );
            assert_eq!(
                wire.matches("terminal notification").count(),
                1,
                "{name}: {wire}"
            );
            assert!(!wire.contains("interrupted"), "{name}: {wire}");
        }
        assert_eq!(request.messages, original);
    }
}

#[test]
fn runtime_observations_do_not_match_results_across_real_turn_boundaries() {
    let mut notification = assistant_tool_call("notification", "Task_Notification");
    notification
        .metadata
        .insert("runtime_generated".into(), json!(true));
    let output = |id: &str, text: &str, runtime_generated| ModelDialogueItem {
        content: vec![ContentPart::ToolResult {
            result: ToolResult::text(id, text),
        }],
        metadata: [("runtime_generated".into(), json!(runtime_generated))]
            .into_iter()
            .collect(),
        ..ModelDialogueItem::text(Role::Tool, "")
    };
    let request = ChatRequest::new(vec![
        assistant_tool_call("reused", "task_await"),
        notification,
        output("notification", "notification output", true),
        ModelDialogueItem::text(Role::User, "new turn"),
        assistant_tool_call("reused", "task_await"),
        output("reused", "new turn result", false),
    ]);
    let messages = llm_multiplex::request::normalize_tool_history(&request.messages);
    assert!(messages[1].tool_results()[0]
        .text_content()
        .contains("interrupted"));
    assert_eq!(
        messages[3].tool_results()[0].text_content(),
        "notification output"
    );
    assert_eq!(messages[4].text_content(), "new turn");
    assert_eq!(
        messages[6].tool_results()[0].text_content(),
        "new turn result"
    );
    assert_eq!(
        llm_multiplex::request::normalize_tool_history(&messages),
        messages
    );
}

#[test]
fn runtime_observations_keep_completed_parallel_results_when_another_call_is_missing() {
    let mut calls = assistant_tool_call("done", "Task_Get");
    calls
        .content
        .extend(assistant_tool_call("pending", "task_await").content);
    let mut notification = assistant_tool_call("notification", "Task_Notification");
    notification
        .metadata
        .insert("runtime_generated".into(), json!(true));
    let notification_result = ModelDialogueItem {
        content: vec![ContentPart::ToolResult {
            result: ToolResult::text("notification", "async"),
        }],
        metadata: notification.metadata.clone(),
        ..ModelDialogueItem::text(Role::Tool, "")
    };
    let real_result = ToolResult::error("done", "actual failure");
    let messages = llm_multiplex::request::normalize_tool_history(&[
        calls,
        notification,
        notification_result,
        ModelDialogueItem {
            content: vec![ContentPart::ToolResult {
                result: real_result.clone(),
            }],
            ..ModelDialogueItem::text(Role::Tool, "")
        },
    ]);
    assert_eq!(messages[1].tool_results()[0].content, real_result.content);
    assert!(messages[1].tool_results()[0].is_error);
    assert!(messages[2].tool_results()[0]
        .text_content()
        .contains("interrupted"));
    assert_eq!(messages[4].tool_results()[0].text_content(), "async");
}
