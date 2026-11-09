use axum::body::Body;
use axum::extract::Request;
use axum::http::Method;
use axum::response::Response;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use agent_types::TokenUsage;
use chrono::{DateTime, Utc};
use rua_contract::UsageRecordMetadata;
use rua_server::endpoints::token_usage::router as token_usage_router;
use rua_server::{AppState, ServerProfile};
use rua_usage::UsageLedger;

pub struct TestApp {
    pub app: Router,
    pub tmp: TempDir,
}

pub fn build_app() -> TestApp {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("default workspace");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let app = Router::new()
        .nest("/api", token_usage_router())
        .with_state(state);
    TestApp { app, tmp }
}

pub fn seed_usage_file(tmp: &TempDir, data: &Value) {
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
                let session_id = entry.get("session_id").and_then(|value| value.as_str());
                for idx in 0..calls {
                    records.push(serde_json::json!({
                        "usage_id": format!("{date}:{key}:{idx}"),
                        "provider_id": provider,
                        "model_name": model,
                        "prompt_tokens": if idx == 0 { prompt } else { 0 },
                        "completion_tokens": if idx == 0 { completion } else { 0 },
                        "total_tokens": if idx == 0 { prompt + completion } else { 0 },
                        "agent_id": agent_id,
                        "chat_id": chat_id,
                        "session_id": session_id,
                        "started_at": format!("{date}T00:00:00Z"),
                        "finished_at": format!("{date}T00:00:00Z"),
                        "source": "test"
                    }));
                }
            }
        }
    }
    let ledger = UsageLedger::open(tmp.path()).expect("usage ledger");
    for record in records {
        let Ok(at) =
            DateTime::parse_from_rfc3339(record["finished_at"].as_str().expect("finished_at"))
        else {
            continue;
        };
        let at = at.with_timezone(&Utc);
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

pub fn seed_raw_usage_file(tmp: &TempDir, data: &str) {
    std::fs::write(tmp.path().join("token_usage_ledger.json"), data)
        .expect("write token_usage_ledger.json");
}

pub async fn request(app: &Router, method: Method, uri: &str) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
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
