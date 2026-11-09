use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use agent_types::TokenUsage;
use chrono::{DateTime, Utc};
use rua_contract::UsageRecordMetadata;
use rua_server::endpoints::token_usage::router as token_usage_router;
use rua_server::AppState;
use rua_usage::UsageLedger;

fn build_app() -> (Router, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed workspace dir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    let app = Router::new()
        .nest("/api", token_usage_router())
        .with_state(state);
    (app, tmp)
}

fn seed_usage_file(tmp: &TempDir, data: &Value) {
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
                for idx in 0..calls {
                    records.push(serde_json::json!({
                        "usage_id": format!("{date}:{key}:{idx}"),
                        "provider_id": provider,
                        "model_name": model,
                        "prompt_tokens": if idx == 0 { prompt } else { 0 },
                        "completion_tokens": if idx == 0 { completion } else { 0 },
                        "total_tokens": if idx == 0 { prompt + completion } else { 0 },
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
                    source: Some("test".into()),
                    ..Default::default()
                },
            )
            .expect("record usage");
    }
}

async fn get_json(app: Router, uri: &str) -> (StatusCode, Value) {
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request");
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn by_date_returns_daily_usage_for_date_range() {
    let (app, tmp) = build_app();
    let data = serde_json::json!({
        "2025-05-10": {
            "openai:gpt-4": {
                "provider_id": "openai",
                "model_name": "gpt-4",
                "prompt_tokens": 100,
                "completion_tokens": 50,
                "call_count": 2
            }
        },
        "2025-05-11": {
            "openai:gpt-4": {
                "provider_id": "openai",
                "model_name": "gpt-4",
                "prompt_tokens": 200,
                "completion_tokens": 80,
                "call_count": 3
            }
        },
        "2025-05-12": {
            "openai:gpt-4": {
                "provider_id": "openai",
                "model_name": "gpt-4",
                "prompt_tokens": 300,
                "completion_tokens": 120,
                "call_count": 5
            }
        }
    });
    seed_usage_file(&tmp, &data);

    let (status, json) = get_json(
        app,
        "/api/token-usage/by-date?from=2025-05-10&to=2025-05-12",
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let arr = json.as_array().expect("response is array");
    assert_eq!(arr.len(), 3, "expected 3 daily entries");

    assert_eq!(arr[0]["date"], "2025-05-10");
    assert_eq!(arr[0]["prompt_tokens"], 100);
    assert_eq!(arr[0]["completion_tokens"], 50);

    assert_eq!(arr[1]["date"], "2025-05-11");
    assert_eq!(arr[1]["prompt_tokens"], 200);

    assert_eq!(arr[2]["date"], "2025-05-12");
    assert_eq!(arr[2]["prompt_tokens"], 300);
    assert_eq!(arr[2]["completion_tokens"], 120);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn by_date_defaults_to_last_30_days() {
    let (app, tmp) = build_app();
    let today = chrono::Utc::now().date_naive();
    let yesterday = today - chrono::Duration::days(1);
    let old = today - chrono::Duration::days(40);

    let data = serde_json::json!({
        yesterday.format("%Y-%m-%d").to_string(): {
            "openai:gpt-4": {
                "provider_id": "openai",
                "model_name": "gpt-4",
                "prompt_tokens": 100,
                "completion_tokens": 50,
                "call_count": 1
            }
        },
        old.format("%Y-%m-%d").to_string(): {
            "openai:gpt-4": {
                "provider_id": "openai",
                "model_name": "gpt-4",
                "prompt_tokens": 999,
                "completion_tokens": 999,
                "call_count": 1
            }
        }
    });
    seed_usage_file(&tmp, &data);

    let (status, json) = get_json(app, "/api/token-usage/by-date").await;

    assert_eq!(status, StatusCode::OK);
    let arr = json.as_array().expect("response is array");
    assert_eq!(arr.len(), 1, "only yesterday should be within 30 days");
    assert_eq!(arr[0]["prompt_tokens"], 100);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn by_date_empty_db_returns_empty_array() {
    let (app, _tmp) = build_app();

    let (status, json) = get_json(
        app,
        "/api/token-usage/by-date?from=2025-01-01&to=2025-01-31",
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let arr = json.as_array().expect("response is array");
    assert!(arr.is_empty(), "empty DB should return empty array");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn by_date_from_after_to_returns_400() {
    let (app, _tmp) = build_app();

    let (status, _json) = get_json(
        app,
        "/api/token-usage/by-date?from=2025-05-15&to=2025-05-10",
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
}
