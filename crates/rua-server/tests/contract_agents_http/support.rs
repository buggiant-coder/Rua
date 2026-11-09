use agent_types::TokenUsage;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::response::Response;
use axum::Router;
use chrono::{DateTime, Utc};
use http_body_util::BodyExt;
use rua_contract::UsageRecordMetadata;
use rua_usage::UsageLedger;
use serde_json::json;
use serde_json::Value;
use std::path::PathBuf;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::{agents, gap_fixes};
use rua_server::middleware::agent_context_middleware;
use rua_server::{AppState, ServerProfile};

pub struct TestApp {
    pub app: Router,
    pub state: AppState,
    pub data_dir: PathBuf,
    _tmp: TempDir,
}

pub fn build_app() -> TestApp {
    let tmp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let app = Router::new()
        .nest("/api", agents::router())
        .nest("/api", gap_fixes::agent_stats_router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state.clone());

    TestApp {
        app,
        state,
        data_dir: tmp.path().to_path_buf(),
        _tmp: tmp,
    }
}

pub async fn request(app: &Router, method: Method, uri: &str, body: Option<Value>) -> Response {
    request_with_agent(app, method, uri, body, "default").await
}

pub async fn request_with_agent(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
    agent_id: &str,
) -> Response {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("X-Agent-Id", agent_id);
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

pub async fn create_agent(app: &Router, id: &str, name: &str) -> Value {
    let resp = request(
        app,
        Method::POST,
        "/api/agents",
        Some(json!({
            "id": id,
            "name": name,
            "description": ""
        })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    read_json(resp).await
}

pub async fn delete_agent(app: &Router, id: &str) {
    let resp = request(app, Method::DELETE, &format!("/api/agents/{id}"), None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(read_json(resp).await["success"], true);
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

pub fn seed_usage_file(app: &TestApp, data: Value) {
    let mut records = Vec::new();
    if let Some(days) = data.as_object() {
        for (date, by_model) in days {
            let Some(models) = by_model.as_object() else {
                continue;
            };
            for (key, entry) in models {
                let Some(entry) = entry.as_object() else {
                    continue;
                };
                let (fallback_provider, fallback_model) =
                    key.split_once(':').unwrap_or(("", key.as_str()));
                let provider = entry
                    .get("provider_id")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.is_empty())
                    .unwrap_or(fallback_provider);
                let model = entry
                    .get("model_name")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.is_empty())
                    .unwrap_or(fallback_model);
                let prompt = entry
                    .get("prompt_tokens")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0);
                let completion = entry
                    .get("completion_tokens")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0);
                let calls = entry
                    .get("call_count")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(if prompt > 0 || completion > 0 { 1 } else { 0 });
                let agent_id = entry.get("agent_id").and_then(|value| value.as_str());
                let chat_id = entry.get("chat_id").and_then(|value| value.as_str());
                for idx in 0..calls {
                    records.push(json!({
                        "usage_id": format!("{date}:{key}:{idx}"),
                        "provider_id": provider,
                        "model_name": model,
                        "prompt_tokens": if idx == 0 { prompt } else { 0 },
                        "completion_tokens": if idx == 0 { completion } else { 0 },
                        "total_tokens": if idx == 0 { prompt + completion } else { 0 },
                        "agent_id": agent_id,
                        "chat_id": chat_id,
                        "session_id": chat_id,
                        "started_at": format!("{date}T00:00:00Z"),
                        "finished_at": format!("{date}T00:00:00Z"),
                        "source": "test"
                    }));
                }
            }
        }
    }
    let ledger = UsageLedger::open(&app.data_dir).expect("usage ledger");
    for record in records {
        let at = DateTime::parse_from_rfc3339(record["finished_at"].as_str().expect("finished_at"))
            .expect("valid usage timestamp")
            .with_timezone(&Utc);
        ledger
            .record(
                record["provider_id"].as_str().unwrap_or_default(),
                record["model_name"].as_str().unwrap_or_default(),
                &TokenUsage::new(
                    record["prompt_tokens"].as_u64().unwrap_or_default(),
                    record["completion_tokens"].as_u64().unwrap_or_default(),
                ),
                at,
                at,
                &UsageRecordMetadata {
                    usage_id: record["usage_id"].as_str().map(str::to_owned),
                    agent_id: record["agent_id"].as_str().map(str::to_owned),
                    chat_id: record["chat_id"].as_str().map(str::to_owned),
                    session_id: record["session_id"].as_str().map(str::to_owned),
                    source: Some("test".into()),
                    ..Default::default()
                },
            )
            .expect("record usage");
    }
}
