use agent_types::{
    ChatRequest, ContentPart, ModelDialogueItem, ReasoningConfig, ReasoningMode, ReasoningSummary,
    Role, ToolCall,
};
use llm_multiplex::{AnthropicConfig, AnthropicProvider, ModelInfo};
use serde_json::json;

fn provider_with_kwargs(
    generate_kwargs: serde_json::Map<String, serde_json::Value>,
) -> AnthropicProvider {
    AnthropicProvider::new(AnthropicConfig {
        provider_id: "anthropic".into(),
        provider_name: "Anthropic".into(),
        base_url: "https://mock-anthropic.local".into(),
        api_key: Some("ant-test".into()),
        headers: serde_json::Map::new(),
        models: vec![ModelInfo {
            id: "claude-test".into(),
            name: "claude-test".into(),
            context_window: None,
            max_output_tokens: None,
            supports_multimodal: None,
            supports_image: None,
            supports_video: None,
            probe_source: None,
            generate_kwargs: serde_json::Map::new(),
        }],
        timeout_secs: 30,
        generate_kwargs,
        support_connection_check: true,
        api_version: "2023-06-01".into(),
        auth_mode: Default::default(),
    })
    .expect("anthropic provider")
}

#[test]
fn effort_reasoning_uses_adaptive_thinking_and_output_config() {
    let provider = provider_with_kwargs(serde_json::Map::new());
    for effort in [None, Some("low"), Some("medium"), Some("high"), Some("max")] {
        let request = ChatRequest {
            messages: vec![user_message("hello")],
            max_tokens: Some(4096),
            reasoning: Some(ReasoningConfig {
                mode: ReasoningMode::Effort,
                enabled: true,
                effort: effort.map(String::from),
                budget_tokens: None,
                summary: Some(ReasoningSummary::Auto),
            }),
            ..ChatRequest::new(vec![])
        };
        let body = provider.build_request_body(&request);
        assert_eq!(body["thinking"], json!({"type": "adaptive"}));
        assert_eq!(body["output_config"]["effort"], json!(effort));
        assert_eq!(body["max_tokens"], json!(4096));
        assert!(body.get("reasoning_effort").is_none());
    }
}

#[test]
fn effort_reasoning_overrides_manual_thinking_without_inflating_output_budget() {
    let provider = provider_with_kwargs(serde_json::Map::from_iter([
        ("thinking_enable".into(), json!(true)),
        ("thinking_budget".into(), json!(8192)),
        (
            "output_config".into(),
            json!({"effort": "low", "format": {"type": "json_schema"}}),
        ),
    ]));
    let mut request = ChatRequest {
        messages: vec![user_message("hello")],
        max_tokens: Some(4096),
        reasoning: Some(ReasoningConfig {
            mode: ReasoningMode::Effort,
            enabled: true,
            effort: Some("high".into()),
            budget_tokens: Some(16384),
            summary: Some(ReasoningSummary::Auto),
        }),
        ..ChatRequest::new(vec![])
    };
    request
        .provider_options
        .insert("thinking_enabled".into(), json!(true));
    request
        .provider_options
        .insert("enable_thinking".into(), json!(true));

    for explicit_thinking in [false, true] {
        if explicit_thinking {
            request.provider_options.insert(
                "thinking".into(),
                json!({"type": "enabled", "budget_tokens": 8192}),
            );
        }
        let body = provider.build_request_body(&request);
        assert_eq!(body["thinking"], json!({"type": "adaptive"}));
        assert_eq!(body["max_tokens"], json!(4096));
        assert_eq!(
            body["output_config"],
            json!({"effort": "high", "format": {"type": "json_schema"}})
        );
        for key in [
            "thinking_enable",
            "thinking_enabled",
            "enable_thinking",
            "disable_thinking",
            "thinking_budget",
        ] {
            assert!(body.get(key).is_none(), "leaked alias: {key}");
        }
    }
}

#[test]
fn effort_reasoning_disabled_or_default_clears_stale_effort() {
    let provider = provider_with_kwargs(serde_json::Map::from_iter([
        ("thinking".into(), json!({"type": "adaptive"})),
        (
            "output_config".into(),
            json!({"effort": "max", "format": {"type": "json_schema"}}),
        ),
    ]));
    for enabled in [false, true] {
        let request = ChatRequest {
            messages: vec![user_message("hello")],
            reasoning: Some(ReasoningConfig {
                mode: ReasoningMode::Effort,
                enabled,
                effort: None,
                budget_tokens: None,
                summary: Some(ReasoningSummary::Auto),
            }),
            ..ChatRequest::new(vec![])
        };
        let body = provider.build_request_body(&request);
        assert_eq!(
            body["thinking"],
            json!({"type": if enabled {"adaptive"} else {"disabled"}})
        );
        assert_eq!(
            body["output_config"],
            json!({"format": {"type": "json_schema"}})
        );
    }
}

#[test]
fn typed_budget_reasoning_overrides_defaults_before_sizing_output() {
    let provider = provider_with_kwargs(serde_json::Map::from_iter([
        ("thinking_enable".into(), json!(true)),
        ("thinking_budget".into(), json!(8192)),
    ]));
    for enabled in [false, true] {
        let request = ChatRequest::new(vec![user_message("hello")])
            .with_max_tokens(4096)
            .with_reasoning(ReasoningConfig {
                mode: ReasoningMode::Budget,
                enabled,
                effort: None,
                budget_tokens: Some(1024),
                summary: None,
            });
        let body = provider.build_request_body(&request);
        assert_eq!(body["max_tokens"], 4096);
        assert_eq!(
            body["thinking"],
            if enabled {
                json!({"type": "enabled", "budget_tokens": 1024})
            } else {
                json!({"type": "disabled"})
            }
        );
    }
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

#[test]
fn anthropic_request_uses_configured_max_tokens_when_request_is_unset() {
    let mut generate_kwargs = serde_json::Map::new();
    generate_kwargs.insert("max_tokens".into(), json!(4096));
    generate_kwargs.insert("temperature".into(), json!(0.2));

    let provider = provider_with_kwargs(generate_kwargs);
    let request = ChatRequest::new(vec![user_message("hello")]).with_model("claude-test");

    let body = provider.build_request_body(&request);

    assert_eq!(body["model"], "claude-test");
    assert_eq!(body["max_tokens"], 4096);
    assert_eq!(body["temperature"], 0.2);
}

#[test]
fn anthropic_request_precedence_keeps_explicit_request_over_provider_defaults() {
    let mut generate_kwargs = serde_json::Map::new();
    generate_kwargs.insert("max_tokens".into(), json!(4096));
    generate_kwargs.insert("temperature".into(), json!(0.2));

    let provider = provider_with_kwargs(generate_kwargs);
    let mut request = ChatRequest::new(vec![user_message("hello")])
        .with_model("claude-test")
        .with_max_tokens(512)
        .with_temperature(0.7);
    request.provider_options.insert("top_p".into(), json!(0.9));

    let body = provider.build_request_body(&request);

    assert_eq!(body["max_tokens"], 512);
    let temperature = body["temperature"]
        .as_f64()
        .expect("temperature should be numeric");
    assert!((temperature - 0.7).abs() < 1e-6);
    assert_eq!(body["top_p"], 0.9);
}

#[test]
fn anthropic_request_defaults_max_tokens_only_when_no_config_or_request_value_exists() {
    let provider = provider_with_kwargs(serde_json::Map::new());
    let request = ChatRequest::new(vec![user_message("hello")]).with_model("claude-test");

    let body = provider.build_request_body(&request);

    assert_eq!(body["max_tokens"], 2048);
}

#[test]
fn anthropic_request_translates_neutral_thinking_options() {
    let mut generate_kwargs = serde_json::Map::new();
    generate_kwargs.insert("max_tokens".into(), json!(4096));
    generate_kwargs.insert("thinking_enable".into(), json!(true));
    generate_kwargs.insert("thinking_budget".into(), json!(3072));

    let provider = provider_with_kwargs(generate_kwargs);
    let request = ChatRequest::new(vec![user_message("think")]).with_model("claude-test");

    let body = provider.build_request_body(&request);

    assert_eq!(
        body["thinking"],
        json!({
            "type": "enabled",
            "budget_tokens": 3072
        })
    );
    assert!(body.get("thinking_enable").is_none());
    assert!(body.get("thinking_budget").is_none());
}

#[test]
fn anthropic_request_translates_canonical_thinking_enabled() {
    let mut generate_kwargs = serde_json::Map::new();
    generate_kwargs.insert("max_tokens".into(), json!(4096));
    generate_kwargs.insert("thinking_enabled".into(), json!(true));
    generate_kwargs.insert("thinking_budget".into(), json!(3072));

    let provider = provider_with_kwargs(generate_kwargs);
    let request = ChatRequest::new(vec![user_message("think")]).with_model("claude-test");

    let body = provider.build_request_body(&request);

    assert_eq!(
        body["thinking"],
        json!({
            "type": "enabled",
            "budget_tokens": 3072
        })
    );
    assert!(body.get("thinking_enabled").is_none());
    assert!(body.get("thinking_budget").is_none());
}

#[test]
fn anthropic_protocol_translates_typed_reasoning() {
    let provider = provider_with_kwargs(serde_json::Map::new());
    let request = ChatRequest::new(vec![user_message("think")])
        .with_model("claude-test")
        .with_max_tokens(4096)
        .with_reasoning(ReasoningConfig {
            mode: ReasoningMode::Budget,
            enabled: true,
            effort: None,
            budget_tokens: Some(3072),
            summary: Some(ReasoningSummary::Auto),
        });

    let body = provider.build_request_body(&request);

    assert_eq!(
        body["thinking"],
        json!({
            "type": "enabled",
            "budget_tokens": 3072
        })
    );
}

#[test]
fn anthropic_request_disable_thinking_wins_over_budget() {
    let provider = provider_with_kwargs(serde_json::Map::new());
    let request = ChatRequest::new(vec![user_message("answer directly")])
        .with_model("claude-test")
        .with_provider_option("thinking_budget", json!(3072))
        .with_provider_option("disable_thinking", json!(true));

    let body = provider.build_request_body(&request);

    assert_eq!(body["thinking"], json!({"type": "disabled"}));
    assert!(body.get("disable_thinking").is_none());
    assert!(body.get("thinking_budget").is_none());
}

#[test]
fn anthropic_reasoning_budget_replaces_adaptive_effort_and_meets_minimum() {
    let provider = provider_with_kwargs(json!({"thinking":{"type":"adaptive"},"output_config":{"effort":"high","format":{"type":"json_schema"}}}).as_object().unwrap().clone());
    let request = ChatRequest::new(vec![user_message("hello")])
        .with_max_tokens(512)
        .with_reasoning(ReasoningConfig {
            mode: ReasoningMode::Budget,
            enabled: true,
            budget_tokens: Some(10),
            effort: None,
            summary: None,
        });
    let body = provider.build_request_body(&request);
    assert_eq!(
        body["thinking"],
        json!({"type":"enabled","budget_tokens":1024})
    );
    assert_eq!(body["max_tokens"], 2048);
    assert_eq!(
        body["output_config"],
        json!({"format":{"type":"json_schema"}})
    );
    let body = provider.build_request_body(
        &ChatRequest::new(vec![user_message("hello")])
            .with_provider_option("disable_thinking", json!(true)),
    );
    assert_eq!(body["thinking"], json!({"type":"disabled"}));
    assert!(body["output_config"].get("effort").is_none());
}

#[test]
fn anthropic_request_does_not_leak_gemini_tool_extra_content() {
    let provider = provider_with_kwargs(serde_json::Map::new());
    let request = ChatRequest::new(vec![
        user_message("Find the weather in Tokyo"),
        assistant_tool_use_with_gemini_metadata(),
    ])
    .with_model("claude-test");

    let body = provider.build_request_body(&request);
    let tool_use = &body["messages"][1]["content"][0];

    assert_eq!(
        tool_use,
        &json!({
            "type": "tool_use",
            "id": "tc-gemini-1",
            "name": "get_weather",
            "input": {"city": "Tokyo"}
        }),
        "Anthropic request projection must drop Gemini-only extra_content metadata"
    );
    assert!(
        !serde_json::to_string(&body)
            .expect("body json")
            .contains("gemini_sig_abc"),
        "Gemini thought signatures must not leak into Anthropic payloads: {body:?}"
    );
}

#[test]
fn anthropic_request_repairs_from_raw_arguments_without_leaking_raw_argument_fields() {
    let provider = provider_with_kwargs(serde_json::Map::new());
    let request = ChatRequest::new(vec![
        user_message("Read a file"),
        assistant_tool_use_with_raw_arguments(),
    ])
    .with_model("claude-test");

    let body = provider.build_request_body(&request);
    let tool_use = &body["messages"][1]["content"][0];

    assert_eq!(tool_use["input"], json!({"path": "/tmp/x"}));
    let serialized = serde_json::to_string(&body).expect("body json");
    assert!(!serialized.contains("raw_arguments"));
    assert!(!serialized.contains("raw_input"));
}

#[test]
fn anthropic_request_projects_tool_result_text_payload_inside_wire_block() {
    let provider = provider_with_kwargs(serde_json::Map::new());
    let mut result = agent_types::ToolResult::text("call-json", r#"{"timed_out":true}"#);
    result.name = Some("task_await".into());
    let message = ModelDialogueItem {
        id: None,
        role: Role::Tool,
        content: vec![ContentPart::ToolResult { result }],
        name: Some("task_await".into()),
        created_at: None,
        metadata: serde_json::Map::new(),
    };

    let converted = provider.convert_message(&message);

    assert_eq!(converted["content"][0]["type"], "tool_result");
    assert_eq!(converted["content"][0]["tool_use_id"], "call-json");
    assert_eq!(converted["content"][0]["content"][0]["type"], "text");
    assert_eq!(
        converted["content"][0]["content"][0]["text"],
        r#"{"timed_out":true}"#
    );
}
