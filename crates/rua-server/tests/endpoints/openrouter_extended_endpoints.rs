use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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

fn seed_openrouter_provider(tmp: &TempDir, base_url: &str) {
    let dir = tmp
        .path()
        .join("secret")
        .join("providers")
        .join("providers");
    std::fs::create_dir_all(&dir).expect("create providers dir");
    std::fs::write(
        dir.join("openrouter.json"),
        serde_json::to_string_pretty(&json!({
            "id": "openrouter",
            "name": "OpenRouter",
            "base_url": base_url,
            "api_key": "sk-or-test-key",
            "chat_model": "OpenRouterChatModel",
            "models": [],
            "is_local": false,
            "is_custom": false,
            "freeze_url": false,
            "require_api_key": true,
            "support_model_discovery": true,
            "support_connection_check": true,
        }))
        .unwrap(),
    )
    .expect("write openrouter.json");
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

fn req(method_str: &str, uri: &str, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method_str)
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

fn mock_openrouter_models_payload() -> Value {
    json!({
        "data": [
            {
                "id": "openai/gpt-4o",
                "name": "GPT-4o",
                "context_length": 128000,
                "architecture": {
                    "input_modalities": ["text", "image"],
                    "output_modalities": ["text"]
                },
                "pricing": {
                    "prompt": "0.0025",
                    "completion": "0.01"
                }
            },
            {
                "id": "anthropic/claude-3.5-sonnet",
                "name": "Claude 3.5 Sonnet",
                "context_length": 200000,
                "architecture": {
                    "input_modalities": ["text", "image"],
                    "output_modalities": ["text"]
                },
                "pricing": {
                    "prompt": "0.003",
                    "completion": "0.015"
                }
            },
            {
                "id": "google/gemini-2.5-flash",
                "name": "Gemini 2.5 Flash",
                "context_length": 1048576,
                "architecture": {
                    "input_modalities": ["text", "image", "audio"],
                    "output_modalities": ["text"]
                },
                "pricing": {
                    "prompt": "0.00015",
                    "completion": "0.0006"
                }
            }
        ]
    })
}

async fn setup_mock_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(mock_openrouter_models_payload()))
        .mount(&server)
        .await;
    server
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn openrouter_series_returns_providers() {
    let mock = setup_mock_server().await;
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    seed_openrouter_provider(&tmp, &mock.uri());

    let app = build_app(&tmp);
    let resp = app
        .oneshot(req("GET", "/api/models/openrouter/series", None))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);

    let body = read_json(resp).await;
    let series = body["series"].as_array().expect("series array");
    assert!(!series.is_empty(), "expected non-empty series");

    let names: Vec<&str> = series.iter().filter_map(|s| s["name"].as_str()).collect();
    assert!(
        names.contains(&"openai"),
        "expected openai in series, got {names:?}"
    );
    assert!(names.contains(&"anthropic"), "expected anthropic in series");
    assert!(names.contains(&"google"), "expected google in series");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn openrouter_discover_extended_returns_models() {
    let mock = setup_mock_server().await;
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    seed_openrouter_provider(&tmp, &mock.uri());

    let app = build_app(&tmp);
    let resp = app
        .oneshot(req(
            "POST",
            "/api/models/openrouter/discover-extended",
            Some(json!({})),
        ))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);

    let body = read_json(resp).await;
    let models = body["models"].as_array().expect("models array");
    assert_eq!(models.len(), 3, "expected 3 models, got {}", models.len());

    let ids: Vec<&str> = models.iter().filter_map(|m| m["id"].as_str()).collect();
    assert!(ids.contains(&"openai/gpt-4o"));
    assert!(ids.contains(&"anthropic/claude-3.5-sonnet"));

    let gpt4o = models.iter().find(|m| m["id"] == "openai/gpt-4o").unwrap();
    assert_eq!(gpt4o["provider"], "openai");
    assert_eq!(gpt4o["context_window"], 128000);
    assert!(gpt4o["pricing"]["prompt"].as_str().is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn openrouter_filter_by_provider() {
    let mock = setup_mock_server().await;
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    seed_openrouter_provider(&tmp, &mock.uri());

    let app = build_app(&tmp);
    let resp = app
        .oneshot(req(
            "POST",
            "/api/models/openrouter/models/filter",
            Some(json!({ "providers": ["google"] })),
        ))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);

    let body = read_json(resp).await;
    let models = body["models"].as_array().expect("models array");
    assert_eq!(models.len(), 1);
    assert_eq!(models[0]["id"], "google/gemini-2.5-flash");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn openrouter_filter_by_max_price() {
    let mock = setup_mock_server().await;
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    seed_openrouter_provider(&tmp, &mock.uri());

    let app = build_app(&tmp);
    let resp = app
        .oneshot(req(
            "POST",
            "/api/models/openrouter/models/filter",
            Some(json!({ "max_prompt_price": 0.001 })),
        ))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);

    let body = read_json(resp).await;
    let models = body["models"].as_array().expect("models array");
    assert_eq!(models.len(), 1);
    assert_eq!(models[0]["id"], "google/gemini-2.5-flash");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn openrouter_series_returns_error_when_no_provider() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);

    let app = build_app(&tmp);
    let resp = app
        .oneshot(req("GET", "/api/models/openrouter/series", None))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
