use std::collections::HashMap;

use agent_types::TokenUsage;
use chrono::{DateTime, Utc};
use rua_contract::{Tool, UsageRecordMetadata};
use rua_core::tool::ToolContext;
use rua_tool::extended::usage::GetTokenUsageTool;
use rua_usage::UsageLedger;
use serde_json::json;
use tempfile::TempDir;

fn make_ctx(root: &TempDir) -> ToolContext {
    ToolContext {
        agent_id: "agent".into(),
        session_id: "session".into(),
        working_dir: root.path().to_path_buf(),
        metadata: HashMap::new(),
        cancel: None,
        task_runtime: None,
        ..Default::default()
    }
}

fn write_usage(root: &TempDir, payload: serde_json::Value) {
    let mut records = Vec::new();
    if let Some(days) = payload.as_object() {
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
                    records.push(json!({
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
    let ledger = UsageLedger::open(root.path()).expect("usage ledger");
    for record in records {
        let at = DateTime::parse_from_rfc3339(record["finished_at"].as_str().expect("finished_at"))
            .expect("usage timestamp")
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

#[tokio::test]
async fn get_token_usage_treats_missing_or_corrupt_file_as_empty_summary() {
    let root = TempDir::new().expect("tempdir");
    let tool = GetTokenUsageTool::new();

    let result = tool
        .call(json!({"days": 30}), make_ctx(&root))
        .await
        .expect("tool result");
    let text = result.text_content();
    assert!(text.contains("- Total tokens: 0"), "got: {text}");
    assert!(
        text.contains("No token usage data in this period."),
        "got: {text}"
    );

    std::fs::write(
        root.path().join("token_usage_ledger.json"),
        "{not valid json",
    )
    .unwrap();
    let result = tool
        .call(json!({"days": 30}), make_ctx(&root))
        .await
        .expect("tool result");
    let text = result.text_content();
    assert!(text.contains("- Total calls: 0"), "got: {text}");
    assert!(
        !text.contains("corrupted"),
        "corrupt files should share the empty-summary path, got: {text}"
    );
}

#[tokio::test]
async fn get_token_usage_reuses_store_query_identity_fallback_and_filters() {
    let root = TempDir::new().expect("tempdir");
    let today = Utc::now().date_naive().format("%Y-%m-%d").to_string();
    write_usage(
        &root,
        json!({
            today: {
                "openai:gpt-4": {
                    "prompt_tokens": 1000,
                    "completion_tokens": 500,
                    "call_count": 2
                },
                "dashscope:qwen-plus": {
                    "provider_id": "dashscope",
                    "model_name": "",
                    "prompt_tokens": 999,
                    "completion_tokens": 1,
                    "call_count": 9
                }
            }
        }),
    );

    let result = GetTokenUsageTool::new()
        .call(
            json!({"days": 30, "provider_id": "openai", "model_name": "gpt-4"}),
            make_ctx(&root),
        )
        .await
        .expect("tool result");
    let text = result.text_content();

    assert!(text.contains("model=gpt-4"), "got: {text}");
    assert!(text.contains("provider=openai"), "got: {text}");
    assert!(text.contains("- Total tokens: 1,500"), "got: {text}");
    assert!(text.contains("- Total calls: 2"), "got: {text}");
    assert!(text.contains("  - openai:gpt-4: 1,500 tokens (2 calls)"));
    assert!(
        !text.contains("dashscope:qwen-plus"),
        "filters should apply after identity fallback, got: {text}"
    );
}
