use std::sync::Arc;
use std::time::Duration;

use agent_types::{ChatRequest, ModelDialogueItem, ReasoningConfig, ReasoningMode, Role};
use cell_store::FileSystemCellStore;
use futures::StreamExt;
use rua_provider::config_loader::{ProviderConfig, ProviderModelEntry};
use rua_provider::manager::{create_provider_from_config, ProviderManager};
use rua_provider::ProviderCells;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn config(provider: &str, model: &str) -> ProviderConfig {
    let public_id = match provider {
        "idealab-claude" => "anthropic",
        "idealab-gemini" => "google",
        other => other,
    };
    let mut template = ProviderManager::new()
        .list_templates()
        .iter()
        .find(|entry| entry.id == public_id)
        .unwrap()
        .clone();
    template.id = match provider {
        "idealab-claude" => "idealab-claude",
        "idealab-gemini" => "idealab-gemini",
        _ => template.id,
    };
    let mut config = template.to_config();
    config.models = vec![template.preset_model_entry(model)];
    config.api_key = "test-provider-key".into();
    config.timeout = Some(2);
    config
}

fn request() -> ChatRequest {
    ChatRequest::new(vec![ModelDialogueItem::text(Role::User, "hello")])
}

fn effort(enabled: bool, effort: &str) -> ReasoningConfig {
    ReasoningConfig {
        mode: ReasoningMode::Effort,
        enabled,
        effort: Some(effort.into()),
        budget_tokens: None,
        summary: None,
    }
}

async fn capture(config: ProviderConfig, request: ChatRequest, stream: bool) -> (String, Value) {
    capture_operation(config, request, stream, false).await
}

async fn capture_operation(
    mut config: ProviderConfig,
    request: ChatRequest,
    stream: bool,
    check_connection: bool,
) -> (String, Value) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    config.base_url = format!("http://{}", listener.local_addr().unwrap());
    let protocol = config.chat_model.clone().unwrap();
    let provider = create_provider_from_config(&config).unwrap();
    let model = provider.create_chat_model(&config.models[0].id).unwrap();
    let server = async {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0; 4096];
        let (headers, body) = loop {
            let read = socket.read(&mut buffer).await.unwrap();
            assert!(read > 0, "request closed before body");
            bytes.extend_from_slice(&buffer[..read]);
            if let Some(end) = bytes.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..end]).to_string();
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                    .unwrap_or(0);
                if bytes.len() >= end + 4 + length {
                    break (
                        headers,
                        serde_json::from_slice::<Value>(&bytes[end + 4..end + 4 + length]).unwrap(),
                    );
                }
            }
        };
        let (content_type, response) = if body["stream"] == true {
            ("text/event-stream", "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"OK\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n")
        } else {
            (
                "application/json",
                match protocol.as_str() {
                    "AnthropicChatModel" => {
                        r#"{"id":"msg-test","content":[{"type":"text","text":"OK"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}"#
                    }
                    "GeminiChatModel" => {
                        r#"{"candidates":[{"content":{"parts":[{"text":"OK"}]},"finishReason":"STOP"}]}"#
                    }
                    "OllamaChatModel" => {
                        r#"{"message":{"role":"assistant","content":"OK"},"done":true,"done_reason":"stop"}"#
                    }
                    "OpenAIResponseModel" => {
                        r#"{"id":"resp-test","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"OK"}]}]}"#
                    }
                    _ => {
                        r#"{"choices":[{"message":{"role":"assistant","content":"OK"},"finish_reason":"stop"}]}"#
                    }
                },
            )
        };
        socket.write_all(format!("HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{response}", response.len()).as_bytes()).await.unwrap();
        (headers, body)
    };
    let call = async {
        if check_connection {
            assert!(provider.check_connection().await.unwrap().is_connected());
        } else if stream {
            let events = model.chat_stream(request).collect::<Vec<_>>().await;
            assert!(events.iter().all(Result::is_ok), "{events:?}");
        } else {
            model.chat(request).await.unwrap();
        }
    };
    let (_, captured) =
        tokio::time::timeout(Duration::from_secs(3), async { tokio::join!(call, server) })
            .await
            .unwrap();
    captured
}

#[tokio::test]
async fn model_defaults_reach_each_protocol_as_wire_parameters() {
    for (provider, model, pointer, expected) in [
        (
            "openai",
            "gpt-5.6-sol",
            "/reasoning_effort",
            json!("medium"),
        ),
        (
            "openai-response",
            "gpt-5.6-sol",
            "/reasoning/effort",
            json!("medium"),
        ),
        ("kimi-cn", "kimi-k3", "/reasoning_effort", json!("low")),
        ("zhipu-cn", "glm-5.2", "/reasoning_effort", json!("high")),
        ("dashscope", "qwen3.8-max", "/thinking_budget", json!(8192)),
        (
            "aliyun-tokenplan",
            "glm-5.2",
            "/thinking/type",
            json!("enabled"),
        ),
        (
            "anthropic",
            "claude-opus-4-8",
            "/output_config/effort",
            json!("medium"),
        ),
        (
            "google",
            "gemini-3.1-pro-preview",
            "/generationConfig/thinkingConfig/thinkingLevel",
            json!("medium"),
        ),
        (
            "google",
            "gemini-2.5-flash",
            "/generationConfig/thinkingConfig/thinkingBudget",
            json!(8192),
        ),
    ] {
        let (_, body) = capture(config(provider, model), request(), false).await;
        assert_eq!(
            body.pointer(pointer),
            Some(&expected),
            "{provider}/{model}: {body}"
        );
        assert!(body.get("extra_body").is_none(), "{body}");
        assert!(
            body.get("metadata").is_none(),
            "internal reasoning metadata must stay local"
        );
        if provider == "dashscope" {
            assert_eq!(body["stream"], true, "Qwen thinking requires streaming");
        }
        if model == "kimi-k3" {
            assert!(
                body.get("thinking").is_none(),
                "Kimi K3 accepts effort only"
            );
        }
    }
}

#[tokio::test]
async fn chat_override_beats_nested_defaults_in_both_call_modes() {
    for stream in [false, true] {
        let mut cfg = config("zhipu-cn", "glm-5.2");
        cfg.generate_kwargs = json!({"extra_body":{"thinking":{"type":"enabled"},"reasoning_effort":"max","vendor":{"keep":1,"replace":0}}}).as_object().unwrap().clone();
        cfg.models[0].generate_kwargs = json!({"extra_body":{"vendor":{"from_model":2}}})
            .as_object()
            .unwrap()
            .clone();
        let call = request()
            .with_reasoning(effort(false, "none"))
            .with_provider_option("extra_body", json!({"vendor":{"replace":3}}));
        let (_, body) = capture(cfg, call, stream).await;
        assert_eq!(body["thinking"], json!({"type":"disabled"}));
        assert!(body.get("reasoning_effort").is_none());
        assert!(body.get("enable_thinking").is_none());
        assert_eq!(body["vendor"], json!({"keep":1,"from_model":2,"replace":3}));
        assert!(body.get("extra_body").is_none());
    }
}

#[tokio::test]
async fn explicit_raw_options_override_model_reasoning_defaults() {
    let (_, body) = capture(
        config("openai", "gpt-5.6-sol"),
        request().with_provider_option("reasoning_effort", json!("high")),
        false,
    )
    .await;
    assert_eq!(body["reasoning_effort"], "high");
}

#[tokio::test]
async fn request_sampling_and_limits_override_provider_and_model_defaults() {
    for (provider, model, limit, pointer) in [
        ("openai", "plain-model", "max_tokens", ""),
        ("openai-response", "plain-model", "max_output_tokens", ""),
        ("anthropic", "plain-model", "max_tokens", ""),
        ("idealab-claude", "plain-model", "max_tokens", ""),
        (
            "google",
            "plain-model",
            "maxOutputTokens",
            "/generationConfig",
        ),
        (
            "idealab-gemini",
            "plain-model",
            "maxOutputTokens",
            "/generationConfig",
        ),
    ] {
        let mut cfg = config(provider, model);
        cfg.generate_kwargs = json!({"temperature":0.1, limit:4096})
            .as_object()
            .unwrap()
            .clone();
        cfg.models[0].generate_kwargs = json!({"temperature":0.2, limit:2048})
            .as_object()
            .unwrap()
            .clone();
        let (_, defaults) = capture(cfg.clone(), request(), false).await;
        assert_eq!(
            defaults.pointer(pointer).unwrap()[limit],
            2048,
            "{provider}: {defaults}"
        );
        let call = request().with_temperature(0.7).with_max_tokens(512);
        let (_, body) = capture(cfg.clone(), call.clone(), false).await;
        let generation = body.pointer(pointer).unwrap();
        assert_eq!(generation[limit], 512, "{provider}: {body}");
        assert!(
            (generation["temperature"].as_f64().unwrap() - 0.7).abs() < 1e-6,
            "{provider}: {body}"
        );
        let (_, body) = capture(
            cfg,
            call.with_provider_option("temperature", json!(0.9)),
            false,
        )
        .await;
        assert_eq!(
            body.pointer(pointer).unwrap()["temperature"],
            0.9,
            "{provider}: {body}"
        );
    }
}

#[tokio::test]
async fn dashscope_raw_model_defaults_select_streaming_and_request_can_disable() {
    let mut cfg = config("dashscope", "plain-model");
    cfg.models[0].generate_kwargs =
        json!({"extra_body":{"enable_thinking":true,"thinking_budget":4096}})
            .as_object()
            .unwrap()
            .clone();
    let (_, body) = capture(cfg.clone(), request(), false).await;
    assert_eq!(body["stream"], true);
    assert_eq!(body["enable_thinking"], true);
    let (_, body) = capture(
        cfg,
        request().with_provider_option("disable_thinking", json!(true)),
        false,
    )
    .await;
    assert_eq!(body["stream"], false);
    assert_eq!(body["enable_thinking"], false);
    assert!(body.get("thinking_budget").is_none());
}

#[tokio::test]
async fn off_uses_supported_model_effort_and_gemini_control() {
    for (provider, model, pointer, expected) in [
        ("openai", "gpt-5.6-sol", "/reasoning_effort", "none"),
        (
            "openai-response",
            "gpt-5.6-luna",
            "/reasoning/effort",
            "minimal",
        ),
        ("openai", "o3", "/reasoning_effort", "low"),
        ("kimi-cn", "kimi-k3", "/reasoning_effort", "low"),
        (
            "google",
            "gemini-3.1-pro-preview",
            "/generationConfig/thinkingConfig/thinkingLevel",
            "low",
        ),
    ] {
        let (_, body) = capture(
            config(provider, model),
            request().with_reasoning(effort(false, "none")),
            false,
        )
        .await;
        assert_eq!(
            body.pointer(pointer),
            Some(&json!(expected)),
            "{provider}/{model}: {body}"
        );
    }
    let (_, body) = capture(
        config("google", "gemini-2.5-pro"),
        request().with_reasoning(ReasoningConfig {
            mode: ReasoningMode::Budget,
            enabled: false,
            effort: None,
            budget_tokens: None,
            summary: None,
        }),
        false,
    )
    .await;
    assert_eq!(
        body["generationConfig"]["thinkingConfig"]["thinkingBudget"],
        128
    );
}

#[tokio::test]
async fn unsupported_effort_fails_before_network_for_chat_and_stream() {
    let cfg = config("kimi-cn", "kimi-k3");
    let model = create_provider_from_config(&cfg)
        .unwrap()
        .create_chat_model("kimi-k3")
        .unwrap();
    let call = request().with_reasoning(effort(true, "medium"));
    assert!(model
        .chat(call.clone())
        .await
        .unwrap_err()
        .to_string()
        .contains("does not support reasoning effort 'medium'"));
    assert!(model
        .chat_stream(call)
        .next()
        .await
        .unwrap()
        .unwrap_err()
        .to_string()
        .contains("does not support reasoning effort 'medium'"));
}

#[tokio::test]
async fn cell_ingress_persists_defaults_and_preserves_explicit_settings() {
    let dir = tempfile::tempdir().unwrap();
    let manager = Arc::new(ProviderManager::new());
    let cells = ProviderCells::new(
        Arc::new(FileSystemCellStore::new()),
        manager.clone(),
        dir.path().into(),
    );
    let mut cfg = config("openai", "gpt-5.6-sol");
    cfg.models = vec![serde_json::from_value::<ProviderModelEntry>(
        json!({"id":"gpt-5.6-sol","name":"Sol","custom_metadata":17}),
    )
    .unwrap()];
    cells.cell("openai").write(cfg).await.unwrap();
    let stored: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("openai.json")).unwrap()).unwrap();
    assert_eq!(stored["models"][0]["thinking_enabled"], true);
    assert_eq!(stored["models"][0]["reasoning_effort"], "medium");
    assert_eq!(stored["models"][0]["custom_metadata"], 17);
    let model = manager
        .get("openai")
        .unwrap()
        .create_chat_model("gpt-5.6-sol")
        .unwrap();
    assert!(model
        .chat(request().with_reasoning(effort(true, "unsupported")))
        .await
        .unwrap_err()
        .to_string()
        .contains("does not support reasoning effort"));
    let updated = cells
        .cell("openai")
        .update(|config| {
            let mut config = config.unwrap();
            config.models[0].thinking_enabled = Some(false);
            config.models[0].reasoning_effort = Some("high".into());
            config
        })
        .await
        .unwrap();
    let loaded = cells.cell("openai").read().await.unwrap().unwrap();
    assert_eq!(updated.models[0].thinking_enabled, Some(false));
    assert_eq!(loaded.models[0].reasoning_effort.as_deref(), Some("high"));
}

#[tokio::test]
async fn unknown_model_does_not_get_reasoning_defaults() {
    for provider in ["openai", "anthropic", "google"] {
        let cfg = config(provider, "my-unknown-model");
        assert_eq!(cfg.models[0].thinking_enabled, None);
        for call in [
            request(),
            request().with_reasoning(effort(false, "none")),
            request().with_reasoning(ReasoningConfig {
                mode: ReasoningMode::Budget,
                enabled: true,
                effort: None,
                budget_tokens: None,
                summary: None,
            }),
        ] {
            let (_, body) = capture(cfg.clone(), call, false).await;
            for key in [
                "thinking",
                "reasoning",
                "enable_thinking",
                "reasoning_effort",
                "thinking_budget",
            ] {
                assert!(body.get(key).is_none());
            }
            assert!(body.pointer("/generationConfig/thinkingConfig").is_none());
        }
    }
}

#[tokio::test]
async fn auto_respects_raw_options_and_disabled_model_default() {
    let auto = ReasoningConfig {
        mode: ReasoningMode::Effort,
        enabled: true,
        effort: None,
        budget_tokens: None,
        summary: None,
    };
    let (_, body) = capture(
        config("openai", "gpt-5.6-sol"),
        request()
            .with_reasoning(auto.clone())
            .with_provider_option("reasoning_effort", json!("high")),
        false,
    )
    .await;
    assert_eq!(body["reasoning_effort"], "high");
    let mut cfg = config("openai", "gpt-5.6-sol");
    cfg.models[0].thinking_enabled = Some(false);
    let (_, body) = capture(cfg, request().with_reasoning(auto), false).await;
    assert_eq!(body["reasoning_effort"], "none");
}

#[tokio::test]
async fn ollama_native_think_option_overrides_model_defaults_in_auto() {
    let mut cfg = config("ollama", "custom-thinking-model");
    cfg.models[0].thinking_param_style = Some("effort".into());
    cfg.models[0].thinking_enabled = Some(true);
    cfg.models[0].reasoning_effort = Some("high".into());
    let (headers, body) = capture(
        cfg,
        request()
            .with_provider_option("think", json!(false))
            .with_reasoning(ReasoningConfig {
                mode: ReasoningMode::Effort,
                enabled: true,
                effort: None,
                budget_tokens: None,
                summary: None,
            }),
        false,
    )
    .await;
    assert!(headers.starts_with("POST /api/chat "));
    assert_eq!(body["think"], false);
}

#[tokio::test]
async fn raw_gemini_off_respects_model_minimum() {
    for (model, field, expected) in [
        ("gemini-3.1-pro-preview", "thinkingLevel", json!("low")),
        ("gemini-2.5-pro", "thinkingBudget", json!(128)),
    ] {
        let (_, body) = capture(
            config("google", model),
            request().with_provider_option("disable_thinking", json!(true)),
            false,
        )
        .await;
        assert_eq!(body["generationConfig"]["thinkingConfig"][field], expected);
    }
}

#[tokio::test]
async fn dashscope_custom_identity_and_headers_survive_protocol_construction() {
    let mut cfg = config("dashscope", "qwen3.8-max");
    cfg.id = "my-dashscope".into();
    cfg.name = "My DashScope".into();
    cfg.custom_headers.insert("x-custom".into(), json!("kept"));
    assert_eq!(
        create_provider_from_config(&cfg).unwrap().info().id,
        "my-dashscope"
    );
    let (headers, body) = capture(cfg, request(), false).await;
    assert!(headers.to_lowercase().contains("x-custom: kept"));
    assert_eq!(body["enable_thinking"], true);
}

#[tokio::test]
async fn github_checks_with_chat_and_openrouter_sends_attribution_headers() {
    let (headers, body) = capture_operation(
        config("github-models", "openai/gpt-4o-mini"),
        request(),
        false,
        true,
    )
    .await;
    assert!(headers.starts_with("POST /chat/completions "));
    assert_eq!(body["max_tokens"], 5);
    let (headers, _) = capture(config("openrouter", "test-model"), request(), false).await;
    let headers = headers.to_lowercase();
    assert!(headers.contains("x-openrouter-title: rua-rust"));
    assert!(headers.contains("x-openrouter-categories: personal-agent,cli-agent"));
}

#[tokio::test]
async fn budget_and_mode_validation_rejects_invalid_requests_before_network() {
    let cfg = config("google", "gemini-2.5-pro");
    let model = create_provider_from_config(&cfg)
        .unwrap()
        .create_chat_model("gemini-2.5-pro")
        .unwrap();
    let call = request().with_reasoning(ReasoningConfig {
        mode: ReasoningMode::Budget,
        enabled: true,
        effort: None,
        budget_tokens: Some(1),
        summary: None,
    });
    assert!(model
        .chat(call.clone())
        .await
        .unwrap_err()
        .to_string()
        .contains("between 128 and 32768"));
    assert!(model
        .chat_stream(call)
        .next()
        .await
        .unwrap()
        .unwrap_err()
        .to_string()
        .contains("between 128 and 32768"));
    assert!(model
        .chat(request().with_reasoning(effort(true, "high")))
        .await
        .unwrap_err()
        .to_string()
        .contains("requested reasoning mode"));
}
