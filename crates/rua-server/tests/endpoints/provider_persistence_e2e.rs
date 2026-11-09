use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::Json;
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

fn provider_seed(id: &str, models: Vec<&str>) -> Value {
    json!({
        "id": id,
        "name": format!("{id} provider"),
        "base_url": "https://api.example.com/v1",
        "api_key": format!("sk-{id}-seed"),
        "chat_model": "OpenAIChatModel",
        "models": models.iter().map(|m| json!({"id": m, "name": m})).collect::<Vec<_>>(),
        "extra_models": [],
        "is_local": false,
        "is_custom": false,
        "freeze_url": false,
        "require_api_key": true,
        "support_model_discovery": true,
        "support_connection_check": true,
    })
}

fn list_model_ids_for_provider(list_body: &Value, provider_id: &str) -> Vec<String> {
    let arr = list_body.as_array().expect("list array");
    let provider = arr
        .iter()
        .find(|p| p["id"] == provider_id)
        .expect("provider in list");
    provider["models"]
        .as_array()
        .expect("models array")
        .iter()
        .filter_map(|m| m["id"].as_str().map(String::from))
        .collect()
}

fn list_provider_ids(list_body: &Value) -> Vec<String> {
    list_body
        .as_array()
        .expect("list array")
        .iter()
        .filter_map(|p| p["id"].as_str().map(String::from))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn add_model_appears_in_list_immediately() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    seed_provider(&tmp, provider_seed("dashscope", vec!["qwen-max"]));
    let app = build_app(&tmp);

    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/models/dashscope/models",
            Some(json!({"id": "qwen-plus", "name": "Qwen Plus"})),
        ))
        .await
        .expect("send POST");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .oneshot(req("GET", "/api/models", None))
        .await
        .expect("send GET");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let ids = list_model_ids_for_provider(&body, "dashscope");
    assert!(
        ids.contains(&"qwen-plus".to_string()),
        "expected qwen-plus in list after POST, got {ids:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_model_disappears_from_list_immediately() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    seed_provider(&tmp, provider_seed("openai", vec!["gpt-4o", "gpt-4o-mini"]));
    let app = build_app(&tmp);

    let resp = app
        .clone()
        .oneshot(req("DELETE", "/api/models/openai/models/gpt-4o-mini", None))
        .await
        .expect("send DELETE");
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(req("GET", "/api/models", None))
        .await
        .expect("send GET");
    let body = read_json(resp).await;
    let ids = list_model_ids_for_provider(&body, "openai");
    assert!(
        !ids.contains(&"gpt-4o-mini".to_string()),
        "expected gpt-4o-mini absent after DELETE, got {ids:?}"
    );
    assert!(
        ids.contains(&"gpt-4o".to_string()),
        "untouched model gpt-4o must remain"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_provider_appears_in_list_immediately() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    let app = build_app(&tmp);

    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/models/custom-providers",
            Some(json!({
                "id": "custom-x",
                "name": "Custom X",
                "base_url": "https://api.x.example",
                "api_key": "sk-x",
                "models": [{"id": "x-mini", "name": "X Mini"}],
                "chat_model": "OpenAIChatModel"
            })),
        ))
        .await
        .expect("send POST");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .oneshot(req("GET", "/api/models", None))
        .await
        .expect("send GET");
    let body = read_json(resp).await;
    let ids = list_provider_ids(&body);
    assert!(
        ids.contains(&"custom-x".to_string()),
        "expected custom-x in list after POST, got {ids:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_provider_disappears_from_list_immediately() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    seed_provider(&tmp, provider_seed("openai", vec!["gpt-4o"]));
    let app = build_app(&tmp);

    let resp = app
        .clone()
        .oneshot(req("DELETE", "/api/models/providers/openai", None))
        .await
        .expect("send DELETE");
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(req("GET", "/api/models", None))
        .await
        .expect("send GET");
    let body = read_json(resp).await;
    let ids = list_provider_ids(&body);
    assert!(
        !ids.contains(&"openai".to_string()),
        "expected openai absent after DELETE, got {ids:?}"
    );

    let path = tmp.path().join("secret/providers/providers/openai.json");
    assert!(
        !path.exists(),
        "provider JSON must be deleted from disk, but {} still exists",
        path.display()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_provider_removes_disk_config_even_when_runtime_provider_is_not_registered() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    seed_provider(
        &tmp,
        json!({
            "id": "broken-sync-provider",
            "name": "Broken synced provider",
            "base_url": "",
            "api_key": "",
            "chat_model": "OpenAIChatModel",
            "models": [],
            "extra_models": [],
            "is_custom": true
        }),
    );
    let app = build_app(&tmp);

    let resp = app
        .clone()
        .oneshot(req("GET", "/api/models/providers", None))
        .await
        .expect("send GET");
    let body = read_json(resp).await;
    assert!(
        list_provider_ids(&body).contains(&"broken-sync-provider".to_string()),
        "disk-backed provider should be visible in list before delete, got {body:?}"
    );

    let resp = app
        .clone()
        .oneshot(req(
            "DELETE",
            "/api/models/providers/broken-sync-provider",
            None,
        ))
        .await
        .expect("send DELETE");
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(req("GET", "/api/models/providers", None))
        .await
        .expect("send GET after delete");
    let body = read_json(resp).await;
    assert!(
        !list_provider_ids(&body).contains(&"broken-sync-provider".to_string()),
        "provider must disappear from list after delete, got {body:?}"
    );

    let path = tmp
        .path()
        .join("secret/providers/providers/broken-sync-provider.json");
    assert!(
        !path.exists(),
        "provider JSON must be deleted from disk, but {} still exists",
        path.display()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_provider_config_persists_to_disk() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    seed_provider(&tmp, provider_seed("openai", vec!["gpt-4o"]));
    let app = build_app(&tmp);

    let resp = app
        .oneshot(req(
            "PUT",
            "/api/models/providers/openai",
            Some(json!({
                "name": "OpenAI (renamed)",
                "base_url": "https://api.openai.com/v2",
            })),
        ))
        .await
        .expect("send PUT");
    assert_eq!(resp.status(), StatusCode::OK);

    let path = tmp.path().join("secret/providers/providers/openai.json");
    let on_disk: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("parse");
    assert_eq!(on_disk["name"], "OpenAI (renamed)");
    assert_eq!(on_disk["base_url"], "https://api.openai.com/v2");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dashscope_update_persists_responses_protocol_and_dedicated_base_url() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    let mut provider = provider_seed("dashscope", vec!["qwen3.8-max"]);
    provider["freeze_url"] = json!(true);
    provider["thinking_param_style"] = json!("budget");
    seed_provider(&tmp, provider);
    let app = build_app(&tmp);
    let dedicated_base_url = "https://workspace.cn-beijing.maas.aliyuncs.com/compatible-mode/v1";

    let resp = app
        .oneshot(req(
            "PUT",
            "/api/models/providers/dashscope",
            Some(json!({
                "base_url": dedicated_base_url,
                "chat_model": "DashScopeResponseModel",
            })),
        ))
        .await
        .expect("send PUT");
    assert_eq!(resp.status(), StatusCode::OK);

    let path = tmp.path().join("secret/providers/providers/dashscope.json");
    let on_disk: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("parse");
    assert_eq!(on_disk["base_url"], dedicated_base_url);
    assert_eq!(on_disk["chat_model"], "DashScopeResponseModel");
    assert_eq!(on_disk["thinking_param_style"], "effort");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pause_then_restart_simulation_keeps_config() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    seed_provider(&tmp, provider_seed("dashscope", vec!["qwen-max"]));

    {
        let app = build_app(&tmp);
        let resp = app
            .oneshot(req(
                "POST",
                "/api/models/dashscope/models",
                Some(json!({"id": "qwen-plus", "name": "Qwen Plus"})),
            ))
            .await
            .expect("send POST");
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    let app = build_app(&tmp);
    let resp = app
        .oneshot(req("GET", "/api/models", None))
        .await
        .expect("send GET");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let ids = list_model_ids_for_provider(&body, "dashscope");
    assert!(
        ids.contains(&"qwen-plus".to_string()),
        "expected qwen-plus to survive restart, got {ids:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multimodal_probe_result_persists_to_disk() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    seed_provider(&tmp, provider_seed("openai", vec!["gpt-4o"]));

    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);

    state
        .provider_cells
        .cell("openai")
        .update(|maybe| {
            let mut cfg = maybe.expect("provider exists on disk");
            for m in cfg.models.iter_mut().chain(cfg.extra_models.iter_mut()) {
                if m.id == "gpt-4o" {
                    m.supports_image = Some(true);
                    m.supports_video = Some(false);
                    m.supports_multimodal = Some(true);
                    m.probe_source = Some("probed".into());
                    break;
                }
            }
            cfg
        })
        .await
        .expect("update provider config");

    let path = tmp.path().join("secret/providers/providers/openai.json");
    let on_disk: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("parse");
    let model = on_disk["models"]
        .as_array()
        .expect("models array")
        .iter()
        .find(|m| m["id"] == "gpt-4o")
        .expect("gpt-4o on disk");
    assert_eq!(model["supports_image"], json!(true));
    assert_eq!(model["supports_video"], json!(false));
    assert_eq!(model["supports_multimodal"], json!(true));
    assert_eq!(model["probe_source"], json!("probed"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_multimodal_probe_preserves_capabilities_and_returns_detail() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let cancellation = tokio_util::sync::CancellationToken::new();
    let shutdown = cancellation.clone();
    let upstream = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().fallback(|| async {
                (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "error": {"message": "invalid image payload"}
                    })),
                )
            }),
        )
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await
        .unwrap();
    });
    let tmp = TempDir::new().unwrap();
    seed_default_agent(&tmp);
    let mut config = provider_seed("openai", vec!["gpt-4o"]);
    config["base_url"] = json!(format!("http://{address}/v1"));
    config["models"][0]["supports_image"] = json!(true);
    config["models"][0]["supports_multimodal"] = json!(true);
    config["models"][0]["probe_source"] = json!("configured");
    seed_provider(&tmp, config);
    let path = tmp.path().join("secret/providers/providers/openai.json");
    let app = build_app(&tmp);
    let before = std::fs::read(&path).unwrap();
    let response = app
        .oneshot(req(
            "POST",
            "/api/models/openai/models/probe-multimodal",
            Some(json!({"model_id": "gpt-4o"})),
        ))
        .await
        .unwrap();
    let status = response.status();
    let body = read_json(response).await;
    cancellation.cancel();
    upstream.await.unwrap();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert!(body["detail"]
        .as_str()
        .unwrap()
        .contains("Multimodal probe failed"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
