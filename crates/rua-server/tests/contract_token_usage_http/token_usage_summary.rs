use axum::http::{Method, StatusCode};
use chrono::Duration;
use serde_json::json;

use super::support::{build_app, read_json, request, seed_raw_usage_file, seed_usage_file};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_summary_returns_console_token_usage_shape() {
    let test = build_app();

    let resp = request(
        &test.app,
        Method::GET,
        "/api/token-usage?start_date=2025-05-10&end_date=2025-05-11",
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["total_prompt_tokens"], 0);
    assert_eq!(body["total_completion_tokens"], 0);
    assert_eq!(body["total_calls"], 0);
    assert_eq!(body["total_tokens"], 0);
    assert_eq!(body["count"], 0);
    assert_eq!(body["by_model"], json!({}));
    assert_eq!(body["by_provider"], json!({}));
    assert_eq!(body["by_date"], json!({}));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn corrupt_usage_file_returns_empty_summary_contract() {
    let test = build_app();
    seed_raw_usage_file(&test.tmp, "{invalid json");

    let resp = request(
        &test.app,
        Method::GET,
        "/api/token-usage?start_date=2025-05-10&end_date=2025-05-11",
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["total_prompt_tokens"], 0);
    assert_eq!(body["total_completion_tokens"], 0);
    assert_eq!(body["total_calls"], 0);
    assert_eq!(body["total_tokens"], 0);
    assert_eq!(body["count"], 0);
    assert_eq!(body["by_model"], json!({}));
    assert_eq!(body["by_provider"], json!({}));
    assert_eq!(body["by_date"], json!({}));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn summary_aggregates_by_model_provider_and_date_with_filters() {
    let test = build_app();
    seed_usage_file(
        &test.tmp,
        &json!({
            "2025-05-10": {
                "openai:gpt-4": {
                    "provider_id": "openai",
                    "model_name": "gpt-4",
                    "prompt_tokens": 100,
                    "completion_tokens": 50,
                    "call_count": 2
                },
                "dashscope:qwen-plus": {
                    "provider_id": "dashscope",
                    "model_name": "qwen-plus",
                    "prompt_tokens": 20,
                    "completion_tokens": 10,
                    "call_count": 1
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
                "openai:gpt-4o": {
                    "provider_id": "openai",
                    "model_name": "gpt-4o",
                    "prompt_tokens": 999,
                    "completion_tokens": 999,
                    "call_count": 9
                }
            }
        }),
    );

    let resp = request(
        &test.app,
        Method::GET,
        "/api/token-usage?start_date=2025-05-10&end_date=2025-05-11&provider_id=openai&model_name=gpt-4",
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["total_prompt_tokens"], 300);
    assert_eq!(body["total_completion_tokens"], 130);
    assert_eq!(body["total_calls"], 5);
    assert_eq!(body["total_tokens"], 430);
    assert_eq!(body["count"], 5);
    assert_eq!(body["by_model"]["openai:gpt-4"]["provider_id"], "openai");
    assert_eq!(body["by_model"]["openai:gpt-4"]["model"], "gpt-4");
    assert_eq!(body["by_model"]["openai:gpt-4"]["prompt_tokens"], 300);
    assert!(body["by_model"].get("openai:gpt-4o").is_none());
    assert!(body["by_model"].get("dashscope:qwen-plus").is_none());
    assert_eq!(body["by_provider"]["openai"]["completion_tokens"], 130);
    assert_eq!(body["by_date"]["2025-05-10"]["call_count"], 2);
    assert_eq!(body["by_date"]["2025-05-11"]["call_count"], 3);
    assert!(body["by_date"].get("2025-05-12").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn summary_accepts_qwenpaw_model_and_provider_query_aliases() {
    let test = build_app();
    seed_usage_file(
        &test.tmp,
        &json!({
            "2025-05-10": {
                "openai:gpt-4": {
                    "provider_id": "openai",
                    "model_name": "gpt-4",
                    "prompt_tokens": 100,
                    "completion_tokens": 50,
                    "call_count": 2
                },
                "dashscope:qwen-plus": {
                    "provider_id": "dashscope",
                    "model_name": "qwen-plus",
                    "prompt_tokens": 999,
                    "completion_tokens": 999,
                    "call_count": 9
                }
            }
        }),
    );

    let resp = request(
        &test.app,
        Method::GET,
        "/api/token-usage?start_date=2025-05-10&end_date=2025-05-10&provider=openai&model=gpt-4",
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["total_prompt_tokens"], 100);
    assert_eq!(body["total_completion_tokens"], 50);
    assert_eq!(body["total_calls"], 2);
    assert!(body["by_model"].get("openai:gpt-4").is_some());
    assert!(body["by_model"].get("dashscope:qwen-plus").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn summary_days_filter_matches_dashboard_quick_range_semantics() {
    let test = build_app();
    let today = chrono::Utc::now().date_naive();
    let today_key = today.format("%Y-%m-%d").to_string();
    let yesterday_key = (today - Duration::days(1)).format("%Y-%m-%d").to_string();
    let old_key = (today - Duration::days(45)).format("%Y-%m-%d").to_string();
    seed_usage_file(
        &test.tmp,
        &json!({
            today_key.clone(): {
                "openai:gpt-4": {
                    "provider_id": "openai",
                    "model_name": "gpt-4",
                    "prompt_tokens": 100,
                    "completion_tokens": 50,
                    "call_count": 2
                }
            },
            yesterday_key.clone(): {
                "dashscope:qwen-plus": {
                    "provider_id": "dashscope",
                    "model_name": "qwen-plus",
                    "prompt_tokens": 20,
                    "completion_tokens": 10,
                    "call_count": 1
                }
            },
            old_key.clone(): {
                "openai:gpt-4o": {
                    "provider_id": "openai",
                    "model_name": "gpt-4o",
                    "prompt_tokens": 999,
                    "completion_tokens": 999,
                    "call_count": 9
                }
            }
        }),
    );

    let thirty_day = request(&test.app, Method::GET, "/api/token-usage?days=30").await;

    assert_eq!(thirty_day.status(), StatusCode::OK);
    let body = read_json(thirty_day).await;
    assert_eq!(body["total_prompt_tokens"], 120);
    assert_eq!(body["total_completion_tokens"], 60);
    assert_eq!(body["total_calls"], 3);
    assert!(body["by_date"].get(&today_key).is_some());
    assert!(body["by_date"].get(&yesterday_key).is_some());
    assert!(
        body["by_date"].get(&old_key).is_none(),
        "dashboard quick range must exclude usage outside the requested days window"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_token_usage_filters_ledger_by_agent_attribution() {
    let test = build_app();
    seed_usage_file(
        &test.tmp,
        &json!({
            "2025-05-10": {
                "openai:gpt-4": {
                    "provider_id": "openai",
                    "model_name": "gpt-4",
                    "prompt_tokens": 100,
                    "completion_tokens": 50,
                    "call_count": 2,
                    "agent_id": "research-agent"
                },
                "dashscope:qwen-plus": {
                    "provider_id": "dashscope",
                    "model_name": "qwen-plus",
                    "prompt_tokens": 20,
                    "completion_tokens": 10,
                    "call_count": 1,
                    "agent_id": "research-agent"
                }
            },
            "2025-05-11": {
                "openai:gpt-4": {
                    "provider_id": "openai",
                    "model_name": "gpt-4",
                    "prompt_tokens": 200,
                    "completion_tokens": 80,
                    "call_count": 3,
                    "agent_id": "other-agent"
                }
            }
        }),
    );

    let query = "start_date=2025-05-10&end_date=2025-05-11&provider_id=openai&model_name=gpt-4";
    let root = request(&test.app, Method::GET, &format!("/api/token-usage?{query}")).await;
    let scoped = request(
        &test.app,
        Method::GET,
        &format!("/api/agents/research-agent/token-usage?{query}"),
    )
    .await;

    assert_eq!(root.status(), StatusCode::OK);
    assert_eq!(scoped.status(), StatusCode::OK);
    let root_body = read_json(root).await;
    let scoped_body = read_json(scoped).await;

    assert_ne!(scoped_body, root_body);
    assert_eq!(root_body["total_prompt_tokens"], 300);
    assert_eq!(root_body["total_completion_tokens"], 130);
    assert_eq!(root_body["total_calls"], 5);
    assert_eq!(scoped_body["total_prompt_tokens"], 100);
    assert_eq!(scoped_body["total_completion_tokens"], 50);
    assert_eq!(scoped_body["total_calls"], 2);
    assert_eq!(scoped_body["by_date"]["2025-05-10"]["call_count"], 2);
    assert!(scoped_body["by_date"].get("2025-05-11").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn by_date_route_returns_rust_daily_usage_array_shape() {
    let test = build_app();
    seed_usage_file(
        &test.tmp,
        &json!({
            "2025-05-10": {
                "openai:gpt-4": {
                    "provider_id": "openai",
                    "model_name": "gpt-4",
                    "prompt_tokens": 100,
                    "completion_tokens": 50,
                    "call_count": 2
                }
            }
        }),
    );

    let resp = request(
        &test.app,
        Method::GET,
        "/api/token-usage/by-date?from=2025-05-10&to=2025-05-10",
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(
        body,
        json!([{
            "date": "2025-05-10",
            "prompt_tokens": 100,
            "completion_tokens": 50,
            "total_cost": 0.0
        }])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn date_filtered_summary_ignores_malformed_usage_date_buckets() {
    let test = build_app();
    seed_usage_file(
        &test.tmp,
        &json!({
            "not-a-date": {
                "openai:gpt-4": {
                    "provider_id": "openai",
                    "model_name": "gpt-4",
                    "prompt_tokens": 999,
                    "completion_tokens": 999,
                    "call_count": 9
                }
            },
            "2025-05-10": {
                "openai:gpt-4": {
                    "provider_id": "openai",
                    "model_name": "gpt-4",
                    "prompt_tokens": 100,
                    "completion_tokens": 50,
                    "call_count": 2
                }
            }
        }),
    );

    let resp = request(
        &test.app,
        Method::GET,
        "/api/token-usage?start_date=2025-05-10&end_date=2025-05-10",
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["total_prompt_tokens"], 100);
    assert_eq!(body["total_completion_tokens"], 50);
    assert_eq!(body["total_calls"], 2);
    assert!(body["by_date"].get("2025-05-10").is_some());
    assert!(body["by_date"].get("not-a-date").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn summary_uses_outer_usage_key_when_legacy_entries_omit_identity_fields() {
    let test = build_app();
    seed_usage_file(
        &test.tmp,
        &json!({
            "2025-05-10": {
                "openai:gpt-4": {
                    "prompt_tokens": 100,
                    "completion_tokens": 50,
                    "call_count": 2
                },
                "dashscope:qwen-plus": {
                    "provider_id": "dashscope",
                    "model_name": "",
                    "prompt_tokens": 20,
                    "completion_tokens": 10,
                    "call_count": 1
                }
            }
        }),
    );

    let resp = request(
        &test.app,
        Method::GET,
        "/api/token-usage?start_date=2025-05-10&end_date=2025-05-10&model_name=gpt-4",
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["total_prompt_tokens"], 100);
    assert_eq!(body["total_completion_tokens"], 50);
    assert_eq!(body["total_calls"], 2);
    assert_eq!(body["by_model"]["openai:gpt-4"]["provider_id"], "openai");
    assert_eq!(body["by_model"]["openai:gpt-4"]["model"], "gpt-4");
    assert!(body["by_model"].get("openai:").is_none());
    assert!(body["by_model"].get(":gpt-4").is_none());

    let resp = request(
        &test.app,
        Method::GET,
        "/api/token-usage?start_date=2025-05-10&end_date=2025-05-10&provider_id=dashscope&model_name=qwen-plus",
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["total_prompt_tokens"], 20);
    assert_eq!(body["total_completion_tokens"], 10);
    assert_eq!(body["total_calls"], 1);
    assert_eq!(
        body["by_model"]["dashscope:qwen-plus"]["model"],
        "qwen-plus"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn by_date_route_validates_date_filters_with_fastapi_style_detail() {
    let test = build_app();

    let invalid_from = request(
        &test.app,
        Method::GET,
        "/api/token-usage/by-date?from=not-a-date&to=2025-05-10",
    )
    .await;
    assert_eq!(invalid_from.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        read_json(invalid_from).await,
        json!({"detail": "invalid 'from' date: not-a-date"})
    );

    let invalid_to = request(
        &test.app,
        Method::GET,
        "/api/token-usage/by-date?from=2025-05-10&to=2025-99-99",
    )
    .await;
    assert_eq!(invalid_to.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        read_json(invalid_to).await,
        json!({"detail": "invalid 'to' date: 2025-99-99"})
    );

    let reversed = request(
        &test.app,
        Method::GET,
        "/api/token-usage/by-date?from=2025-05-11&to=2025-05-10",
    )
    .await;
    assert_eq!(reversed.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        read_json(reversed).await,
        json!({"detail": "'from' must not be after 'to'"})
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn details_route_returns_qwenpaw_style_date_provider_model_rows() {
    let test = build_app();
    seed_usage_file(
        &test.tmp,
        &json!({
            "2025-05-10": {
                "openai:gpt-4": {
                    "provider_id": "openai",
                    "model_name": "gpt-4",
                    "prompt_tokens": 100,
                    "completion_tokens": 50,
                    "call_count": 2
                },
                "dashscope:qwen-plus": {
                    "provider_id": "dashscope",
                    "model_name": "qwen-plus",
                    "prompt_tokens": 20,
                    "completion_tokens": 10,
                    "call_count": 1
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
                "openai:gpt-4o": {
                    "provider_id": "openai",
                    "model_name": "gpt-4o",
                    "prompt_tokens": 999,
                    "completion_tokens": 999,
                    "call_count": 9
                }
            }
        }),
    );

    let resp = request(
        &test.app,
        Method::GET,
        "/api/token-usage/details?start_date=2025-05-10&end_date=2025-05-11&provider=openai&model=gpt-4",
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(
        body,
        json!([
            {
                "date": "2025-05-10",
                "provider_id": "openai",
                "model": "gpt-4",
                "prompt_tokens": 100,
                "completion_tokens": 50,
                "call_count": 2
            },
            {
                "date": "2025-05-11",
                "provider_id": "openai",
                "model": "gpt-4",
                "prompt_tokens": 200,
                "completion_tokens": 80,
                "call_count": 3
            }
        ])
    );
}
