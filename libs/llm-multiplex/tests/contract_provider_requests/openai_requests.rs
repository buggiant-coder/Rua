use agent_types::{
    AgentEvent, ChatRequest, ContentPart, ModelDialogueItem, ReasoningConfig, ReasoningMode,
    ReasoningSummary, Role, StructuredOutput, ToolCall, ToolChoice, ToolDefinition, ToolResult,
};
use llm_multiplex::providers::openai_responses::{
    parse_response_stream_event, parse_response_stream_event_with_state, parse_response_value,
    ResponseStreamParseState, RESPONSES_SESSION_METADATA_KEY,
};
use llm_multiplex::{
    ModelInfo, OpenAiCompatibleConfig, OpenAiCompatibleProvider, OpenAiResponsesProvider,
};
use serde_json::json;
use sha2::{Digest, Sha256};

fn provider_with_kwargs(
    generate_kwargs: serde_json::Map<String, serde_json::Value>,
) -> OpenAiCompatibleProvider {
    OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        provider_id: "openai-compatible".into(),
        provider_name: "OpenAI-compatible".into(),
        base_url: "https://llm.example.test/v1".into(),
        api_key: Some("sk-test".into()),
        api_keys: Vec::new(),
        headers: serde_json::Map::new(),
        models: Vec::new(),
        timeout_secs: 30,
        generate_kwargs,
        support_model_discovery: false,
        support_connection_check: true,
        is_local: false,
    })
    .expect("openai-compatible provider")
}

fn provider_with_id_and_kwargs(
    provider_id: &str,
    generate_kwargs: serde_json::Map<String, serde_json::Value>,
) -> OpenAiCompatibleProvider {
    OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        provider_id: provider_id.into(),
        provider_name: provider_id.into(),
        base_url: "https://llm.example.test/v1".into(),
        api_key: Some("sk-test".into()),
        api_keys: Vec::new(),
        headers: serde_json::Map::new(),
        models: Vec::new(),
        timeout_secs: 30,
        generate_kwargs,
        support_model_discovery: false,
        support_connection_check: true,
        is_local: false,
    })
    .expect("openai-compatible provider")
}

fn provider_with_model(model: ModelInfo) -> OpenAiCompatibleProvider {
    OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        provider_id: "openai-compatible".into(),
        provider_name: "OpenAI-compatible".into(),
        base_url: "https://llm.example.test/v1".into(),
        api_key: Some("sk-test".into()),
        api_keys: Vec::new(),
        headers: serde_json::Map::new(),
        models: vec![model],
        timeout_secs: 30,
        generate_kwargs: serde_json::Map::new(),
        support_model_discovery: false,
        support_connection_check: true,
        is_local: false,
    })
    .expect("openai-compatible provider")
}

fn responses_provider_with_kwargs(
    generate_kwargs: serde_json::Map<String, serde_json::Value>,
) -> OpenAiResponsesProvider {
    responses_provider_with_id_and_kwargs("openai-response", generate_kwargs)
}

fn responses_provider_with_id_and_kwargs(
    provider_id: &str,
    generate_kwargs: serde_json::Map<String, serde_json::Value>,
) -> OpenAiResponsesProvider {
    OpenAiResponsesProvider::new(OpenAiCompatibleConfig {
        provider_id: provider_id.into(),
        provider_name: "OpenAI (Responses API)".into(),
        base_url: "https://llm.example.test/v1".into(),
        api_key: Some("sk-test".into()),
        api_keys: Vec::new(),
        headers: serde_json::Map::new(),
        models: Vec::new(),
        timeout_secs: 30,
        generate_kwargs,
        support_model_discovery: false,
        support_connection_check: true,
        is_local: false,
    })
    .expect("responses provider")
}

fn responses_session_metadata(
    provider_id: &str,
    model: &str,
    response_id: &str,
    input: &serde_json::Value,
) -> serde_json::Value {
    let input_hash = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(input).expect("serialize input"))
    );
    json!({
        "provider_id": provider_id,
        "model": model,
        "response_id": response_id,
        "request_input_sha256": input_hash,
        "created_at_unix_ms": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64,
    })
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

fn assistant_tool_use_with_gemini_metadata() -> ModelDialogueItem {
    let mut call = ToolCall::new("tc-gemini-1", "get_weather", json!({"city": "Tokyo"}));
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
fn openai_responses_projects_tool_history_as_native_input_items() {
    let provider = responses_provider_with_kwargs(serde_json::Map::new());
    let mut first = ToolCall::new(
        "call_1",
        "glob",
        json!({"path": "/tmp/project", "pattern": "**/*"}),
    );
    first.provider_call_id = Some("call_1".into());
    let mut second = ToolCall::new(
        "call_2",
        "grep",
        json!({"path": "/tmp/project", "pattern": "README"}),
    );
    second.provider_call_id = Some("call_2".into());
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
    let request = ChatRequest::new(vec![
        user_message("inspect the project"),
        assistant,
        tool_result_message(&first, r#"{"matches":[]}"#),
        tool_result_message(&second, "grep output"),
    ])
    .with_model("gpt-test");

    let body = provider.build_request_body(&request);
    let input = body["input"].as_array().expect("Responses input array");

    assert_eq!(input.len(), 5);
    assert_eq!(input[1]["type"], "function_call");
    assert_eq!(input[1]["call_id"], "call_1");
    assert_eq!(input[1]["name"], "glob");
    assert_eq!(
        input[1]["arguments"],
        r#"{"path":"/tmp/project","pattern":"**/*"}"#
    );
    assert_eq!(input[2]["type"], "function_call");
    assert_eq!(input[2]["call_id"], "call_2");
    assert_eq!(input[3]["type"], "function_call_output");
    assert_eq!(input[3]["call_id"], "call_1");
    assert_eq!(input[3]["output"], r#"{"matches":[]}"#);
    assert_eq!(input[4]["type"], "function_call_output");
    assert_eq!(input[4]["call_id"], "call_2");
    assert_eq!(input[4]["output"], "grep output");
}

#[test]
fn openai_responses_repairs_incomplete_tool_history_before_projection() {
    let provider = responses_provider_with_kwargs(serde_json::Map::new());
    let first = ToolCall::new("call_1", "read", json!({"path": "README.md"}));
    let second = ToolCall::new("call_2", "glob", json!({"pattern": "docs/**"}));
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
    let orphan = ToolCall::new("orphan", "Unused", json!({}));
    let request = ChatRequest::new(vec![
        assistant,
        tool_result_message(&second, "glob output"),
        tool_result_message(&orphan, "orphan output"),
    ])
    .with_model("gpt-test");

    let body = provider.build_request_body(&request);
    let outputs = body["input"]
        .as_array()
        .expect("Responses input")
        .iter()
        .filter(|item| item["type"] == "function_call_output")
        .collect::<Vec<_>>();

    assert_eq!(outputs.len(), 2);
    assert_eq!(outputs[0]["call_id"], "call_1");
    assert!(outputs[0]["output"]
        .as_str()
        .unwrap()
        .contains("interrupted"));
    assert_eq!(outputs[1]["call_id"], "call_2");
    assert_eq!(outputs[1]["output"], "glob output");
    assert!(!serde_json::to_string(&body)
        .unwrap()
        .contains("orphan output"));
}

#[test]
fn openai_request_merges_provider_kwargs_without_overriding_explicit_request_fields() {
    let mut generate_kwargs = serde_json::Map::new();
    generate_kwargs.insert("temperature".into(), json!(0.2));
    generate_kwargs.insert("max_tokens".into(), json!(4096));
    generate_kwargs.insert("top_p".into(), json!(0.9));
    generate_kwargs.insert("presence_penalty".into(), json!(0.3));

    let provider = provider_with_kwargs(generate_kwargs);
    let request = ChatRequest::new(vec![user_message("hello")])
        .with_model("gpt-test")
        .with_temperature(0.7)
        .with_max_tokens(512);

    let body = provider.build_request_body(&request);

    assert_eq!(body["model"], "gpt-test");
    let temperature = body["temperature"]
        .as_f64()
        .expect("temperature should be numeric");
    assert!((temperature - 0.7).abs() < 1e-6);
    assert_eq!(body["max_tokens"], 512);
    assert_eq!(body["top_p"], 0.9);
    assert_eq!(body["presence_penalty"], 0.3);
}

#[test]
fn openai_request_provider_options_override_provider_generate_kwargs() {
    let mut generate_kwargs = serde_json::Map::new();
    generate_kwargs.insert("top_p".into(), json!(0.9));
    generate_kwargs.insert("seed".into(), json!(1));
    generate_kwargs.insert("parallel_tool_calls".into(), json!(false));

    let provider = provider_with_kwargs(generate_kwargs);
    let request = ChatRequest::new(vec![user_message("search docs")])
        .with_model("gpt-test")
        .with_provider_option("top_p", json!(0.4))
        .with_provider_option("seed", json!(42))
        .with_provider_option("reasoning_effort", json!("low"));

    let body = provider.build_request_body(&request);

    assert_eq!(body["top_p"], 0.4);
    assert_eq!(body["seed"], 42);
    assert_eq!(body["parallel_tool_calls"], false);
    assert_eq!(body["reasoning_effort"], "low");
}

#[test]
fn openai_compatible_request_translates_canonical_budget_reasoning_options() {
    let provider = provider_with_id_and_kwargs("dashscope", serde_json::Map::new());
    let request = ChatRequest::new(vec![user_message("think")])
        .with_model("qwen-test")
        .with_provider_option("thinking_enabled", json!(true))
        .with_provider_option("thinking_budget", json!(8192));

    let body = provider.build_request_body(&request);

    assert_eq!(body["enable_thinking"], true);
    assert_eq!(body["thinking_budget"], 8192);
    assert!(body.get("thinking_enabled").is_none());
}

#[test]
fn openai_compatible_protocol_translates_typed_reasoning() {
    let provider = provider_with_kwargs(serde_json::Map::new());
    let request = ChatRequest::new(vec![user_message("think")])
        .with_model("gpt-5")
        .with_reasoning(ReasoningConfig {
            mode: ReasoningMode::Effort,
            enabled: true,
            effort: Some("high".into()),
            budget_tokens: None,
            summary: Some(ReasoningSummary::Auto),
        });

    let body = provider.build_request_body(&request);

    assert_eq!(body["reasoning_effort"], "high");
    assert!(body.get("reasoning").is_none());
}

#[test]
fn openai_compatible_vendor_dialects_translate_typed_reasoning_in_adapter() {
    let reasoning = ReasoningConfig {
        mode: ReasoningMode::Effort,
        enabled: true,
        effort: Some("high".into()),
        budget_tokens: None,
        summary: Some(ReasoningSummary::Auto),
    };

    let dashscope = provider_with_id_and_kwargs("dashscope", serde_json::Map::new());
    let dashscope_body = dashscope.build_request_body(
        &ChatRequest::new(vec![user_message("think")])
            .with_model("glm-5.2")
            .with_reasoning(reasoning.clone()),
    );
    assert_eq!(dashscope_body["thinking"], json!({"type": "enabled"}));
    assert_eq!(dashscope_body["reasoning_effort"], "high");
    assert!(dashscope_body.get("extra_body").is_none());

    let openrouter = provider_with_id_and_kwargs("openrouter", serde_json::Map::new());
    let openrouter_body = openrouter.build_request_body(
        &ChatRequest::new(vec![user_message("think")])
            .with_model("anthropic/claude-sonnet-4")
            .with_reasoning(reasoning),
    );
    assert_eq!(openrouter_body["reasoning"], json!({"effort": "high"}));
}

#[test]
fn reasoning_overrides_remove_sdk_wrappers_and_inherited_controls() {
    let kwargs = json!({
        "extra_body": {"enable_thinking": true, "thinking_budget": 4096,
            "thinking": {"type": "enabled"}, "reasoning_effort": "high", "vendor": {"keep": 1}},
        "reasoning": {"effort": "high", "max_tokens": 4096, "exclude": true}
    })
    .as_object()
    .unwrap()
    .clone();
    for provider_id in ["kimi-cn", "zhipu-cn", "deepseek", "mimo", "volcengine-cn"] {
        let provider = provider_with_id_and_kwargs(provider_id, kwargs.clone());
        let body = provider.build_request_body(
            &ChatRequest::new(vec![user_message("hello")]).with_reasoning(ReasoningConfig {
                mode: ReasoningMode::Effort,
                enabled: false,
                effort: Some("none".into()),
                budget_tokens: None,
                summary: None,
            }),
        );
        assert_eq!(
            body["thinking"],
            json!({"type":"disabled"}),
            "{provider_id}"
        );
        assert_eq!(body["vendor"], json!({"keep":1}));
        for key in [
            "extra_body",
            "enable_thinking",
            "thinking_budget",
            "reasoning_effort",
            "reasoning",
        ] {
            assert!(body.get(key).is_none(), "{provider_id}: {body}");
        }
    }
    let provider = provider_with_id_and_kwargs("openrouter", kwargs);
    let body = provider.build_request_body(
        &ChatRequest::new(vec![user_message("hello")]).with_reasoning(ReasoningConfig {
            mode: ReasoningMode::Budget,
            enabled: true,
            effort: None,
            budget_tokens: Some(8192),
            summary: None,
        }),
    );
    assert_eq!(body["reasoning"], json!({"max_tokens":8192,"exclude":true}));
    assert!(body.get("thinking").is_none());
}

#[test]
fn reasoning_sdk_extras_merge_deeply_and_neutral_off_is_consumed() {
    let provider = provider_with_id_and_kwargs("dashscope", json!({
        "extra_body":{"enable_thinking":true,"thinking_budget":8192,"vendor":{"keep":1,"override":0}}
    }).as_object().unwrap().clone());
    let body = provider.build_request_body(
        &ChatRequest::new(vec![user_message("hello")])
            .with_provider_option("extra_body", json!({"vendor":{"override":2}}))
            .with_provider_option("disable_thinking", json!(true)),
    );
    assert_eq!(body["enable_thinking"], false);
    assert_eq!(body["vendor"], json!({"keep":1,"override":2}));
    assert!(body.get("thinking_budget").is_none());
    assert!(body.get("disable_thinking").is_none());
    assert!(body.get("extra_body").is_none());
}

#[test]
fn reasoning_models_use_protocol_specific_output_limits() {
    let chat = provider_with_kwargs(serde_json::Map::new());
    let responses = responses_provider_with_kwargs(serde_json::Map::new());
    for model in ["gpt-5", "gpt-6-astra", "o3", "openai/o4-mini"] {
        let request = ChatRequest::new(vec![user_message("hello")])
            .with_model(model)
            .with_max_tokens(1024);
        let body = chat.build_request_body(&request);
        assert_eq!(body["max_completion_tokens"], 1024, "{model}");
        assert!(body.get("max_tokens").is_none());
        let body = responses.build_request_body(&request);
        assert_eq!(body["max_output_tokens"], 1024, "{model}");
        assert!(body.get("max_completion_tokens").is_none());
        assert!(body.get("max_tokens").is_none());
    }
}

#[test]
fn responses_reasoning_converts_raw_effort_and_respects_off_metadata() {
    let provider = responses_provider_with_kwargs(
        json!({
            "extra_body":{"reasoning_effort":"high"}
        })
        .as_object()
        .unwrap()
        .clone(),
    );
    let mut request = ChatRequest::new(vec![user_message("hello")]);
    let body = provider.build_request_body(&request);
    assert_eq!(body["reasoning"]["effort"], "high");
    assert!(body.get("reasoning_effort").is_none());
    assert!(body.get("extra_body").is_none());
    request
        .provider_options
        .insert("disable_thinking".into(), json!(true));
    request
        .metadata
        .insert("rua_reasoning_off_effort".into(), json!("minimal"));
    let body = provider.build_request_body(&request);
    assert_eq!(body["reasoning"], json!({"effort":"minimal"}));
    assert!(body.get("disable_thinking").is_none());
}

#[test]
fn openai_request_merges_model_thinking_kwargs_with_request_overrides() {
    let mut model_kwargs = serde_json::Map::new();
    model_kwargs.insert("enable_thinking".into(), json!(true));
    model_kwargs.insert("thinking_budget".into(), json!(4096));
    model_kwargs.insert("reasoning_effort".into(), json!("high"));
    let provider = provider_with_model(ModelInfo {
        id: "qwen-thinking".into(),
        name: "Qwen Thinking".into(),
        context_window: None,
        max_output_tokens: None,
        supports_multimodal: None,
        supports_image: None,
        supports_video: None,
        probe_source: Some("config".into()),
        generate_kwargs: model_kwargs,
    });
    let request = ChatRequest::new(vec![user_message("think")])
        .with_model("qwen-thinking")
        .with_provider_option("reasoning_effort", json!("low"));

    let body = provider.build_request_body_for_model(&request, "qwen-thinking");

    assert_eq!(body["enable_thinking"], true);
    assert_eq!(body["thinking_budget"], 4096);
    assert_eq!(
        body["reasoning_effort"], "low",
        "request-level provider_options must override model defaults"
    );
}

#[test]
fn openai_request_without_thinking_metadata_does_not_emit_thinking_fields() {
    let provider = provider_with_model(ModelInfo {
        id: "plain-model".into(),
        name: "Plain Model".into(),
        context_window: None,
        max_output_tokens: None,
        supports_multimodal: None,
        supports_image: None,
        supports_video: None,
        probe_source: Some("config".into()),
        generate_kwargs: serde_json::Map::new(),
    });
    let request = ChatRequest::new(vec![user_message("hello")]).with_model("plain-model");

    let body = provider.build_request_body_for_model(&request, "plain-model");

    assert!(body.get("enable_thinking").is_none());
    assert!(body.get("thinking_budget").is_none());
    assert!(body.get("reasoning_effort").is_none());
}

#[test]
fn openai_responses_request_uses_responses_wire_shape() {
    let mut generate_kwargs = serde_json::Map::new();
    generate_kwargs.insert("reasoning_effort".into(), json!("medium"));
    let provider = responses_provider_with_kwargs(generate_kwargs);
    let request = ChatRequest::new(vec![user_message("hello")])
        .with_model("gpt-5")
        .with_temperature(0.2)
        .with_max_tokens(512)
        .with_provider_option("reasoning_effort", json!("high"));

    let body = provider.build_request_body(&request);

    assert_eq!(body["model"], "gpt-5");
    assert!(body.get("input").is_some());
    assert!(body.get("messages").is_none());
    assert_eq!(body["max_output_tokens"], 512);
    assert!(body.get("max_tokens").is_none());
    assert_eq!(body["reasoning"]["effort"], "high");
    assert!(body.get("reasoning_effort").is_none());
}

#[test]
fn output_limit_and_reasoning_aliases_preserve_request_precedence() {
    let provider = provider_with_kwargs(
        json!({"extra_body":{"max_completion_tokens":4096}})
            .as_object()
            .unwrap()
            .clone(),
    );
    let call = ChatRequest::new(vec![user_message("hello")])
        .with_model("gpt-5")
        .with_max_tokens(512);
    let body = provider.build_request_body(&call);
    assert_eq!(body["max_completion_tokens"], 512);
    let body =
        provider.build_request_body(&call.clone().with_provider_option("max_tokens", json!(256)));
    assert_eq!(body["max_completion_tokens"], 256);

    let provider = responses_provider_with_kwargs(
        json!({"max_output_tokens":4096,"reasoning":{"effort":"medium","summary":"auto"}})
            .as_object()
            .unwrap()
            .clone(),
    );
    let body = provider.build_request_body(
        &call
            .with_provider_option("max_completion_tokens", json!(256))
            .with_provider_option("reasoning_effort", json!("high")),
    );
    assert_eq!(body["max_output_tokens"], 256);
    assert!(body.get("max_completion_tokens").is_none());
    assert_eq!(body["reasoning"], json!({"effort":"high","summary":"auto"}));
    assert!(body.get("reasoning_effort").is_none());
}

#[test]
fn dashscope_responses_uses_previous_response_id_and_only_new_tool_results() {
    let provider = responses_provider_with_id_and_kwargs("dashscope", serde_json::Map::new());
    let user = user_message("inspect the project");
    let first_request = ChatRequest::new(vec![user.clone()]).with_model("qwen3.8-max");
    let first_body = provider.build_request_body(&first_request);
    assert_eq!(first_body["store"], true);
    assert!(first_body.get("previous_response_id").is_none());

    let call = ToolCall::new("call_1", "read", json!({"path": "README.md"}));
    let mut assistant = ModelDialogueItem {
        id: None,
        role: Role::Assistant,
        content: vec![ContentPart::ToolUse { call: call.clone() }],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    };
    assistant.metadata.insert(
        RESPONSES_SESSION_METADATA_KEY.into(),
        responses_session_metadata(
            "dashscope",
            "qwen3.8-max",
            "response-1",
            &first_body["input"],
        ),
    );
    let request = ChatRequest::new(vec![
        user,
        assistant,
        tool_result_message(&call, "file contents"),
    ])
    .with_model("qwen3.8-max");

    let body = provider.build_request_body(&request);
    let input = body["input"].as_array().expect("Responses input array");

    assert_eq!(body["previous_response_id"], "response-1");
    assert_eq!(body["store"], true);
    assert_eq!(input.len(), 1);
    assert_eq!(input[0]["type"], "function_call_output");
    assert_eq!(input[0]["call_id"], "call_1");
    assert_eq!(input[0]["output"], "file contents");
}

#[test]
fn dashscope_responses_replays_full_history_when_prefix_changed() {
    let provider = responses_provider_with_id_and_kwargs("dashscope", serde_json::Map::new());
    let original_user = user_message("original");
    let first_request = ChatRequest::new(vec![original_user]).with_model("qwen3.8-max");
    let first_body = provider.build_request_body(&first_request);
    let mut assistant = ModelDialogueItem::text(Role::Assistant, "answer");
    assistant.metadata.insert(
        RESPONSES_SESSION_METADATA_KEY.into(),
        responses_session_metadata(
            "dashscope",
            "qwen3.8-max",
            "response-1",
            &first_body["input"],
        ),
    );
    let request = ChatRequest::new(vec![
        user_message("edited"),
        assistant,
        user_message("follow-up"),
    ])
    .with_model("qwen3.8-max");

    let body = provider.build_request_body(&request);

    assert!(body.get("previous_response_id").is_none());
    assert_eq!(body["input"].as_array().unwrap().len(), 3);
}

#[test]
fn dashscope_responses_finds_older_anchor_after_newer_prefixes_change() {
    let provider = responses_provider_with_id_and_kwargs("dashscope", serde_json::Map::new());
    let mut request =
        ChatRequest::new(vec![user_message("完整历史：你好")]).with_model("qwen3.8-max");
    for index in 0..5 {
        let mut without_sessions = request.clone();
        for message in &mut without_sessions.messages {
            message.metadata.remove(RESPONSES_SESSION_METADATA_KEY);
        }
        let input = provider.build_request_body(&without_sessions)["input"].clone();
        let call = ToolCall::new(format!("call-{index}"), "read", json!({"path": "文件.md"}));
        let mut assistant = ModelDialogueItem::text(Role::Assistant, format!("answer {index}"));
        assistant
            .content
            .push(ContentPart::ToolUse { call: call.clone() });
        let mut session = responses_session_metadata(
            "dashscope",
            "qwen3.8-max",
            &format!("response-{index}"),
            &input,
        );
        if index > 1 {
            session["request_input_sha256"] = json!("stale-prefix");
        }
        assistant
            .metadata
            .insert(RESPONSES_SESSION_METADATA_KEY.into(), session);
        request.messages.push(assistant);
        request.messages.push(tool_result_message(&call, "结果"));
    }

    let body = provider.build_request_body(&request);
    assert_eq!(body["previous_response_id"], "response-1");
    let input = body["input"].as_array().unwrap();
    assert_eq!(input[0]["call_id"], "call-1");
    assert_eq!(input.last().unwrap()["call_id"], "call-4");
    assert_eq!(
        input
            .iter()
            .filter(|item| item["type"] == "function_call_output")
            .count(),
        4
    );
}

#[test]
fn dashscope_older_session_matching_preserves_input_overrides() {
    let provider = responses_provider_with_id_and_kwargs("dashscope", serde_json::Map::new());
    let input = json!([{"role": "user", "content": "explicit full input"}]);
    let mut older = ModelDialogueItem::text(Role::Assistant, "old answer");
    older.metadata.insert(
        RESPONSES_SESSION_METADATA_KEY.into(),
        responses_session_metadata("dashscope", "qwen3.8-max", "older-session", &input),
    );
    let mut newest = ModelDialogueItem::text(Role::Assistant, "new answer");
    let mut stale = responses_session_metadata("dashscope", "qwen3.8-max", "stale-session", &input);
    stale["request_input_sha256"] = json!("changed");
    newest
        .metadata
        .insert(RESPONSES_SESSION_METADATA_KEY.into(), stale);
    let request = ChatRequest::new(vec![
        user_message("history"),
        older,
        user_message("middle"),
        newest,
        user_message("follow-up"),
    ])
    .with_model("qwen3.8-max")
    .with_provider_option("input", input.clone());

    let body = provider.build_request_body(&request);
    assert_eq!(body["previous_response_id"], "older-session");
    assert_eq!(body["input"], input);
}

#[test]
fn openai_responses_protocol_translates_canonical_reasoning_for_any_provider_id() {
    for provider_id in ["openai-response", "idealab-responses", "custom-responses"] {
        let provider = responses_provider_with_id_and_kwargs(provider_id, serde_json::Map::new());
        let request = ChatRequest::new(vec![user_message("think")])
            .with_model("gpt-5")
            .with_reasoning(ReasoningConfig {
                mode: ReasoningMode::Effort,
                enabled: true,
                effort: Some("high".into()),
                budget_tokens: None,
                summary: Some(ReasoningSummary::Auto),
            });

        let body = provider.build_request_body(&request);

        assert_eq!(
            body["reasoning"],
            json!({
                "effort": "high",
                "summary": "auto"
            }),
            "{provider_id}"
        );
        assert!(body.get("reasoning_effort").is_none(), "{provider_id}");
    }

    let provider =
        responses_provider_with_id_and_kwargs("idealab-responses", serde_json::Map::new());
    let auto = provider.build_request_body(
        &ChatRequest::new(vec![user_message("think")])
            .with_model("gpt-5")
            .with_reasoning(ReasoningConfig {
                mode: ReasoningMode::Effort,
                enabled: true,
                effort: None,
                budget_tokens: None,
                summary: Some(ReasoningSummary::Auto),
            }),
    );
    assert_eq!(auto["reasoning"], json!({"summary": "auto"}));
}

#[test]
fn openai_responses_request_flattens_function_tools_and_choice() {
    let provider = responses_provider_with_kwargs(serde_json::Map::new());
    let tool = ToolDefinition {
        name: "get_time".into(),
        description: "Get the current time".into(),
        parameters: json!({
            "type": "object",
            "properties": {},
            "required": []
        }),
        strict: None,
        annotations: Default::default(),
        metadata: serde_json::Map::new(),
    };
    let request = ChatRequest::new(vec![user_message("hello")])
        .with_model("gpt-5")
        .with_tools(vec![tool])
        .with_tool_choice(ToolChoice::Function("get_time".into()));

    let body = provider.build_request_body(&request);

    assert_eq!(
        body["tools"],
        json!([{
            "type": "function",
            "name": "get_time",
            "description": "Get the current time",
            "parameters": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }])
    );
    assert_eq!(
        body["tool_choice"],
        json!({
            "type": "function",
            "name": "get_time"
        })
    );
}

#[test]
fn openai_responses_request_uses_responses_content_part_types() {
    let provider = responses_provider_with_kwargs(serde_json::Map::new());
    let request = ChatRequest::new(vec![
        ModelDialogueItem {
            id: None,
            role: Role::System,
            content: vec![
                ContentPart::Text {
                    text: "First instruction.".into(),
                },
                ContentPart::Text {
                    text: "Second instruction.".into(),
                },
            ],
            name: None,
            created_at: None,
            metadata: serde_json::Map::new(),
        },
        ModelDialogueItem {
            id: None,
            role: Role::Assistant,
            content: vec![
                ContentPart::Text {
                    text: "Previous".into(),
                },
                ContentPart::Text {
                    text: "answer".into(),
                },
            ],
            name: None,
            created_at: None,
            metadata: serde_json::Map::new(),
        },
        ModelDialogueItem {
            id: None,
            role: Role::User,
            content: vec![
                ContentPart::Text {
                    text: "Describe this image.".into(),
                },
                ContentPart::Image {
                    url: "https://example.test/image.png".into(),
                    media_type: Some("image/png".into()),
                    alt: None,
                },
            ],
            name: None,
            created_at: None,
            metadata: serde_json::Map::new(),
        },
    ])
    .with_model("gpt-5");

    let body = provider.build_request_body(&request);

    assert_eq!(body["input"][0]["role"], "system");
    assert_eq!(
        body["input"][0]["content"],
        "First instruction.\n\nSecond instruction."
    );
    assert_eq!(body["input"][1]["content"][0]["type"], "output_text");
    assert_eq!(body["input"][1]["content"][1]["type"], "output_text");
    assert_eq!(body["input"][2]["content"][0]["type"], "input_text");
    assert_eq!(body["input"][2]["content"][1]["type"], "input_image");
    assert_eq!(
        body["input"][2]["content"][1]["image_url"],
        "https://example.test/image.png"
    );
}

#[test]
fn openai_responses_encodes_local_images_as_data_urls() {
    let provider = responses_provider_with_kwargs(serde_json::Map::new());
    let path = std::env::temp_dir().join(format!("rua-image-{}.png", uuid::Uuid::new_v4()));
    std::fs::write(&path, b"\x89PNG\r\n\x1a\n").expect("write image fixture");
    let mut message = ModelDialogueItem::text(Role::User, "");
    message.content = vec![ContentPart::Image {
        url: path.to_string_lossy().into_owned(),
        media_type: Some("image/png".to_string()),
        alt: None,
    }];

    let body = provider.build_request_body(&ChatRequest::new(vec![message]).with_model("gpt-5"));
    let image_url = body["input"][0]["content"][0]["image_url"]
        .as_str()
        .expect("image URL");
    let _ = std::fs::remove_file(path);

    assert!(image_url.starts_with("data:image/png;base64,"));
}

#[test]
fn openai_responses_response_normalizes_text_tool_calls_and_usage() {
    let response = parse_response_value(json!({
        "id": "resp_1",
        "model": "gpt-5",
        "output": [
            {
                "type": "message",
                "content": [
                    {"type": "output_text", "text": "hello"}
                ]
            },
            {
                "type": "function_call",
                "call_id": "call_1",
                "name": "read_file",
                "arguments": "{\"path\":\"/tmp/a\"}"
            }
        ],
        "usage": {
            "input_tokens": 10,
            "output_tokens": 5,
            "total_tokens": 15,
            "input_tokens_details": {"cached_tokens": 3},
            "output_tokens_details": {"reasoning_tokens": 2}
        }
    }))
    .expect("responses value parses");

    assert_eq!(response.model.as_deref(), Some("gpt-5"));
    assert_eq!(response.usage.as_ref().unwrap().prompt_tokens, 10);
    assert_eq!(response.usage.as_ref().unwrap().completion_tokens, 5);
    assert_eq!(response.usage.as_ref().unwrap().total_tokens, Some(15));
    assert_eq!(response.usage.as_ref().unwrap().cached_tokens, Some(3));
    assert_eq!(response.usage.as_ref().unwrap().reasoning_tokens, Some(2));
    assert!(matches!(
        &response.message.content[0],
        ContentPart::Text { text } if text == "hello"
    ));
    match &response.message.content[1] {
        ContentPart::ToolUse { call } => {
            assert_eq!(call.id, "call_1");
            assert_eq!(call.provider_call_id.as_deref(), Some("call_1"));
            assert_eq!(call.name, "read_file");
            assert_eq!(call.arguments["path"], "/tmp/a");
        }
        other => panic!("expected tool use, got {other:?}"),
    }
}

#[test]
fn openai_responses_response_normalizes_reasoning_summary() {
    let response = parse_response_value(json!({
        "id": "resp_reasoning",
        "model": "gpt-5",
        "output": [
            {
                "type": "reasoning",
                "summary": [
                    {"type": "summary_text", "text": "Inspected the relevant paths."}
                ]
            },
            {
                "type": "message",
                "content": [
                    {"type": "output_text", "text": "Done"}
                ]
            }
        ]
    }))
    .expect("responses value parses");

    assert!(matches!(
        &response.message.content[0],
        ContentPart::Thinking { text, .. } if text == "Inspected the relevant paths."
    ));
    assert!(matches!(
        &response.message.content[1],
        ContentPart::Text { text } if text == "Done"
    ));
}

#[test]
fn openai_responses_stream_events_normalize_text_tool_usage_and_errors() {
    let text = parse_response_stream_event(&json!({
        "type": "response.output_text.delta",
        "delta": "hel"
    }));
    assert!(matches!(
        &text.events[0],
        AgentEvent::TextDelta { delta, .. } if delta == "hel"
    ));

    let reasoning = parse_response_stream_event(&json!({
        "type": "response.reasoning_summary_text.delta",
        "delta": "checked the repository"
    }));
    assert!(matches!(
        &reasoning.events[0],
        AgentEvent::ThinkingDelta { delta, .. } if delta == "checked the repository"
    ));

    let dashscope_reasoning = parse_response_stream_event(&json!({
        "type": "response.reasoning_text.delta",
        "delta": "checking the next step"
    }));
    assert!(matches!(
        &dashscope_reasoning.events[0],
        AgentEvent::ThinkingDelta { delta, .. } if delta == "checking the next step"
    ));

    let tool_start = parse_response_stream_event(&json!({
        "type": "response.output_item.added",
        "output_index": 2,
        "item": {
            "type": "function_call",
            "id": "fc_1",
            "call_id": "call_1",
            "name": "grep"
        }
    }));
    assert!(matches!(
        &tool_start.events[0],
        AgentEvent::ToolCallDelta { id, index, name, arguments_delta, .. }
            if id == "call_1" && *index == 2 && name.as_deref() == Some("grep") && arguments_delta.is_empty()
    ));

    let tool_args = parse_response_stream_event(&json!({
        "type": "response.function_call_arguments.delta",
        "output_index": 2,
        "item_id": "call_1",
        "delta": "{\"query\":\"foo\""
    }));
    assert!(matches!(
        &tool_args.events[0],
        AgentEvent::ToolCallDelta { id, index, name, arguments_delta, .. }
            if id == "call_1" && *index == 2 && name.is_none() && arguments_delta == "{\"query\":\"foo\""
    ));

    let completed = parse_response_stream_event(&json!({
        "type": "response.completed",
        "response": {
            "usage": {
                "input_tokens": 4,
                "output_tokens": 6,
                "total_tokens": 10
            }
        }
    }));
    assert_eq!(completed.usage.unwrap().total_tokens, Some(10));

    let failed = parse_response_stream_event(&json!({
        "type": "response.failed",
        "response": {
            "error": {"message": "bad request"}
        }
    }));
    assert!(failed.terminal_error);
    assert!(matches!(
        &failed.events[0],
        AgentEvent::Error { message, retryable, .. } if message == "bad request" && !retryable
    ));

    let transient_failure = parse_response_stream_event(&json!({
        "type": "response.failed",
        "response": {
            "error": {
                "message": "upstream connect error or disconnect/reset before headers. reset reason: connection termination"
            }
        }
    }));
    assert!(matches!(
        &transient_failure.events[0],
        AgentEvent::Error {
            retryable: true,
            ..
        }
    ));

    let top_level_error = parse_response_stream_event(&json!({
        "type": "error",
        "error": {
            "message": "invalid input content",
            "type": "invalid_request_error"
        }
    }));
    assert!(top_level_error.terminal_error);
    assert!(matches!(
        &top_level_error.events[0],
        AgentEvent::Error { message, retryable, .. }
            if message == "invalid input content" && !retryable
    ));
}

#[test]
fn openai_responses_stream_done_events_complete_tool_arguments_without_duplication() {
    let done_only = parse_response_stream_event(&json!({
        "type": "response.output_item.done",
        "output_index": 1,
        "item": {
            "type": "function_call",
            "id": "fc_1",
            "call_id": "call_1",
            "name": "read_file",
            "arguments": "{\"path\":\"/tmp/a\"}"
        }
    }));
    assert!(matches!(
        &done_only.events[0],
        AgentEvent::ToolCallDelta { id, index, name, arguments_delta, .. }
            if id == "call_1" && *index == 1 && name.as_deref() == Some("read_file") && arguments_delta == "{\"path\":\"/tmp/a\"}"
    ));

    let mut state = ResponseStreamParseState::default();
    let delta = parse_response_stream_event_with_state(
        &json!({
            "type": "response.function_call_arguments.delta",
            "output_index": 1,
            "item_id": "call_1",
            "delta": "{\"path\""
        }),
        &mut state,
    );
    assert!(matches!(
        &delta.events[0],
        AgentEvent::ToolCallDelta { arguments_delta, .. } if arguments_delta == "{\"path\""
    ));

    let done = parse_response_stream_event_with_state(
        &json!({
            "type": "response.function_call_arguments.done",
            "output_index": 1,
            "item_id": "call_1",
            "arguments": "{\"path\":\"/tmp/a\"}"
        }),
        &mut state,
    );
    assert!(matches!(
        &done.events[0],
        AgentEvent::ToolCallDelta { id, index, name, arguments_delta, .. }
            if id == "call_1" && *index == 1 && name.is_none() && arguments_delta == ":\"/tmp/a\"}"
    ));
}

#[test]
fn openai_responses_stream_uses_call_id_across_item_and_argument_events() {
    let mut state = ResponseStreamParseState::default();
    let wire_events = [
        json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {
                "type": "function_call",
                "id": "fc_1",
                "call_id": "call_1",
                "name": "bash"
            }
        }),
        json!({
            "type": "response.function_call_arguments.delta",
            "output_index": 0,
            "item_id": "fc_1",
            "delta": "{\"command\":\"pwd\"}"
        }),
        json!({
            "type": "response.function_call_arguments.done",
            "output_index": 0,
            "item_id": "fc_1",
            "arguments": "{\"command\":\"pwd\"}"
        }),
        json!({
            "type": "response.output_item.done",
            "output_index": 0,
            "item": {
                "type": "function_call",
                "id": "fc_1",
                "call_id": "call_1",
                "name": "bash",
                "arguments": "{\"command\":\"pwd\"}"
            }
        }),
    ];

    let tool_deltas: Vec<_> = wire_events
        .iter()
        .flat_map(|event| parse_response_stream_event_with_state(event, &mut state).events)
        .filter_map(|event| match event {
            AgentEvent::ToolCallDelta {
                id,
                arguments_delta,
                ..
            } => Some((id, arguments_delta)),
            _ => None,
        })
        .collect();

    assert!(
        tool_deltas.iter().all(|(id, _)| id == "call_1"),
        "all streamed fragments must use the provider call_id: {tool_deltas:?}"
    );
    assert_eq!(
        tool_deltas
            .iter()
            .map(|(_, delta)| delta.as_str())
            .collect::<String>(),
        r#"{"command":"pwd"}"#,
        "completed events must not append the full arguments a second time"
    );
}

#[test]
fn openai_streaming_request_keeps_usage_options_and_tool_choice_contract() {
    let provider = provider_with_kwargs(serde_json::Map::new());
    let mut request = ChatRequest::streaming(vec![user_message("search docs")])
        .with_model("gpt-test")
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
    request
        .provider_options
        .insert("stream_options".into(), json!({"include_usage": true}));

    let body = provider.build_request_body(&request);

    assert_eq!(body["stream"], true);
    assert_eq!(body["stream_options"], json!({"include_usage": true}));
    assert_eq!(
        body["tools"][0],
        json!({
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
        })
    );
    assert_eq!(
        body["tool_choice"],
        json!({
            "type": "function",
            "function": {"name": "Search"}
        })
    );
}

#[test]
fn openai_request_does_not_leak_gemini_tool_extra_content() {
    let provider = provider_with_kwargs(serde_json::Map::new());
    let request = ChatRequest::new(vec![
        user_message("Find the weather in Tokyo"),
        assistant_tool_use_with_gemini_metadata(),
    ])
    .with_model("gpt-test");

    let body = provider.build_request_body(&request);
    let tool_call = &body["messages"][1]["tool_calls"][0];

    assert_eq!(
        tool_call,
        &json!({
            "id": "tc-gemini-1",
            "type": "function",
            "function": {
                "name": "get_weather",
                "arguments": "{\"city\":\"Tokyo\"}"
            }
        }),
        "OpenAI-compatible request projection must drop Gemini-only extra_content metadata"
    );
    assert!(
        !serde_json::to_string(&body)
            .expect("body json")
            .contains("gemini_sig_abc"),
        "Gemini thought signatures must not leak into OpenAI-compatible payloads: {body:?}"
    );
}

#[test]
fn openai_request_uses_raw_arguments_without_leaking_raw_argument_fields() {
    let provider = provider_with_kwargs(serde_json::Map::new());
    let request = ChatRequest::new(vec![
        user_message("Read a file"),
        assistant_tool_use_with_raw_arguments(),
    ])
    .with_model("gpt-test");

    let body = provider.build_request_body(&request);
    let tool_call = &body["messages"][1]["tool_calls"][0];

    assert_eq!(tool_call["function"]["arguments"], r#"{"path":"/tmp/x"}"#);
    let serialized = serde_json::to_string(&body).expect("body json");
    assert!(!serialized.contains("raw_arguments"));
    assert!(!serialized.contains("raw_input"));
}

#[test]
fn openai_structured_output_uses_native_json_schema_without_schema_titles() {
    let mut generate_kwargs = serde_json::Map::new();
    generate_kwargs.insert("response_format".into(), json!({"type": "text"}));
    generate_kwargs.insert("top_p".into(), json!(0.8));

    let provider = provider_with_kwargs(generate_kwargs);
    let mut structured = StructuredOutput::new(json!({
        "type": "object",
        "title": "Answer",
        "properties": {
            "answer": {"type": "string", "title": "Answer text"},
            "confidence": {"type": "number"}
        },
        "required": ["answer"]
    }));
    structured.name = Some("AnswerPayload".into());
    structured.strict = Some(true);

    let request = ChatRequest::new(vec![user_message("return json")])
        .with_model("gpt-test")
        .with_structured_output(structured);

    let body = provider.build_request_body(&request);
    let response_format = &body["response_format"];
    let json_schema = &response_format["json_schema"];

    assert_eq!(response_format["type"], "json_schema");
    assert_eq!(json_schema["name"], "AnswerPayload");
    assert_eq!(json_schema["strict"], true);
    assert_eq!(json_schema["schema"]["type"], "object");
    assert!(json_schema["schema"].get("title").is_none());
    assert!(
        json_schema["schema"]["properties"]["answer"]
            .get("title")
            .is_none(),
        "provider request schemas strip title fields recursively"
    );
    assert_eq!(body["top_p"], 0.8);
    assert!(body.get("tools").is_none());
    assert!(body.get("tool_choice").is_none());
}

#[test]
fn openai_structured_output_fallback_forces_synthetic_tool_call() {
    let mut generate_kwargs = serde_json::Map::new();
    generate_kwargs.insert("tool_choice".into(), json!("auto"));
    generate_kwargs.insert("parallel_tool_calls".into(), json!(false));

    let provider = provider_with_kwargs(generate_kwargs);
    let mut structured = StructuredOutput::new(json!({
        "type": "object",
        "title": "Answer",
        "properties": {
            "answer": {"type": "string", "title": "Answer text"}
        },
        "required": ["answer"]
    }));
    structured.name = Some("emit_answer".into());
    structured.fallback_to_tool_call = true;

    let request = ChatRequest::new(vec![user_message("return json")])
        .with_model("gpt-test")
        .with_structured_output(structured);

    let body = provider.build_request_body(&request);
    let function = &body["tools"][0]["function"];

    assert_eq!(function["name"], "emit_answer");
    assert_eq!(
        function["description"],
        "Generate the required structured output with this function"
    );
    assert_eq!(function["parameters"]["type"], "object");
    assert!(function["parameters"].get("title").is_none());
    assert!(function["parameters"]["properties"]["answer"]
        .get("title")
        .is_none());
    assert_eq!(
        body["tool_choice"],
        json!({
            "type": "function",
            "function": {"name": "emit_answer"}
        }),
        "structured fallback owns tool_choice instead of inheriting provider defaults"
    );
    assert_eq!(body["parallel_tool_calls"], false);
    assert!(body.get("response_format").is_none());
}

#[test]
fn responses_phase_round_trip_preserves_each_assistant_message_boundary() {
    let parsed = parse_response_value(json!({
        "output": [
            {"type": "message", "id": "msg_1", "role": "assistant", "phase": "commentary",
             "content": [{"type": "output_text", "text": "Checking."}]},
            {"type": "message", "id": "msg_2", "role": "assistant", "phase": "commentary",
             "content": [{"type": "output_text", "text": "Checking again."}]},
            {"type": "message", "id": "msg_3", "role": "assistant", "phase": "final_answer",
             "content": [{"type": "output_text", "text": "Done."}]}
        ]
    }))
    .unwrap();
    let reloaded: ModelDialogueItem =
        serde_json::from_value(serde_json::to_value(parsed.message).unwrap()).unwrap();
    let mut user = ModelDialogueItem::text(Role::User, "continue");
    user.metadata.insert("phase".into(), json!("commentary"));
    let request = ChatRequest::new(vec![user, reloaded]).with_model("gpt-5.6-sol");
    let body = responses_provider_with_kwargs(Default::default()).build_request_body(&request);
    let input = body["input"].as_array().unwrap();
    assert!(
        input[0].get("phase").is_none(),
        "phase belongs only to assistant messages"
    );
    assert_eq!(input.len(), 4);
    for (item, (phase, text)) in input[1..].iter().zip([
        ("commentary", "Checking."),
        ("commentary", "Checking again."),
        ("final_answer", "Done."),
    ]) {
        assert_eq!(item["phase"], phase);
        assert_eq!(item["content"], text);
    }
    let chat = provider_with_kwargs(Default::default()).build_request_body(&request);
    assert!(chat["messages"]
        .as_array()
        .unwrap()
        .iter()
        .all(|message| message.get("phase").is_none()));
}

#[test]
fn responses_phase_stream_tracks_messages_and_late_phase_without_repeating_text() {
    let mut state = ResponseStreamParseState::default();
    let mut events = Vec::new();
    for value in [
        json!({"type": "response.output_item.added", "output_index": 0,
               "item": {"type": "message", "id": "msg_1", "role": "assistant", "phase": "commentary"}}),
        json!({"type": "response.output_text.delta", "output_index": 0, "item_id": "msg_1", "delta": "Working."}),
        json!({"type": "response.output_item.done", "output_index": 0,
               "item": {"type": "message", "id": "msg_1", "role": "assistant", "phase": "commentary",
                        "content": [{"type": "output_text", "text": "Working."}]}}),
        json!({"type": "response.output_item.added", "output_index": 1,
               "item": {"type": "message", "id": "msg_2", "role": "assistant"}}),
        json!({"type": "response.output_text.delta", "output_index": 1, "item_id": "msg_2", "delta": "Done."}),
        json!({"type": "response.output_item.done", "output_index": 1,
               "item": {"type": "message", "id": "msg_2", "role": "assistant", "phase": "final_answer",
                        "content": [{"type": "output_text", "text": "Done."}]}}),
    ] {
        events.extend(parse_response_stream_event_with_state(&value, &mut state).events);
    }
    let text: String = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::TextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "Working.Done.");
    assert!(events.iter().any(|event| matches!(event,
        AgentEvent::TextDelta { delta, metadata }
            if delta == "Working." && metadata["phase"] == "commentary"
    )));
    assert!(events.iter().any(|event| matches!(event,
        AgentEvent::TextDelta { metadata, .. }
            if metadata["phase"] == "final_answer"
    )));
}

#[test]
fn responses_phase_completed_only_output_and_legacy_history_remain_compatible() {
    let response = json!({
        "output_text": "Done.",
        "output": [{"type": "message", "role": "assistant", "id": "msg_final", "phase": "final_answer",
                    "content": [{"type": "output_text", "text": "Done."}]}]
    });
    let mut state = ResponseStreamParseState::default();
    let parsed = parse_response_stream_event_with_state(
        &json!({
            "type": "response.completed", "response": response,
        }),
        &mut state,
    );
    assert!(
        matches!(&parsed.events[0], AgentEvent::TextDelta { delta, metadata }
        if delta == "Done." && metadata["phase"] == "final_answer")
    );
    let parsed = parse_response_value(response).unwrap();
    assert_eq!(
        parsed.message.text_content(),
        "Done.",
        "output_text is an aggregate, not an additional message"
    );
    let legacy: ModelDialogueItem = serde_json::from_value(json!({
        "role": "assistant", "content": "old answer"
    }))
    .unwrap();
    let request = ChatRequest::new(vec![legacy, parsed.message]);
    let body = responses_provider_with_kwargs(Default::default()).build_request_body(&request);
    assert!(body["input"][0].get("phase").is_none());
    assert_eq!(body["input"][1]["phase"], "final_answer");
}

#[test]
fn responses_phase_with_tool_calls_keeps_protocol_pairing_and_message_order() {
    let response = parse_response_value(json!({"output": [
        {"type": "reasoning", "summary": [{"type": "summary_text", "text": "Inspecting."}]},
        {"type": "message", "id": "msg_progress", "role": "assistant", "phase": "commentary",
         "content": [{"type": "output_text", "text": "I will inspect it."}]},
        {"type": "function_call", "call_id": "call_read", "name": "read", "arguments": "{}"}
    ]}))
    .unwrap();
    let result = ModelDialogueItem {
        content: vec![ContentPart::ToolResult {
            result: ToolResult::text("call_read", "file content"),
        }],
        ..ModelDialogueItem::text(Role::Tool, "")
    };
    let body = responses_provider_with_kwargs(Default::default())
        .build_request_body(&ChatRequest::new(vec![response.message, result]));
    let items = body["input"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0]["phase"], "commentary");
    assert_eq!(items[1]["type"], "function_call");
    assert!(items[1].get("phase").is_none());
    assert_eq!(items[2]["type"], "function_call_output");
    assert_eq!(items[2]["output"], "file content");
}
