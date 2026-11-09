use axum::body::Body;
use axum::extract::Request;
use axum::http::Method;
use axum::response::Response;
use axum::Router;
use http_body_util::BodyExt;
use rua_server::AppState;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::{models, provider_oauth};
use rua_server::middleware::agent_context_middleware;
use rua_server::ServerProfile;

pub struct TestApp {
    pub app: Router,
    pub tmp: TempDir,
    pub state: AppState,
}

pub fn build_app() -> TestApp {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let app = Router::new()
        .nest(
            "/api",
            Router::new()
                .merge(models::router())
                .merge(provider_oauth::router()),
        )
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state.clone());

    TestApp { app, tmp, state }
}

fn seed_default_agent(tmp: &TempDir) {
    let dir = tmp.path().join("workspaces").join("default");
    std::fs::create_dir_all(&dir).expect("default workspace");
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
        .expect("agent json"),
    )
    .expect("write default agent");
}

pub fn seed_provider(tmp: &TempDir, body: Value) {
    let dir = tmp
        .path()
        .join("secret")
        .join("providers")
        .join("providers");
    std::fs::create_dir_all(&dir).expect("providers dir");
    let id = body["id"].as_str().expect("provider id");
    std::fs::write(
        dir.join(format!("{id}.json")),
        serde_json::to_string_pretty(&body).expect("provider json"),
    )
    .expect("write provider json");
}

pub fn provider_seed(id: &str, name: &str) -> Value {
    json!({
        "id": id,
        "name": name,
        "base_url": "https://api.example.com/v1",
        "api_key": format!("sk-{id}-supersecret123456"),
        "api_key_prefix": "sk-",
        "chat_model": "OpenAIChatModel",
        "models": [
            {"id": "model-primary", "name": "Model Primary"}
        ],
        "extra_models": [],
        "is_local": false,
        "is_custom": true,
        "freeze_url": false,
        "require_api_key": true,
        "support_model_discovery": false,
        "support_connection_check": true,
    })
}

pub async fn request(app: &Router, method: Method, uri: &str, body: Option<Value>) -> Response {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("X-Agent-Id", "default");
    let body = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&value).expect("request json"))
        }
        None => Body::empty(),
    };

    app.clone()
        .oneshot(builder.body(body).expect("request"))
        .await
        .expect("oneshot")
}

pub async fn read_json(resp: Response) -> Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    if bytes.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(&bytes).expect("response json")
}
