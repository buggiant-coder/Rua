use agent_types::{
    AgentEvent, ChatRequest, ContentPart, ModelDialogueItem, ReasoningConfig, ReasoningMode,
    ReasoningSummary, Role, StructuredOutput, ToolCall, ToolResult,
};
use base64::Engine;
use llm_multiplex::event_parser::parse_gemini_chunk;
use llm_multiplex::providers::gemini::{GeminiConfig, GeminiProvider};
use serde_json::json;

fn provider_with_kwargs(
    generate_kwargs: serde_json::Map<String, serde_json::Value>,
) -> GeminiProvider {
    GeminiProvider::new(GeminiConfig {
        provider_id: "gemini".into(),
        provider_name: "Gemini".into(),
        base_url: "https://generativelanguage.googleapis.com".into(),
        api_key: Some("gem-test".into()),
        headers: serde_json::Map::new(),
        models: Vec::new(),
        timeout_secs: 30,
        generate_kwargs,
    })
    .expect("gemini provider")
}

fn user_message(text: &str) -> ModelDialogueItem {
    ModelDialogueItem {
        id: None,
        role: Role::User,
        content: vec![ContentPart::Text { text: text.into() }],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    }
}

fn assistant_with_thinking_and_text() -> ModelDialogueItem {
    ModelDialogueItem {
        id: None,
        role: Role::Assistant,
        content: vec![
            ContentPart::Thinking {
                text: "private reasoning".into(),
                signature: Some("gemini-thought-sig".into()),
            },
            ContentPart::Text {
                text: "visible answer".into(),
            },
        ],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    }
}

fn assistant_tool_use_with_raw_arguments() -> ModelDialogueItem {
    let mut call = ToolCall::new("tc-raw-1", "read_file", json!({}));
    call.raw_arguments = Some(r#"{"path":"/tmp/x"}"#.into());

    ModelDialogueItem {
        id: None,
        role: Role::Assistant,
        content: vec![ContentPart::ToolUse { call }],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    }
}

fn tool_result_message(call: &ToolCall, output: &str) -> ModelDialogueItem {
    let mut result = ToolResult::text(call.id.clone(), output);
    result.name = Some(call.name.clone());
    ModelDialogueItem {
        id: None,
        role: Role::Tool,
        content: vec![ContentPart::ToolResult { result }],
        name: Some(call.name.clone()),
        created_at: None,
        metadata: serde_json::Map::new(),
    }
}

#[test]
fn gemini_request_merges_parallel_tool_results_into_one_response_turn() {
    let first = ToolCall::new("call-1", "read", json!({"path": "README.md"}));
    let second = ToolCall::new("call-2", "glob", json!({"pattern": "docs/**"}));
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
        ],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    };
    let provider = provider_with_kwargs(serde_json::Map::new());
    let request = ChatRequest::new(vec![
        user_message("inspect project"),
        assistant,
        tool_result_message(&first, "read output"),
        tool_result_message(&second, "glob output"),
    ])
    .with_model("gemini-test");

    let body = provider.build_request_body(&request, "gemini-test");
    let contents = body["contents"].as_array().expect("Gemini contents");

    assert_eq!(contents.len(), 3);
    assert_eq!(contents[2]["role"], "user");
    let response_parts = contents[2]["parts"]
        .as_array()
        .expect("merged function response parts");
    assert_eq!(response_parts.len(), 2);
    assert_eq!(response_parts[0]["functionResponse"]["name"], "read");
    assert_eq!(response_parts[1]["functionResponse"]["name"], "glob");
}

#[test]
fn gemini_request_projects_tool_result_text_payload_without_rua_envelope() {
    let call = ToolCall::new("call-json", "task_await", json!({}));
    let request = ChatRequest::new(vec![
        ModelDialogueItem {
            id: None,
            role: Role::Assistant,
            content: vec![ContentPart::ToolUse { call: call.clone() }],
            name: None,
            created_at: None,
            metadata: serde_json::Map::new(),
        },
        tool_result_message(&call, r#"{"timed_out":true}"#),
    ]);

    let body = provider_with_kwargs(serde_json::Map::new()).build_request_body(&request, "gemini");

    assert_eq!(
        body["contents"][1]["parts"][0]["functionResponse"]["response"]["result"],
        r#"{"timed_out":true}"#
    );
}

#[test]
fn gemini_request_roundtrips_provider_call_ids_for_same_name_parallel_calls() {
    let mut first = ToolCall::new("internal-1", "bash", json!({"command": "pwd"}));
    first.provider_call_id = Some("provider-call-1".into());
    let mut second = ToolCall::new("internal-2", "bash", json!({"command": "ls"}));
    second.provider_call_id = Some("provider-call-2".into());
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
        ],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    };
    let provider = provider_with_kwargs(serde_json::Map::new());
    let request = ChatRequest::new(vec![
        assistant,
        tool_result_message(&first, "first output"),
        tool_result_message(&second, "second output"),
    ])
    .with_model("gemini-test");

    let body = provider.build_request_body(&request, "gemini-test");
    let calls = body["contents"][0]["parts"]
        .as_array()
        .expect("Gemini function calls");
    let responses = body["contents"][1]["parts"]
        .as_array()
        .expect("Gemini function responses");

    assert_eq!(calls[0]["functionCall"]["id"], "provider-call-1");
    assert_eq!(calls[1]["functionCall"]["id"], "provider-call-2");
    assert_eq!(responses[0]["functionResponse"]["id"], "provider-call-1");
    assert_eq!(responses[1]["functionResponse"]["id"], "provider-call-2");
}

#[test]
fn gemini_streamed_tool_signature_roundtrips_into_followup_request() {
    let chunk = json!({
        "candidates": [{
            "content": {
                "parts": [{
                    "functionCall": {
                        "name": "bash",
                        "args": {"command": "pwd"}
                    },
                    "thoughtSignature": "dGVzdC1zaWduYXR1cmU="
                }]
            },
            "finishReason": "STOP"
        }]
    })
    .to_string();
    let (events, _) = parse_gemini_chunk(&chunk).expect("Gemini chunk parses");
    let call = events
        .into_iter()
        .find_map(|event| match event {
            AgentEvent::ToolCall { call } => Some(call),
            _ => None,
        })
        .expect("tool call event");
    let assistant = ModelDialogueItem {
        id: None,
        role: Role::Assistant,
        content: vec![ContentPart::ToolUse { call: call.clone() }],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    };
    let mut result = ToolResult::text(call.id.clone(), "ok");
    result.name = Some(call.name.clone());
    let tool = ModelDialogueItem {
        id: None,
        role: Role::Tool,
        content: vec![ContentPart::ToolResult { result }],
        name: Some(call.name),
        created_at: None,
        metadata: serde_json::Map::new(),
    };
    let provider = provider_with_kwargs(serde_json::Map::new());
    let request =
        ChatRequest::new(vec![user_message("run pwd"), assistant, tool]).with_model("gemini-test");

    let body = provider.build_request_body(&request, "gemini-test");

    assert_eq!(
        body["contents"][1]["parts"][0]["thoughtSignature"], "dGVzdC1zaWduYXR1cmU=",
        "the provider-issued thought signature must be replayed with the function call"
    );
}

#[test]
fn gemini_request_recovers_legacy_signature_stored_as_tool_call_id() {
    let legacy_signature =
        base64::engine::general_purpose::STANDARD.encode("legacy Gemini signature".repeat(4));
    let call = ToolCall::new(legacy_signature.clone(), "bash", json!({"command": "pwd"}));
    let assistant = ModelDialogueItem {
        id: None,
        role: Role::Assistant,
        content: vec![ContentPart::ToolUse { call }],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    };
    let provider = provider_with_kwargs(serde_json::Map::new());
    let request =
        ChatRequest::new(vec![user_message("run pwd"), assistant]).with_model("gemini-test");

    let body = provider.build_request_body(&request, "gemini-test");

    assert_eq!(
        body["contents"][1]["parts"][0]["thoughtSignature"],
        legacy_signature
    );
}

#[test]
fn gemini_request_preserves_thinking_separately_from_visible_content() {
    let provider = provider_with_kwargs(serde_json::Map::new());
    let request = ChatRequest::new(vec![
        user_message("hello"),
        assistant_with_thinking_and_text(),
    ])
    .with_model("gemini-test");

    let body = provider.build_request_body(&request, "gemini-test");

    assert_eq!(body["contents"][1]["role"], "model");
    assert_eq!(
        body["contents"][1]["parts"],
        json!([
            {
                "text": "private reasoning",
                "thought": true,
                "thoughtSignature": "gemini-thought-sig"
            },
            {"text": "visible answer"}
        ])
    );
    let serialized = serde_json::to_string(&body).expect("body json");
    assert!(serialized.contains("private reasoning"));
}

#[test]
fn gemini_request_merges_generate_kwargs_into_generation_config() {
    let mut generate_kwargs = serde_json::Map::new();
    generate_kwargs.insert("temperature".into(), json!(0.2));
    generate_kwargs.insert("maxOutputTokens".into(), json!(4096));
    generate_kwargs.insert("topP".into(), json!(0.9));
    generate_kwargs.insert("thinkingConfig".into(), json!({"thinkingBudget": 128}));

    let provider = provider_with_kwargs(generate_kwargs);
    let request = ChatRequest::new(vec![user_message("hello")])
        .with_model("gemini-test")
        .with_temperature(0.7)
        .with_max_tokens(512);

    let body = provider.build_request_body(&request, "gemini-test");
    let generation = &body["generationConfig"];

    let temperature = generation["temperature"]
        .as_f64()
        .expect("temperature should be numeric");
    assert!((temperature - 0.7).abs() < 1e-6);
    assert_eq!(generation["maxOutputTokens"], 512);
    assert_eq!(generation["topP"], 0.9);
    assert_eq!(generation["thinkingConfig"], json!({"thinkingBudget": 128}));
}

#[test]
fn gemini_request_provider_options_override_generate_kwargs() {
    let mut generate_kwargs = serde_json::Map::new();
    generate_kwargs.insert("topP".into(), json!(0.9));
    generate_kwargs.insert("topK".into(), json!(40));
    generate_kwargs.insert("thinkingConfig".into(), json!({"thinkingBudget": 128}));

    let provider = provider_with_kwargs(generate_kwargs);
    let request = ChatRequest::new(vec![user_message("hello")])
        .with_model("gemini-test")
        .with_provider_option("topP", json!(0.4))
        .with_provider_option("thinkingConfig", json!({"thinkingBudget": 0}));

    let body = provider.build_request_body(&request, "gemini-test");
    let generation = &body["generationConfig"];

    assert_eq!(generation["topP"], 0.4);
    assert_eq!(generation["topK"], 40);
    assert_eq!(generation["thinkingConfig"], json!({"thinkingBudget": 0}));
}

#[test]
fn gemini_request_translates_canonical_budget_reasoning_options() {
    let provider = provider_with_kwargs(serde_json::Map::new());
    let request = ChatRequest::new(vec![user_message("hello")])
        .with_model("gemini-test")
        .with_provider_option("thinking_enabled", json!(true))
        .with_provider_option("thinking_budget", json!(8192));

    let body = provider.build_request_body(&request, "gemini-test");
    let generation = &body["generationConfig"];

    assert_eq!(
        generation["thinkingConfig"],
        json!({
            "includeThoughts": true,
            "thinkingBudget": 8192
        })
    );
    assert!(generation.get("thinking_enabled").is_none());
    assert!(generation.get("thinking_budget").is_none());
}

#[test]
fn gemini_protocol_translates_typed_reasoning() {
    let provider = provider_with_kwargs(serde_json::Map::new());
    let request = ChatRequest::new(vec![user_message("hello")])
        .with_model("gemini-test")
        .with_reasoning(ReasoningConfig {
            mode: ReasoningMode::Budget,
            enabled: true,
            effort: None,
            budget_tokens: Some(8192),
            summary: Some(ReasoningSummary::Auto),
        });

    let body = provider.build_request_body(&request, "gemini-test");

    assert_eq!(
        body["generationConfig"]["thinkingConfig"],
        json!({
            "includeThoughts": true,
            "thinkingBudget": 8192
        })
    );
}

#[test]
fn gemini_reasoning_level_clears_budget_and_respects_model_off_policy() {
    let provider = provider_with_kwargs(
        json!({"thinkingConfig":{"thinkingBudget":8192,"custom":true}})
            .as_object()
            .unwrap()
            .clone(),
    );
    let mut request =
        ChatRequest::new(vec![user_message("think")]).with_reasoning(ReasoningConfig {
            mode: ReasoningMode::Effort,
            enabled: true,
            effort: Some("high".into()),
            budget_tokens: None,
            summary: None,
        });
    let body = provider.build_request_body(&request, "gemini-3.1-pro-preview");
    assert_eq!(
        body["generationConfig"]["thinkingConfig"],
        json!({"thinkingLevel":"high","includeThoughts":true,"custom":true})
    );
    request.reasoning.as_mut().unwrap().enabled = false;
    request
        .metadata
        .insert("rua_reasoning_off_effort".into(), json!("low"));
    let body = provider.build_request_body(&request, "gemini-3.1-pro-preview");
    assert_eq!(
        body["generationConfig"]["thinkingConfig"],
        json!({"thinkingLevel":"low","includeThoughts":false,"custom":true})
    );
}

#[test]
fn gemini_reasoning_accepts_upstream_sdk_thinking_config() {
    let provider = provider_with_kwargs(serde_json::Map::new());
    let request = ChatRequest::new(vec![user_message("think")]).with_provider_option(
        "thinking_config",
        json!({"thinking_budget":2048,"include_thoughts":true}),
    );
    let body = provider.build_request_body(&request, "gemini-2.5-flash");
    assert_eq!(
        body["generationConfig"]["thinkingConfig"],
        json!({"thinkingBudget":2048,"includeThoughts":true})
    );
    assert!(body["generationConfig"].get("thinking_config").is_none());
}

#[test]
fn gemini_request_repairs_from_raw_arguments_without_leaking_raw_argument_fields() {
    let provider = provider_with_kwargs(serde_json::Map::new());
    let request = ChatRequest::new(vec![
        user_message("Read a file"),
        assistant_tool_use_with_raw_arguments(),
    ])
    .with_model("gemini-test");

    let body = provider.build_request_body(&request, "gemini-test");
    let function_call = &body["contents"][1]["parts"][0]["functionCall"];

    assert_eq!(function_call["args"], json!({"path": "/tmp/x"}));
    let serialized = serde_json::to_string(&body).expect("body json");
    assert!(!serialized.contains("raw_arguments"));
    assert!(!serialized.contains("raw_input"));
}

#[test]
fn gemini_structured_output_keeps_rust_json_schema_contract() {
    let mut generate_kwargs = serde_json::Map::new();
    generate_kwargs.insert("responseMimeType".into(), json!("text/plain"));
    generate_kwargs.insert("temperature".into(), json!(0.2));

    let provider = provider_with_kwargs(generate_kwargs);
    let mut structured = StructuredOutput::new(json!({
        "type": "object",
        "title": "Answer",
        "properties": {
            "answer": {"type": "string", "title": "Answer text"}
        },
        "required": ["answer"]
    }));
    structured.name = Some("Answer".into());
    structured.strict = Some(true);

    let request = ChatRequest::new(vec![user_message("return json")])
        .with_model("gemini-test")
        .with_structured_output(structured);

    let body = provider.build_request_body(&request, "gemini-test");
    let generation = &body["generationConfig"];

    assert_eq!(
        generation["responseMimeType"], "application/json",
        "structured output must override incompatible provider defaults"
    );
    assert_eq!(generation["responseSchema"]["type"], "object");
    assert!(generation["responseSchema"].get("title").is_none());
    assert!(
        generation["responseSchema"]["properties"]["answer"]
            .get("title")
            .is_none(),
        "Rust strips title fields before sending provider schemas"
    );
    assert_eq!(generation["temperature"], 0.2);
}
