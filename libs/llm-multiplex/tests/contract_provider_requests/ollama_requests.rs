use agent_types::{
    ChatRequest, ContentPart, ModelDialogueItem, ReasoningConfig, ReasoningMode, ReasoningSummary,
    Role, ToolCall, ToolChoice, ToolDefinition, ToolResult,
};
use llm_multiplex::providers::ollama::{OllamaConfig, OllamaProvider};
use serde_json::json;

fn provider_with_kwargs(
    generate_kwargs: serde_json::Map<String, serde_json::Value>,
) -> OllamaProvider {
    OllamaProvider::new(OllamaConfig {
        provider_id: "ollama".into(),
        provider_name: "Ollama".into(),
        base_url: "http://localhost:11434".into(),
        models: vec![],
        timeout_secs: 30,
        generate_kwargs,
    })
    .expect("ollama provider")
}

fn user_message(text: &str) -> ModelDialogueItem {
    ModelDialogueItem {
        id: None,
        role: Role::User,
        content: vec![ContentPart::Text {
            text: text.to_string(),
        }],
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
                signature: None,
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

fn tool_result_message(text: &str) -> ModelDialogueItem {
    let mut result = ToolResult::text("tc-1", text);
    result.name = Some("bash".into());
    ModelDialogueItem {
        id: None,
        role: Role::Tool,
        content: vec![ContentPart::ToolResult { result }],
        name: Some("bash".into()),
        created_at: None,
        metadata: serde_json::Map::new(),
    }
}

fn assistant_tool_call() -> ModelDialogueItem {
    ModelDialogueItem {
        id: None,
        role: Role::Assistant,
        content: vec![ContentPart::ToolUse {
            call: ToolCall::new("tc-1", "bash", json!({})),
        }],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    }
}

#[test]
fn ollama_request_uses_native_top_level_keep_alive_think_and_options() {
    let mut generate_kwargs = serde_json::Map::new();
    generate_kwargs.insert("keep_alive".into(), json!("15m"));
    generate_kwargs.insert("think".into(), json!(false));
    generate_kwargs.insert("top_p".into(), json!(0.9));
    generate_kwargs.insert("num_ctx".into(), json!(8192));

    let provider = provider_with_kwargs(generate_kwargs);
    let mut request = ChatRequest::new(vec![user_message("hello")])
        .with_temperature(0.2)
        .with_max_tokens(128);
    request.stream = true;
    request
        .provider_options
        .insert("keep_alive".into(), json!("30m"));
    request
        .provider_options
        .insert("enable_thinking".into(), json!(true));
    request.provider_options.insert("top_p".into(), json!(0.7));
    request.provider_options.insert("seed".into(), json!(42));

    let body = provider.build_request_body(&request, "qwen2.5");

    assert_eq!(body["model"], "qwen2.5");
    assert_eq!(body["stream"], true);
    assert_eq!(body["keep_alive"], "30m");
    assert_eq!(body["think"], true);
    assert_eq!(
        body["messages"],
        json!([{"role": "user", "content": "hello"}])
    );
    let temperature = body["options"]["temperature"]
        .as_f64()
        .expect("temperature should be numeric");
    assert!((temperature - 0.2).abs() < 1e-6);
    assert_eq!(body["options"]["num_predict"], 128);
    assert_eq!(body["options"]["top_p"], 0.7);
    assert_eq!(body["options"]["num_ctx"], 8192);
    assert_eq!(body["options"]["seed"], 42);
    assert!(body["options"].get("keep_alive").is_none());
    assert!(body["options"].get("think").is_none());
    assert!(body["options"].get("enable_thinking").is_none());
}

#[test]
fn ollama_protocol_translates_typed_reasoning() {
    let provider = provider_with_kwargs(serde_json::Map::new());
    let request = ChatRequest::new(vec![user_message("hello")]).with_reasoning(ReasoningConfig {
        mode: ReasoningMode::Effort,
        enabled: true,
        effort: Some("high".into()),
        budget_tokens: None,
        summary: Some(ReasoningSummary::Auto),
    });

    let body = provider.build_request_body(&request, "qwen3");

    assert_eq!(body["think"], "high");
}

#[test]
fn ollama_request_preserves_thinking_separately_from_visible_content() {
    let provider = provider_with_kwargs(serde_json::Map::new());
    let request = ChatRequest::new(vec![
        user_message("hello"),
        assistant_with_thinking_and_text(),
    ]);

    let body = provider.build_request_body(&request, "qwen2.5");

    assert_eq!(body["messages"][1]["role"], "assistant");
    assert_eq!(body["messages"][1]["content"], "visible answer");
    assert_eq!(body["messages"][1]["thinking"], "private reasoning");
}

#[test]
fn ollama_request_projects_tool_result_text_as_plain_content() {
    let provider = provider_with_kwargs(serde_json::Map::new());
    let request = ChatRequest::new(vec![
        assistant_tool_call(),
        tool_result_message("line 1\nline 2\n"),
    ]);

    let body = provider.build_request_body(&request, "qwen2.5");

    assert_eq!(body["messages"][1]["role"], "tool");
    assert_eq!(body["messages"][1]["content"], "line 1\nline 2\n");
    assert!(!serde_json::to_string(&body)
        .expect("ollama body serializes")
        .contains("\"type\":\"text\""));
}

#[test]
fn ollama_request_projects_tools_without_serializing_unsupported_tool_choice() {
    let provider = provider_with_kwargs(serde_json::Map::new());
    let request = ChatRequest::new(vec![user_message("search docs")])
        .with_tools(vec![ToolDefinition {
            name: "Search".into(),
            description: "Search local docs".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"}
                },
                "required": ["query"]
            }),
            strict: None,
            annotations: Default::default(),
            metadata: serde_json::Map::new(),
        }])
        .with_tool_choice(ToolChoice::Function("Search".into()));

    let body = provider.build_request_body(&request, "qwen2.5");

    assert_eq!(
        body["tools"],
        json!([
            {
                "type": "function",
                "function": {
                    "name": "Search",
                    "description": "Search local docs",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "query": {"type": "string"}
                        },
                        "required": ["query"]
                    }
                }
            }
        ])
    );
    assert!(
        body.get("tool_choice").is_none(),
        "Rust's native Ollama adapter warns and omits unsupported tool_choice"
    );
}
