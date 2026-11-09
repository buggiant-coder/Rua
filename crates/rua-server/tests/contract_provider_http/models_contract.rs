use axum::http::{Method, StatusCode};
use rua_agent::context_manager::{
    CompactContextRequest, CompactContextResult, ContextManager, ContextSummaryRequest,
    ContextSummaryResult,
};
use rua_agent::error::AgentError;
use rua_session::{
    ChatMetadata, ChatSessionHeader, NewConversationMessage, NewConversationTurn, TurnLifecycle,
    WorkspaceSessionStore,
};
use serde_json::json;
use std::sync::Arc;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::support::{build_app, provider_seed, read_json, request, seed_provider};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn models_list_returns_provider_dto_with_redacted_secret() {
    let test = build_app();
    seed_provider(
        &test.tmp,
        provider_seed("openai-contract", "OpenAI Contract"),
    );

    let resp = request(&test.app, Method::GET, "/api/models", None).await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let providers = body.as_array().expect("provider list must be an array");

    let provider = providers
        .iter()
        .find(|provider| provider["id"] == "openai-contract")
        .unwrap_or_else(|| panic!("expected seeded provider in response: {body:?}"));
    assert_eq!(provider["id"], "openai-contract");
    assert_eq!(provider["name"], "OpenAI Contract");
    assert_eq!(provider["base_url"], "https://api.example.com/v1");
    assert_eq!(provider["chat_model"], "OpenAIChatModel");
    assert_eq!(provider["is_custom"], true);
    assert_eq!(provider["require_api_key"], true);
    assert_eq!(provider["support_connection_check"], true);

    let api_key = provider["api_key"].as_str().expect("api_key string");
    assert_ne!(api_key, "sk-openai-contract-supersecret123456");
    assert!(
        api_key.contains("...") || api_key.contains("***"),
        "api key must be redacted in HTTP response, got {api_key:?}"
    );

    let models = provider["models"].as_array().expect("models array");
    assert_eq!(models[0]["id"], "model-primary");
    assert_eq!(models[0]["name"], "Model Primary");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn models_list_returns_qwenpaw_parity_provider_metadata() {
    let test = build_app();
    let mut seeded = provider_seed("github-models", "GitHub Models");
    seeded["api_key_prefixes"] = json!(["ghp_", "github_pat_"]);
    seeded["custom_headers"] = json!({
        "X-Provider-Trace": "rua"
    });
    seeded["provider_group"] = json!("github");
    seeded["provider_group_name"] = json!("GitHub");
    seeded["provider_variant"] = json!("models");
    seeded["base_url_options"] = json!([
        {"label": "GitHub Models", "value": "https://models.github.ai/inference"},
        {"label": "GitHub Models Preview", "value": "https://models.github.ai/preview"}
    ]);
    seeded["thinking_param_style"] = json!("effort");
    seeded["reasoning_effort_options"] = json!(["low", "medium", "high"]);
    seeded["thinking_budget_range"] = json!([1, 81920]);
    seed_provider(&test.tmp, seeded);

    let resp = request(&test.app, Method::GET, "/api/models", None).await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let providers = body.as_array().expect("provider list must be an array");
    let provider = providers
        .iter()
        .find(|provider| provider["id"] == "github-models")
        .unwrap_or_else(|| panic!("expected seeded GitHub Models provider in response: {body:?}"));
    assert_eq!(provider["api_key_prefixes"], json!(["ghp_", "github_pat_"]));
    assert_eq!(provider["custom_headers"]["X-Provider-Trace"], "rua");
    assert_eq!(provider["provider_group"], "github");
    assert_eq!(provider["provider_group_name"], "GitHub");
    assert_eq!(provider["provider_variant"], "models");
    assert_eq!(
        provider["base_url_options"],
        json!([
            {"label": "GitHub Models", "value": "https://models.github.ai/inference"},
            {"label": "GitHub Models Preview", "value": "https://models.github.ai/preview"}
        ])
    );
    assert_eq!(provider["thinking_param_style"], "effort");
    assert_eq!(
        provider["reasoning_effort_options"],
        json!(["low", "medium", "high"])
    );
    assert_eq!(provider["thinking_budget_range"], json!([1, 81920]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn models_list_marks_openrouter_as_free_oauth_provider_without_api_key() {
    let test = build_app();
    let mut seeded = provider_seed("openrouter", "OpenRouter");
    seeded["api_key"] = json!("");
    seed_provider(&test.tmp, seeded);

    let resp = request(&test.app, Method::GET, "/api/models", None).await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let providers = body.as_array().expect("provider list must be an array");
    let provider = providers
        .iter()
        .find(|provider| provider["id"] == "openrouter")
        .unwrap_or_else(|| panic!("expected openrouter provider in response: {body:?}"));

    assert_eq!(provider["supports_oauth"], true);
    assert_eq!(provider["is_free_tier"], true);
    assert_eq!(
        provider["oauth_connected"], false,
        "zero-config free-tier providers should be offered for OAuth before a key exists"
    );
    assert_eq!(
        provider["api_key"], "",
        "empty provider key should stay empty rather than being rendered as a redacted secret"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn models_list_includes_zero_config_free_providers_from_templates() {
    let test = build_app();

    let resp = request(&test.app, Method::GET, "/api/models", None).await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let providers = body.as_array().expect("provider list must be an array");
    let opencode = providers
        .iter()
        .find(|provider| provider["id"] == "opencode")
        .unwrap_or_else(|| panic!("expected built-in OpenCode provider in response: {body:?}"));
    assert_eq!(opencode["name"], "OpenCode");
    assert_eq!(opencode["base_url"], "https://opencode.ai/zen/v1");
    assert_eq!(opencode["require_api_key"], false);
    assert_eq!(opencode["is_free_tier"], true);
    assert_eq!(opencode["supports_oauth"], false);
    assert_eq!(
        opencode["models"]
            .as_array()
            .expect("opencode models")
            .iter()
            .map(|model| (
                model["id"].as_str().unwrap(),
                model["name"].as_str().unwrap(),
                model["is_free"].as_bool()
            ))
            .collect::<Vec<_>>(),
        vec![
            ("mimo-v2.5-free", "Mimo V2.5", Some(true)),
            ("nemotron-3-ultra-free", "Nemotron 3 Ultra", Some(true)),
        ]
    );

    let kilo = providers
        .iter()
        .find(|provider| provider["id"] == "kilo")
        .unwrap_or_else(|| panic!("expected built-in Kilo provider in response: {body:?}"));
    assert_eq!(kilo["name"], "Kilo Code");
    assert_eq!(kilo["base_url"], "https://api.kilo.ai/api/gateway");
    assert_eq!(kilo["require_api_key"], false);
    assert_eq!(kilo["freeze_url"], true);
    assert_eq!(kilo["is_free_tier"], true);
    assert_eq!(kilo["supports_oauth"], false);
    let kilo_models = kilo["models"].as_array().expect("kilo models");
    assert_eq!(kilo_models.len(), 6);
    assert_eq!(kilo_models[0]["id"], "kilo-auto/free");
    assert_eq!(kilo_models[0]["name"], "Kilo Auto (Free Router)");
    assert!(kilo_models
        .iter()
        .all(|model| model["is_free"].as_bool() == Some(true)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn startup_materializes_zero_config_free_providers() {
    let test = build_app();

    let opencode = test
        .state
        .provider_cells
        .cell("opencode")
        .read()
        .await
        .expect("read opencode provider")
        .expect("opencode provider config");
    assert_eq!(opencode.name, "OpenCode");
    assert_eq!(opencode.base_url, "https://opencode.ai/zen/v1");
    assert!(!opencode.require_api_key);
    assert!(test.state.provider_manager.get("opencode").is_some());
    assert!(test
        .state
        .provider_manager
        .chat_model("opencode", "deepseek-v4-flash-free")
        .is_ok());

    let kilo = test
        .state
        .provider_cells
        .cell("kilo")
        .read()
        .await
        .expect("read kilo provider")
        .expect("kilo provider config");
    assert_eq!(kilo.name, "Kilo Code");
    assert_eq!(kilo.base_url, "https://api.kilo.ai/api/gateway");
    assert!(!kilo.require_api_key);
    assert!(test.state.provider_manager.get("kilo").is_some());
    assert!(test
        .state
        .provider_manager
        .chat_model("kilo", "kilo-auto/free")
        .is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn models_list_marks_existing_template_provider_with_free_tier_metadata() {
    let test = build_app();
    seed_provider(
        &test.tmp,
        json!({
            "id": "opencode",
            "name": "OpenCode",
            "base_url": "https://opencode.ai/zen/v1",
            "api_key": "",
            "chat_model": "OpenAIChatModel",
            "models": [
                {"id": "deepseek-v4-flash-free", "name": "DeepSeek V4 Flash"}
            ],
            "extra_models": [],
            "is_local": false,
            "is_custom": false,
            "freeze_url": false,
            "require_api_key": false,
            "support_model_discovery": true,
            "support_connection_check": true,
        }),
    );

    let resp = request(&test.app, Method::GET, "/api/models", None).await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let providers = body.as_array().expect("provider list must be an array");
    let opencode = providers
        .iter()
        .find(|provider| provider["id"] == "opencode")
        .unwrap_or_else(|| panic!("expected seeded OpenCode provider in response: {body:?}"));
    assert_eq!(opencode["is_free_tier"], true);
    assert_eq!(opencode["supports_oauth"], false);
    assert_eq!(opencode["models"][0]["is_free"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn templates_return_model_entries_and_free_tier_metadata() {
    let test = build_app();

    let resp = request(&test.app, Method::GET, "/api/models/templates", None).await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let templates = body.as_array().expect("template list must be an array");
    let kilo = templates
        .iter()
        .find(|provider| provider["id"] == "kilo")
        .unwrap_or_else(|| panic!("expected Kilo template in response: {body:?}"));
    assert_eq!(kilo["is_free_tier"], true);
    assert_eq!(kilo["supports_oauth"], false);
    assert_eq!(
        kilo["preset_models"]
            .as_array()
            .expect("default models")
            .len(),
        6
    );
    let models = kilo["models"].as_array().expect("template models");
    assert_eq!(models[1]["id"], "nvidia/nemotron-3-ultra-550b-a55b:free");
    assert_eq!(models[1]["name"], "Nemotron 3 Ultra 550B");
    assert_eq!(models[1]["is_free"], true);

    let openrouter = templates
        .iter()
        .find(|provider| provider["id"] == "openrouter")
        .unwrap_or_else(|| panic!("expected OpenRouter template in response: {body:?}"));
    assert_eq!(openrouter["supports_oauth"], true);
    assert_eq!(openrouter["is_free_tier"], true);

    for (id, protocol) in [
        ("openai", "OpenAIChatModel"),
        ("anthropic", "AnthropicChatModel"),
        ("google", "GeminiChatModel"),
    ] {
        let provider = templates
            .iter()
            .find(|provider| provider["id"] == id)
            .unwrap_or_else(|| panic!("expected public template {id}"));
        assert_eq!(provider["chat_model"], protocol);
        assert!(provider.get("provider_adapter").is_none());
    }
    assert!(!templates.iter().any(|provider| provider["id"]
        .as_str()
        .is_some_and(|id| id.starts_with("idealab"))));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_creation_keeps_catalog_protocol_flags_and_model_defaults() {
    let test = build_app();
    for (id, protocol, discovery, check, local) in [
        ("anthropic", "AnthropicChatModel", true, true, false),
        ("minimax", "AnthropicChatModel", false, false, false),
        ("ollama", "OllamaChatModel", true, true, true),
        ("mimo", "OpenAIChatModel", true, true, false),
        (
            "volcengine-cn-agentplan",
            "OpenAIChatModel",
            false,
            false,
            false,
        ),
        ("dashscope", "DashScopeChatModel", true, true, false),
    ] {
        let response = request(
            &test.app,
            Method::POST,
            "/api/models/custom-providers",
            Some(json!({"id":id,"api_key_prefix":"test-prefix"})),
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::CREATED,
            "{id}: {}",
            read_json(response).await
        );
        let config = test
            .state
            .provider_cells
            .cell(id)
            .read()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(config.chat_model.as_deref(), Some(protocol), "{id}");
        assert_eq!(config.support_model_discovery, discovery, "{id}");
        assert_eq!(config.support_connection_check, check, "{id}");
        assert_eq!(config.is_local, local, "{id}");
        assert_eq!(config.api_key_prefix, "test-prefix");
        assert!(!config.base_url.is_empty(), "{id}");
        if id == "dashscope" {
            let qwen = config
                .models
                .iter()
                .find(|model| model.id == "qwen3.8-max")
                .unwrap();
            assert_eq!(qwen.thinking_enabled, Some(true));
            assert_eq!(qwen.thinking_budget, Some(8192));
            assert_eq!(qwen.thinking_param_style.as_deref(), Some("budget"));
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_creation_keeps_template_policy_when_preset_id_is_renamed() {
    let test = build_app();
    let response = request(
        &test.app,
        Method::POST,
        "/api/models/custom-providers",
        Some(json!({
            "id":"my-kimi", "name":"My Kimi", "template_id":"kimi-cn", "models":[{"id":"kimi-k3"}]
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let config = test
        .state
        .provider_cells
        .cell("my-kimi")
        .read()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(config.id, "my-kimi");
    assert!(config.is_custom);
    assert_eq!(config.template_id(), "kimi-cn");
    assert_eq!(config.models[0].extra["reasoning_format"], "effort");
    assert_eq!(
        config.models[0].reasoning_effort_options,
        ["low", "high", "max"]
    );
    assert_eq!(config.extra["merge_with_catalog"], true);

    let response = request(
        &test.app,
        Method::POST,
        "/api/models/custom-providers",
        Some(json!({
            "id":"my-agent-plan", "template_id":"volcengine-cn-agentplan"
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let config = test
        .state
        .provider_cells
        .cell("my-agent-plan")
        .read()
        .await
        .unwrap()
        .unwrap();
    assert!(!config.support_model_discovery);
    assert!(!config.support_connection_check);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_discovery_filters_non_chat_and_unavailable_catalog_entries() {
    let test = build_app();
    for (id, candidates, expected) in [
        (
            "mimo",
            vec!["mimo-v2.5", "mimo-v2.5-tts", "embedding-v1"],
            vec!["mimo-v2.5"],
        ),
        (
            "modelscope",
            vec!["Qwen/Qwen3.5", "Qwen/Qwen3-PT", "Qwen/Qwen3-Embedding"],
            vec!["Qwen/Qwen3.5"],
        ),
        (
            "dashscope",
            vec!["qwen3.8-max", "qwen-image", "qwen3-asr"],
            vec!["qwen3.8-max"],
        ),
        (
            "opencode",
            vec![
                "mimo-v2.5-free",
                "deepseek-v4-flash-free",
                "nemotron-3-super-free",
            ],
            vec!["mimo-v2.5-free"],
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":candidates.into_iter().map(|id| json!({"id":id,"max_context_length":120000,"max_output_tokens":8192})).collect::<Vec<_>>()})))
            .expect(1).mount(&server).await;
        let template = rua_provider::manager::PROVIDER_TEMPLATES
            .iter()
            .find(|template| template.id == id)
            .unwrap();
        let mut config = template.to_config();
        config.base_url = server.uri();
        config.api_key = "test-key".into();
        test.state
            .provider_cells
            .cell(id)
            .write(config)
            .await
            .unwrap();
        let models = test
            .state
            .provider_manager
            .discover_models(id)
            .await
            .unwrap();
        assert_eq!(
            models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            expected,
            "{id}"
        );
        assert_eq!(models[0].context_window, Some(120000));
        assert_eq!(models[0].max_output_tokens, Some(8192));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catalog_only_providers_never_call_model_discovery_endpoints() {
    let test = build_app();
    let server = MockServer::start().await;
    for id in ["minimax", "kimi-codingplan", "volcengine-cn-agentplan"] {
        let template = rua_provider::manager::PROVIDER_TEMPLATES
            .iter()
            .find(|template| template.id == id)
            .unwrap();
        let mut config = template.to_config();
        config.base_url = server.uri();
        test.state
            .provider_cells
            .cell(id)
            .write(config)
            .await
            .unwrap();
        let models = test
            .state
            .provider_manager
            .discover_models(id)
            .await
            .unwrap();
        assert_eq!(
            models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            template.preset_models
        );
        assert!(test
            .state
            .provider_manager
            .check_connection(id)
            .await
            .unwrap()
            .is_connected());
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn setting_zero_config_free_model_materializes_provider_config() {
    let test = build_app();

    let resp = request(
        &test.app,
        Method::PUT,
        "/api/models/providers/active",
        Some(json!({
            "provider_id": "kilo",
            "model": "kilo-auto/free"
        })),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["provider_id"], "kilo");
    assert_eq!(body["model"], "kilo-auto/free");

    let resp = request(&test.app, Method::GET, "/api/models", None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let providers = body.as_array().expect("provider list must be an array");
    let kilo = providers
        .iter()
        .find(|provider| provider["id"] == "kilo")
        .unwrap_or_else(|| panic!("expected materialized Kilo provider: {body:?}"));
    assert_eq!(kilo["require_api_key"], false);
    assert_eq!(kilo["models"][0]["id"], "kilo-auto/free");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_update_preserves_frozen_url_and_redacted_secret_placeholders() {
    let test = build_app();
    let mut seeded = provider_seed("openai-contract", "OpenAI Contract");
    seeded["is_custom"] = json!(false);
    seeded["freeze_url"] = json!(true);
    seeded["generate_kwargs"] = json!({
        "temperature": 0.1,
        "top_p": 0.8
    });
    seed_provider(&test.tmp, seeded);

    let resp = request(
        &test.app,
        Method::PUT,
        "/api/models/providers/openai-contract",
        Some(json!({
            "name": "OpenAI Contract Renamed",
            "base_url": "https://blocked.example/v1",
            "api_key": "sk-******",
            "generate_kwargs": {
                "temperature": 0.2,
                "top_p": 0.9
            }
        })),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["name"], "OpenAI Contract Renamed");
    assert_eq!(
        body["base_url"], "https://api.example.com/v1",
        "freeze_url=true providers must ignore base_url updates"
    );
    assert_eq!(body["generate_kwargs"]["temperature"], json!(0.2));
    assert_eq!(body["generate_kwargs"]["top_p"], json!(0.9));
    assert_ne!(body["api_key"], "sk-openai-contract-supersecret123456");

    let path = test
        .tmp
        .path()
        .join("secret/providers/providers/openai-contract.json");
    let on_disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read provider"))
            .expect("provider json");
    assert_eq!(on_disk["base_url"], "https://api.example.com/v1");
    assert_eq!(on_disk["generate_kwargs"]["temperature"], json!(0.2));

    let decrypted = test
        .state
        .provider_cells
        .cell("openai-contract")
        .read()
        .await
        .expect("read provider cell")
        .expect("provider config");
    assert_eq!(decrypted.api_key, "sk-openai-contract-supersecret123456");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn model_context_window_update_matches_saved_config_and_live_runtime() {
    let test = build_app();
    let mut seeded = provider_seed("context-contract", "Context Contract");
    seeded["chat_model"] = json!("OpenAIResponseModel");
    seeded["models"][0]["context_window"] = json!(131_072);
    seeded["extra_models"] = json!([{
        "id": "vendor/extra-model",
        "name": "Extra Model",
        "context_window": 131_072
    }]);
    seed_provider(&test.tmp, seeded);

    for (model_id, model_list, route) in [
        (
            "model-primary",
            "models",
            "/api/models/context-contract/models/model-primary/config",
        ),
        (
            "vendor/extra-model",
            "extra_models",
            "/api/models/context-contract/models/config",
        ),
    ] {
        let resp = request(
            &test.app,
            Method::PUT,
            route,
            Some(json!({
                "model_id": model_id,
                "context_window": 1_000_000,
                "display_group": "context-test"
            })),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = read_json(resp).await;
        assert_eq!(body[model_list][0]["context_window"], 1_000_000);
        assert_eq!(body[model_list][0]["display_group"], "context-test");

        let saved = test
            .state
            .provider_cells
            .cell("context-contract")
            .read()
            .await
            .expect("read saved provider")
            .expect("saved provider");
        let saved_model = saved
            .all_models()
            .into_iter()
            .find(|model| model.id == model_id)
            .expect("saved model");
        assert_eq!(saved_model.context_window, Some(1_000_000));

        let live_model = test
            .state
            .provider_manager
            .chat_model("context-contract", model_id)
            .expect("live model after update");
        assert_eq!(
            live_model.model_info().context_window,
            saved_model.context_window,
            "the next Turn must use the context window shown by model settings"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn model_context_window_update_distinguishes_omission_from_null() {
    let test = build_app();
    let mut seeded = provider_seed("context-contract", "Context Contract");
    seeded["models"][0]["context_window"] = json!(1_000_000);
    seed_provider(&test.tmp, seeded);

    for (update, expected) in [
        (json!({"name": "Renamed Model"}), Some(1_000_000)),
        (json!({"context_window": null}), None),
    ] {
        let resp = request(
            &test.app,
            Method::PUT,
            "/api/models/context-contract/models/model-primary/config",
            Some(update),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            read_json(resp).await["models"][0]["context_window"],
            json!(expected)
        );
        let saved = test
            .state
            .provider_cells
            .cell("context-contract")
            .read()
            .await
            .expect("read saved provider")
            .expect("saved provider");
        assert_eq!(saved.models[0].context_window, expected);
        let live_model = test
            .state
            .provider_manager
            .chat_model("context-contract", "model-primary")
            .expect("live model after update");
        assert_eq!(live_model.model_info().context_window, expected);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_test_preserves_real_key_when_candidate_is_redacted() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header(
            "authorization",
            "Bearer sk-real-provider-secret123456",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": []})))
        .mount(&upstream)
        .await;

    let test = build_app();
    let mut seeded = provider_seed("masked-key-test", "Masked Key Test");
    seeded["base_url"] = json!(format!("{}/v1", upstream.uri()));
    seeded["api_key"] = json!("sk-real-provider-secret123456");
    seed_provider(&test.tmp, seeded);
    test.state
        .provider_manager
        .reload_provider("masked-key-test")
        .await
        .expect("reload provider");

    let resp = request(
        &test.app,
        Method::POST,
        "/api/models/masked-key-test/test",
        Some(json!({
            "api_key": "sk-r...3456"
        })),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["success"], true, "unexpected test response: {body:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_model_effective_scope_returns_default_llm_key() {
    let test = build_app();

    let resp = request(&test.app, Method::GET, "/api/models/active", None).await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert!(body.is_object());
    assert!(
        body.get("default_llm").is_some(),
        "active model response must include default_llm: {body:?}"
    );
    assert!(body["default_llm"].is_object());
    assert_eq!(body["default_llm"]["provider_id"], "");
    assert_eq!(body["default_llm"]["model"], "");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_model_global_scope_returns_same_contract() {
    let test = build_app();

    let resp = request(
        &test.app,
        Method::GET,
        "/api/models/active?scope=global",
        None,
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert!(body.is_object());
    assert!(body.get("default_llm").is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn store_default_model_rejects_unknown_provider_with_detail() {
    let test = build_app();

    let resp = request(
        &test.app,
        Method::PUT,
        "/api/models/active",
        Some(json!({
            "provider_id": "integ_unknown_provider_xyz",
            "model": "integ_model_zzz",
            "scope": "global"
        })),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = read_json(resp).await;
    assert!(
        body["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("integ_unknown_provider_xyz")
                || detail.to_lowercase().contains("not found")),
        "expected FastAPI-compatible detail body, got {body:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn store_default_model_with_chat_id_requires_confirmation_when_projection_exceeds_target_model(
) {
    let test = build_app();
    let create_provider = request(
        &test.app,
        Method::POST,
        "/api/models/custom-providers",
        Some(json!({
            "id": "tiny-context-provider",
            "name": "Tiny Context Provider",
            "base_url": "https://llm.example.com/v1",
            "api_key": "sk-tiny-context-secret123456",
            "chat_model": "OpenAIChatModel",
            "models": [
                {
                    "id": "tiny-context",
                    "name": "Tiny Context",
                    "context_window": 64
                }
            ]
        })),
    )
    .await;
    assert_eq!(create_provider.status(), StatusCode::CREATED);

    seed_long_evergreen_chat(&test.tmp, "chat-preflight").await;

    let resp = request(
        &test.app,
        Method::PUT,
        "/api/models/providers/active",
        Some(json!({
            "provider_id": "tiny-context-provider",
            "model": "tiny-context",
            "scope": "global",
            "chat_id": "chat-preflight"
        })),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = read_json(resp).await;
    assert_eq!(body["code"], "context_checkpoint_required");
    assert_eq!(body["chat_id"], "chat-preflight");
    assert_eq!(body["target_model"]["provider_id"], "tiny-context-provider");
    assert_eq!(body["target_model"]["model"], "tiny-context");
    assert!(
        body["estimated_projection_tokens"]
            .as_u64()
            .unwrap_or_default()
            > body["target_upper_tokens"].as_u64().unwrap_or_default(),
        "preflight response should explain why the target model is unsafe: {body:?}"
    );
    assert!(
        body["recommended_fold_frontier"].is_object(),
        "dangerous switch should recommend a fold frontier: {body:?}"
    );

    let active = request(&test.app, Method::GET, "/api/models/active", None).await;
    assert_eq!(active.status(), StatusCode::OK);
    let active = read_json(active).await;
    assert_eq!(active["default_llm"]["provider_id"], "");
    assert_eq!(active["default_llm"]["model"], "");

    let workspace = test.tmp.path().join("workspaces/default");
    let mut profile: rua_config::AgentProfileConfig =
        serde_json::from_slice(&std::fs::read(workspace.join("agent.json")).unwrap()).unwrap();
    profile.running.max_input_length = 1_000;
    profile.running.context_compact.token_count_estimate_divisor = 1_000.0;
    rua_config::loader::save_agent_config(&profile, &workspace).unwrap();
    let changed_default = request(
        &test.app,
        Method::PUT,
        "/api/models/providers/active",
        Some(json!({
            "provider_id": "tiny-context-provider", "model":"tiny-context", "scope":"global"
        })),
    )
    .await;
    assert_eq!(changed_default.status(), StatusCode::OK);
    let repeated = request(&test.app, Method::PUT, "/api/models/providers/active", Some(json!({
        "provider_id": "tiny-context-provider", "model":"tiny-context", "scope":"global", "chat_id":"chat-preflight"
    }))).await;
    assert_eq!(repeated.status(), StatusCode::CONFLICT);
    assert_eq!(
        read_json(repeated).await,
        body,
        "default edits must not change existing Chat preflight"
    );
    let header = WorkspaceSessionStore::new(&workspace)
        .load_chat_header("chat-preflight")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        rua_chat::model_slot_from_header(&header).unwrap()["model"],
        "frozen-model"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn store_default_model_with_unmaterialized_chat_id_skips_context_preflight() {
    let test = build_app();
    let create_provider = request(
        &test.app,
        Method::POST,
        "/api/models/custom-providers",
        Some(json!({
            "id": "new-chat-provider",
            "name": "New Chat Provider",
            "base_url": "https://llm.example.com/v1",
            "api_key": "sk-new-chat-secret123456",
            "chat_model": "OpenAIChatModel",
            "models": [
                {
                    "id": "new-chat-model",
                    "name": "New Chat Model",
                    "context_window": 64
                }
            ]
        })),
    )
    .await;
    assert_eq!(create_provider.status(), StatusCode::CREATED);

    let resp = request(
        &test.app,
        Method::PUT,
        "/api/models/providers/active",
        Some(json!({
            "provider_id": "new-chat-provider",
            "model": "new-chat-model",
            "scope": "session",
            "chat_id": "2e6f79ef-b903-4bfb-acdd-2f84d038f523"
        })),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["provider_id"], "new-chat-provider");
    assert_eq!(body["model"], "new-chat-model");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn confirmed_model_switch_creates_checkpoint_before_applying_target_model() {
    let test = build_app();
    let runtime = test
        .state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime");
    runtime
        .set_context_manager(Arc::new(StaticSummaryContextManager))
        .await;

    let create_provider = request(
        &test.app,
        Method::POST,
        "/api/models/custom-providers",
        Some(json!({
            "id": "confirmed-tiny-context-provider",
            "name": "Confirmed Tiny Context Provider",
            "base_url": "https://llm.example.com/v1",
            "api_key": "sk-confirmed-tiny-context-secret123456",
            "chat_model": "OpenAIChatModel",
            "models": [
                {
                    "id": "confirmed-tiny-context",
                    "name": "Confirmed Tiny Context",
                    "context_window": 280
                }
            ]
        })),
    )
    .await;
    assert_eq!(create_provider.status(), StatusCode::CREATED);

    seed_long_evergreen_chat(&test.tmp, "chat-confirmed-preflight").await;

    let resp = request(
        &test.app,
        Method::PUT,
        "/api/models/providers/active",
        Some(json!({
            "provider_id": "confirmed-tiny-context-provider",
            "model": "confirmed-tiny-context",
            "scope": "global",
            "chat_id": "chat-confirmed-preflight",
            "confirm_context_checkpoint": true
        })),
    )
    .await;

    let status = resp.status();
    let body = read_json(resp).await;
    assert_eq!(status, StatusCode::OK, "response: {body:?}");
    assert_eq!(body["provider_id"], "confirmed-tiny-context-provider");
    assert_eq!(body["model"], "confirmed-tiny-context");

    let active = request(&test.app, Method::GET, "/api/models/active", None).await;
    assert_eq!(active.status(), StatusCode::OK);
    let active = read_json(active).await;
    assert_eq!(
        active["default_llm"]["provider_id"],
        "confirmed-tiny-context-provider"
    );
    assert_eq!(active["default_llm"]["model"], "confirmed-tiny-context");

    let store = WorkspaceSessionStore::new(test.tmp.path().join("workspaces").join("default"));
    let checkpoint = store
        .load_active_summary_checkpoint("chat-confirmed-preflight")
        .await
        .unwrap()
        .expect("confirmed switch should publish checkpoint");
    assert_eq!(checkpoint.summary, "compact ok");
    assert_eq!(checkpoint.model.provider, "confirmed-tiny-context-provider");
    assert_eq!(checkpoint.model.model, "confirmed-tiny-context");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn confirmed_model_switch_keeps_previous_model_when_checkpoint_fails() {
    let test = build_app();
    let runtime = test
        .state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime");
    runtime
        .set_context_manager(Arc::new(FailingSummaryContextManager))
        .await;

    let create_provider = request(
        &test.app,
        Method::POST,
        "/api/models/custom-providers",
        Some(json!({
            "id": "failing-switch-provider",
            "name": "Failing Switch Provider",
            "base_url": "https://llm.example.com/v1",
            "api_key": "sk-failing-switch-secret123456",
            "chat_model": "OpenAIChatModel",
            "models": [
                {
                    "id": "large-current",
                    "name": "Large Current",
                    "context_window": 4096
                },
                {
                    "id": "tiny-target",
                    "name": "Tiny Target",
                    "context_window": 64
                }
            ]
        })),
    )
    .await;
    assert_eq!(create_provider.status(), StatusCode::CREATED);

    let set_current = request(
        &test.app,
        Method::PUT,
        "/api/models/providers/active",
        Some(json!({
            "provider_id": "failing-switch-provider",
            "model": "large-current",
            "scope": "global"
        })),
    )
    .await;
    assert_eq!(set_current.status(), StatusCode::OK);

    seed_long_evergreen_chat(&test.tmp, "chat-failing-preflight").await;

    let resp = request(
        &test.app,
        Method::PUT,
        "/api/models/providers/active",
        Some(json!({
            "provider_id": "failing-switch-provider",
            "model": "tiny-target",
            "scope": "global",
            "chat_id": "chat-failing-preflight",
            "confirm_context_checkpoint": true
        })),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let active = request(&test.app, Method::GET, "/api/models/active", None).await;
    assert_eq!(active.status(), StatusCode::OK);
    let active = read_json(active).await;
    assert_eq!(
        active["default_llm"]["provider_id"],
        "failing-switch-provider"
    );
    assert_eq!(active["default_llm"]["model"], "large-current");

    let store = WorkspaceSessionStore::new(test.tmp.path().join("workspaces").join("default"));
    assert!(store
        .load_active_summary_checkpoint("chat-failing-preflight")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn custom_provider_and_model_lifecycle_uses_rust_management_routes() {
    let test = build_app();

    let create = request(
        &test.app,
        Method::POST,
        "/api/models/custom-providers",
        Some(json!({
            "id": "custom-contract",
            "name": "Custom Contract",
            "base_url": "https://llm.example.com/v1",
            "api_key": "sk-custom-contract-secret123456",
            "chat_model": "OpenAIChatModel",
            "models": [
                {"id": "contract-mini", "name": "Contract Mini"}
            ]
        })),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);
    let created = read_json(create).await;
    assert_eq!(created["id"], "custom-contract");
    assert_eq!(created["name"], "Custom Contract");
    assert_eq!(created["chat_model"], "OpenAIChatModel");
    assert_eq!(created["models"][0]["id"], "contract-mini");
    assert_ne!(created["api_key"], "sk-custom-contract-secret123456");

    let duplicate = request(
        &test.app,
        Method::POST,
        "/api/models/custom-providers",
        Some(json!({
            "id": "custom-contract",
            "name": "Duplicate",
            "base_url": "https://dup.example.com",
            "api_key": "sk-duplicate",
            "models": []
        })),
    )
    .await;
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);
    assert!(read_json(duplicate).await["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains("already exists")));

    let get = request(
        &test.app,
        Method::GET,
        "/api/models/providers/custom-contract",
        None,
    )
    .await;
    assert_eq!(get.status(), StatusCode::OK);
    assert_eq!(read_json(get).await["id"], "custom-contract");

    let update = request(
        &test.app,
        Method::PUT,
        "/api/models/providers/custom-contract",
        Some(json!({
            "name": "Custom Contract Renamed",
            "base_url": "https://llm-renamed.example.com/v1",
            "api_key": created["api_key"].clone(),
            "models": [
                {"id": "contract-mini", "name": "Contract Mini Renamed"}
            ]
        })),
    )
    .await;
    assert_eq!(update.status(), StatusCode::OK);
    let updated = read_json(update).await;
    assert_eq!(updated["name"], "Custom Contract Renamed");
    assert_eq!(updated["base_url"], "https://llm-renamed.example.com/v1");
    assert_eq!(updated["models"][0]["name"], "Contract Mini Renamed");
    assert_ne!(updated["api_key"], "sk-custom-contract-secret123456");

    let add_model = request(
        &test.app,
        Method::POST,
        "/api/models/custom-contract/models",
        Some(json!({
            "id": "contract-large",
            "name": "Contract Large",
            "supports_image": true
        })),
    )
    .await;
    assert_eq!(add_model.status(), StatusCode::CREATED);
    let after_add = read_json(add_model).await;
    let model_ids: Vec<_> = after_add["models"]
        .as_array()
        .expect("models")
        .iter()
        .filter_map(|model| model["id"].as_str())
        .collect();
    assert!(model_ids.contains(&"contract-large"));

    let update_model = request(
        &test.app,
        Method::PUT,
        "/api/models/custom-contract/models/contract-large/config",
        Some(json!({
            "name": "Contract Large Updated",
            "display_group": "contract",
            "thinking_enabled": true,
            "thinking_param_style": "effort",
            "reasoning_effort": "high",
            "reasoning_effort_options": ["low", "medium", "high"],
            "thinking_budget_range": [128, 4096]
        })),
    )
    .await;
    assert_eq!(update_model.status(), StatusCode::OK);
    let after_update_model = read_json(update_model).await;
    let large = after_update_model["models"]
        .as_array()
        .expect("models")
        .iter()
        .find(|model| model["id"] == "contract-large")
        .expect("contract-large");
    assert_eq!(large["name"], "Contract Large Updated");
    assert_eq!(large["display_group"], "contract");
    assert_eq!(large["thinking_enabled"], true);
    assert_eq!(large["thinking_param_style"], "effort");
    assert_eq!(large["reasoning_effort"], "high");
    assert_eq!(
        large["reasoning_effort_options"],
        json!(["low", "medium", "high"])
    );
    assert_eq!(large["thinking_budget_range"], json!([128, 4096]));

    let delete_model = request(
        &test.app,
        Method::DELETE,
        "/api/models/custom-contract/models/contract-large",
        None,
    )
    .await;
    assert_eq!(delete_model.status(), StatusCode::OK);
    let after_delete_model = read_json(delete_model).await;
    assert!(!after_delete_model["models"]
        .as_array()
        .expect("models")
        .iter()
        .any(|model| model["id"] == "contract-large"));

    let add_slash_model = request(
        &test.app,
        Method::POST,
        "/api/models/custom-contract/models",
        Some(json!({
            "id": "vendor/contract-large",
            "name": "Vendor Contract Large"
        })),
    )
    .await;
    assert_eq!(add_slash_model.status(), StatusCode::CREATED);

    let update_slash_model = request(
        &test.app,
        Method::PUT,
        "/api/models/custom-contract/models/config",
        Some(json!({
            "model_id": "vendor/contract-large",
            "name": "Vendor Contract Large Updated",
            "thinking_enabled": true,
            "thinking_param_style": "effort",
            "reasoning_effort": "medium"
        })),
    )
    .await;
    assert_eq!(update_slash_model.status(), StatusCode::OK);
    let after_update_slash_model = read_json(update_slash_model).await;
    let slash_model = after_update_slash_model["models"]
        .as_array()
        .expect("models")
        .iter()
        .find(|model| model["id"] == "vendor/contract-large")
        .expect("vendor/contract-large");
    assert_eq!(slash_model["name"], "Vendor Contract Large Updated");
    assert_eq!(slash_model["reasoning_effort"], "medium");

    let delete_slash_model = request(
        &test.app,
        Method::POST,
        "/api/models/custom-contract/models/delete",
        Some(json!({
            "model_id": "vendor/contract-large"
        })),
    )
    .await;
    assert_eq!(delete_slash_model.status(), StatusCode::OK);
    let after_delete_slash_model = read_json(delete_slash_model).await;
    assert!(!after_delete_slash_model["models"]
        .as_array()
        .expect("models")
        .iter()
        .any(|model| model["id"] == "vendor/contract-large"));

    let add_catalog_alias_model = request(
        &test.app,
        Method::POST,
        "/api/models/custom-contract/models",
        Some(json!({
            "id": "opus-4.8",
            "name": "Opus Alias"
        })),
    )
    .await;
    assert_eq!(add_catalog_alias_model.status(), StatusCode::CREATED);
    let after_add_catalog_alias_model = read_json(add_catalog_alias_model).await;
    let catalog_alias_model = after_add_catalog_alias_model["models"]
        .as_array()
        .expect("models")
        .iter()
        .find(|model| model["id"] == "opus-4.8")
        .expect("opus-4.8");
    assert_eq!(catalog_alias_model["id"], "opus-4.8");
    assert_eq!(catalog_alias_model["context_window"], 1_000_000);
    assert_eq!(catalog_alias_model["max_output_tokens"], 32_768);
    assert_eq!(catalog_alias_model["supports_image"], true);
    assert_eq!(catalog_alias_model["supports_video"], false);
    assert_eq!(catalog_alias_model["supports_multimodal"], true);
    assert!(catalog_alias_model["thinking_param_style"].is_null());
    assert!(catalog_alias_model["thinking_budget_range"].is_null());
    assert_eq!(catalog_alias_model["reasoning_effort_options"], json!([]));

    let add_extra_slash_model = request(
        &test.app,
        Method::PUT,
        "/api/models/providers/custom-contract",
        Some(json!({
            "extra_models": [
                {
                    "id": "vendor/extra-contract",
                    "name": "Vendor Extra Contract"
                }
            ]
        })),
    )
    .await;
    assert_eq!(add_extra_slash_model.status(), StatusCode::OK);

    let duplicate_extra_model = request(
        &test.app,
        Method::POST,
        "/api/models/custom-contract/models",
        Some(json!({
            "id": "vendor/extra-contract",
            "name": "Duplicate Vendor Extra Contract"
        })),
    )
    .await;
    assert_eq!(duplicate_extra_model.status(), StatusCode::CONFLICT);

    let update_extra_slash_model = request(
        &test.app,
        Method::PUT,
        "/api/models/custom-contract/models/vendor%2Fextra-contract/config",
        Some(json!({
            "name": "Vendor Extra Contract Updated",
            "thinking_enabled": true,
            "thinking_param_style": "effort",
            "reasoning_effort": "high"
        })),
    )
    .await;
    assert_eq!(update_extra_slash_model.status(), StatusCode::OK);
    let after_update_extra_slash_model = read_json(update_extra_slash_model).await;
    let extra_slash_model = after_update_extra_slash_model["extra_models"]
        .as_array()
        .expect("extra_models")
        .iter()
        .find(|model| model["id"] == "vendor/extra-contract")
        .expect("vendor/extra-contract");
    assert_eq!(extra_slash_model["name"], "Vendor Extra Contract Updated");
    assert_eq!(extra_slash_model["reasoning_effort"], "high");

    let delete_provider = request(
        &test.app,
        Method::DELETE,
        "/api/models/providers/custom-contract",
        None,
    )
    .await;
    assert_eq!(delete_provider.status(), StatusCode::OK);
    assert_eq!(
        read_json(delete_provider).await,
        json!({
            "success": true,
            "message": "Provider 'custom-contract' deleted"
        })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn custom_provider_preserves_protocol_adapter_metadata() {
    let test = build_app();

    let create = request(
        &test.app,
        Method::POST,
        "/api/models/custom-providers",
        Some(json!({
            "id": "idealab-anthropic-contract",
            "name": "Idealab Anthropic Contract",
            "base_url": "https://gateway.example.test/api/anthropic",
            "api_key": "idealab-secret",
            "chat_model": "AnthropicChatModel",
            "provider_adapter": "idealab-anthropic",
            "models": [
                {"id": "claude-opus-4-6", "name": "Claude Opus 4.6"}
            ]
        })),
    )
    .await;

    assert_eq!(create.status(), StatusCode::CREATED);
    let created = read_json(create).await;
    assert_eq!(created["chat_model"], "AnthropicChatModel");
    assert_eq!(created["provider_adapter"], "idealab-anthropic");

    let get = request(
        &test.app,
        Method::GET,
        "/api/models/providers/idealab-anthropic-contract",
        None,
    )
    .await;
    assert_eq!(get.status(), StatusCode::OK);
    assert_eq!(
        read_json(get).await["provider_adapter"],
        "idealab-anthropic"
    );
}

struct StaticSummaryContextManager;

#[async_trait::async_trait]
impl ContextManager for StaticSummaryContextManager {
    async fn compact_context(
        &self,
        req: CompactContextRequest,
    ) -> Result<CompactContextResult, AgentError> {
        Ok(CompactContextResult {
            compacted_messages: req.messages,
            tokens_saved: 0,
            summary_generated: None,
            grade: None,
        })
    }

    async fn summarize_context(
        &self,
        _req: ContextSummaryRequest,
    ) -> Result<ContextSummaryResult, AgentError> {
        Ok(ContextSummaryResult {
            summary: "compact ok".to_string(),
        })
    }
}

struct FailingSummaryContextManager;

#[async_trait::async_trait]
impl ContextManager for FailingSummaryContextManager {
    async fn compact_context(
        &self,
        req: CompactContextRequest,
    ) -> Result<CompactContextResult, AgentError> {
        Ok(CompactContextResult {
            compacted_messages: req.messages,
            tokens_saved: 0,
            summary_generated: None,
            grade: None,
        })
    }

    async fn summarize_context(
        &self,
        _req: ContextSummaryRequest,
    ) -> Result<ContextSummaryResult, AgentError> {
        Err(AgentError::internal("summary failed"))
    }
}

async fn seed_long_evergreen_chat(tmp: &tempfile::TempDir, chat_id: &str) {
    let workspace = tmp.path().join("workspaces").join("default");
    let store = WorkspaceSessionStore::new(&workspace);
    let mut header = ChatSessionHeader::new(ChatMetadata {
        chat_id: chat_id.to_string(),
        agent_id: "default".to_string(),
        title: "Preflight Chat".to_string(),
        created_at: "2026-06-19T00:00:00Z".to_string(),
        last_activity_at: "2026-06-19T00:00:00Z".to_string(),
        pinned: false,
        archived: false,
        preview: None,
    });
    let hooks = rua_server::services::product_hooks::ProductHookService::new();
    let configuration = hooks
        .resolve_complete_chat_configuration(
            &rua_core::ChatAddress::agent("default", chat_id),
            rua_server::services::product_hooks::ChatConfigurationMaterialization {
                selected_agent_id: "default".into(),
                running_config: Default::default(),
                security_config: Default::default(),
                agent_assignment: None,
                prompt_file_content: None,
                tool_config_snapshot: Default::default(),
                capability_configuration: None,
                model: Some(json!({"provider_id":"frozen-provider", "model":"frozen-model"})),
                thinking: None,
                collaboration_mode: json!("default"),
                harness_configuration: None,
                authority_device_id: "test-device".into(),
            },
        )
        .await
        .unwrap();
    header.extra.insert(
        "hook_configuration".into(),
        serde_json::to_value(configuration).unwrap(),
    );
    store.save_chat_header(&header).await.unwrap();

    for turn in 1..=6 {
        let content = if turn < 6 {
            format!(
                "turn {turn} {}",
                "existing chat context needs a checkpoint ".repeat(2)
            )
        } else {
            "short live tail".to_string()
        };
        store
            .append_conversation_turn(
                chat_id,
                NewConversationTurn {
                    turn_id: format!("turn-{turn}"),
                    lifecycle: TurnLifecycle {
                        user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                        assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                        assistant_completed_at: Some("2026-06-19T00:00:02Z".to_string()),
                        status: "completed".to_string(),
                    },
                    selected_variant_id: None,
                    messages: vec![
                        conversation_text(&format!("user-{turn}"), "user", &content),
                        conversation_text(&format!("assistant-{turn}"), "assistant", &content),
                    ],
                },
            )
            .await
            .unwrap();
    }
}

fn conversation_text(message_id: &str, role: &str, text: &str) -> NewConversationMessage {
    NewConversationMessage {
        message_id: message_id.to_string(),
        role: role.to_string(),
        created_at: "2026-06-19T00:00:00Z".to_string(),
        status: "completed".to_string(),
        content: json!([{"type": "text", "text": text}]),
        provider: None,
        tool: None,
        metadata: serde_json::Map::new(),
    }
}
