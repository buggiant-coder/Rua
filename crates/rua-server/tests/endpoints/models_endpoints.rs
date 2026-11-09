use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::models;
use rua_server::middleware::agent_context_middleware;
use rua_server::AppState;

fn seed_default_agent(tmp: &TempDir) {
    let dir = tmp.path().join("workspaces").join("default");
    std::fs::create_dir_all(&dir).expect("seed default workspace");
    std::fs::write(
        dir.join("agent.json"),
        serde_json::to_string_pretty(&json!({
            "id": "default",
            "name": "default",
            "description": "",
            "workspace_dir": "",
            "language": "",
            "system_prompt_files": [],
        }))
        .unwrap(),
    )
    .expect("write agent.json");
}

fn seed_provider(tmp: &TempDir, body: Value) {
    let dir = tmp
        .path()
        .join("secret")
        .join("providers")
        .join("providers");
    std::fs::create_dir_all(&dir).expect("create providers dir");
    let id = body["id"].as_str().expect("provider id");
    std::fs::write(
        dir.join(format!("{id}.json")),
        serde_json::to_string_pretty(&body).unwrap(),
    )
    .expect("write provider json");
}

fn seed_default_model(tmp: &TempDir, body: Value) {
    let dir = tmp.path().join("secret").join("providers");
    std::fs::create_dir_all(&dir).expect("create providers dir");
    std::fs::write(
        dir.join("default_model.json"),
        serde_json::to_string_pretty(&body).unwrap(),
    )
    .expect("write default_model.json");
}

fn build_app(tmp: &TempDir) -> Router {
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    Router::new()
        .nest("/api", models::router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state)
}

async fn read_json(resp: axum::response::Response) -> Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    if bytes.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

fn req(method: &str, uri: &str, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("X-Agent-Id", "default");
    let body = match body {
        Some(json) => {
            builder = builder.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&json).unwrap())
        }
        None => Body::empty(),
    };
    builder.body(body).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_models_returns_zero_config_free_providers_when_no_providers_on_disk() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    let app = build_app(&tmp);

    let resp = app
        .oneshot(req("GET", "/api/models", None))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);

    let body = read_json(resp).await;
    let arr = body.as_array().expect("array");
    let mut ids: Vec<&str> = arr.iter().filter_map(|p| p["id"].as_str()).collect();
    ids.sort_unstable();
    assert_eq!(ids, vec!["kilo", "opencode"], "got providers {arr:?}");
    assert!(arr
        .iter()
        .all(|p| p["require_api_key"] == false && p["is_free_tier"] == true));
    let opencode = arr
        .iter()
        .find(|provider| provider["id"] == "opencode")
        .expect("opencode provider");
    let models = opencode["models"].as_array().expect("opencode models");
    assert!(
        !models.is_empty(),
        "zero-config provider must expose usable models"
    );
    assert!(
        models
            .iter()
            .all(|model| model["id"] != "deepseek-v4-flash-free"),
        "catalog marks this model unavailable; do not advertise it"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_models_returns_seeded_provider_with_redacted_api_key() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    seed_provider(
        &tmp,
        json!({
            "id": "openai",
            "name": "OpenAI",
            "base_url": "https://api.openai.com/v1",
            "api_key": "sk-supersecret1234567890",
            "chat_model": "OpenAIChatModel",
            "models": [
                {"id": "gpt-4o", "name": "GPT-4o"}
            ],
            "extra_models": [],
            "is_local": false,
            "is_custom": false,
            "freeze_url": false,
            "require_api_key": true,
            "support_model_discovery": true,
            "support_connection_check": true,
        }),
    );

    let app = build_app(&tmp);
    let resp = app
        .oneshot(req("GET", "/api/models", None))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);

    let body = read_json(resp).await;
    let arr = body.as_array().expect("array");
    assert_eq!(
        arr.len(),
        3,
        "expected seeded provider plus free defaults, got {arr:?}"
    );
    let p = arr
        .iter()
        .find(|provider| provider["id"] == "openai")
        .unwrap_or_else(|| panic!("expected seeded openai provider, got {arr:?}"));
    assert_eq!(p["id"], "openai");
    assert_eq!(p["name"], "OpenAI");
    assert_eq!(p["base_url"], "https://api.openai.com/v1");
    let key = p["api_key"].as_str().expect("api_key string");
    assert_ne!(
        key, "sk-supersecret1234567890",
        "api_key must be redacted in response"
    );
    assert!(
        key.contains("...") || key.contains("***"),
        "api_key must be a redacted placeholder, got {key:?}"
    );
    let models = p["models"].as_array().expect("models array");
    assert_eq!(models.len(), 1);
    assert_eq!(models[0]["id"], "gpt-4o");
    assert!(arr.iter().any(|provider| provider["id"] == "opencode"));
    assert!(arr.iter().any(|provider| provider["id"] == "kilo"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_provider_returns_404_when_missing() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    let app = build_app(&tmp);

    let resp = app
        .oneshot(req("GET", "/api/models/providers/nope", None))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_provider_returns_seeded_detail() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    seed_provider(
        &tmp,
        json!({
            "id": "ollama",
            "name": "Ollama",
            "base_url": "http://localhost:11434",
            "api_key": "",
            "chat_model": "OllamaChatModel",
            "models": [{"id": "llama3", "name": "Llama 3"}],
            "is_local": true,
            "is_custom": false,
            "freeze_url": false,
            "require_api_key": false,
            "support_model_discovery": false,
            "support_connection_check": true,
        }),
    );

    let app = build_app(&tmp);
    let resp = app
        .oneshot(req("GET", "/api/models/providers/ollama", None))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);

    let body = read_json(resp).await;
    assert_eq!(body["id"], "ollama");
    assert_eq!(body["is_local"], true);
    assert_eq!(body["chat_model"], "OllamaChatModel");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_default_model_returns_empty_when_no_active() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    let app = build_app(&tmp);

    let resp = app
        .oneshot(req("GET", "/api/models/providers/active", None))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);

    let body = read_json(resp).await;
    assert_eq!(body["provider_id"], "");
    assert_eq!(body["model"], "");
    assert!(body["default_llm"].is_object());
    assert_eq!(body["default_llm"]["provider_id"], "");
    assert_eq!(body["default_llm"]["model"], "");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_templates_includes_builtin_openai() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    let app = build_app(&tmp);

    let resp = app
        .oneshot(req("GET", "/api/models/templates", None))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);

    let body = read_json(resp).await;
    let arr = body.as_array().expect("array");
    let ids: Vec<&str> = arr.iter().filter_map(|t| t["id"].as_str()).collect();
    assert!(
        ids.contains(&"openai"),
        "expected openai in templates, got {ids:?}"
    );
    assert!(
        ids.contains(&"anthropic"),
        "expected anthropic in templates, got {ids:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_model_removes_model_from_provider() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    seed_provider(
        &tmp,
        json!({
            "id": "openai",
            "name": "OpenAI",
            "base_url": "https://api.openai.com/v1",
            "api_key": "sk-test123",
            "chat_model": "OpenAIChatModel",
            "models": [
                {"id": "gpt-4o", "name": "GPT-4o"},
                {"id": "gpt-4o-mini", "name": "GPT-4o Mini"}
            ]
        }),
    );
    let app = build_app(&tmp);

    let resp = app
        .oneshot(req("DELETE", "/api/models/openai/models/gpt-4o-mini", None))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);

    let body = read_json(resp).await;
    let models = body["models"].as_array().expect("models array");
    let model_ids: Vec<&str> = models.iter().filter_map(|m| m["id"].as_str()).collect();
    assert!(
        !model_ids.contains(&"gpt-4o-mini"),
        "deleted model should not be in response"
    );
    assert!(model_ids.contains(&"gpt-4o"), "other model should remain");

    let config_path = tmp.path().join("secret/providers/providers/openai.json");
    let on_disk: Value =
        serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    let disk_ids: Vec<&str> = on_disk["models"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["id"].as_str())
        .collect();
    assert!(
        !disk_ids.contains(&"gpt-4o-mini"),
        "deleted model must not persist on disk"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_model_returns_404_for_unknown_model() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    seed_provider(
        &tmp,
        json!({
            "id": "openai",
            "name": "OpenAI",
            "base_url": "https://api.openai.com/v1",
            "api_key": "sk-test123",
            "chat_model": "OpenAIChatModel",
            "models": [{"id": "gpt-4o", "name": "GPT-4o"}]
        }),
    );
    let app = build_app(&tmp);

    let resp = app
        .oneshot(req("DELETE", "/api/models/openai/models/nonexistent", None))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clear_default_model_sets_none_and_deletes_file() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    seed_provider(
        &tmp,
        json!({
            "id": "openai",
            "name": "OpenAI",
            "base_url": "https://api.openai.com/v1",
            "api_key": "sk-test",
            "chat_model": "OpenAIChatModel",
            "models": [{"id": "gpt-4o", "name": "GPT-4o"}]
        }),
    );
    seed_default_model(
        &tmp,
        json!({
            "provider_id": "openai",
            "model": "gpt-4o"
        }),
    );

    let app = build_app(&tmp);

    let resp = app
        .clone()
        .oneshot(req("GET", "/api/models/providers/active", None))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(req("DELETE", "/api/models/providers/active", None))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["success"], true);

    let resp = app
        .oneshot(req("GET", "/api/models/providers/active", None))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["provider_id"], "");
    assert_eq!(body["model"], "");

    let active_path = tmp.path().join("secret/providers/default_model.json");
    assert!(
        !active_path.exists(),
        "default_model.json should be deleted from disk"
    );
}
