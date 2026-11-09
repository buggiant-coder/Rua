use agent_types::TokenUsage;
use chrono::{TimeZone, Utc};
use rua_contract::{UsageRecordMetadata, UsageRecorder};
use rua_usage::{TokenUsageFilter, UsageLedger};

fn usage() -> TokenUsage {
    TokenUsage {
        prompt_tokens: 120,
        completion_tokens: 30,
        reasoning_tokens: Some(10),
        cached_tokens: Some(5),
        cache_write_tokens: Some(2),
        ..Default::default()
    }
}

#[tokio::test]
async fn persists_compact_usage_and_deduplicates_usage_id() {
    let temp = tempfile::tempdir().unwrap();
    let ledger = UsageLedger::open(temp.path()).unwrap();
    let started = Utc.with_ymd_and_hms(2026, 8, 27, 12, 0, 0).unwrap();
    let finished = Utc.with_ymd_and_hms(2026, 8, 27, 12, 0, 1).unwrap();
    let mut metadata = UsageRecordMetadata {
        usage_id: Some("same-call".into()),
        agent_id: Some("agent-1".into()),
        ..Default::default()
    };
    metadata.extra.insert(
        "rua_model_call_context".into(),
        serde_json::Value::String("x".repeat(1_000_000)),
    );

    ledger
        .record_usage(
            "openai",
            "gpt-test",
            &usage(),
            started,
            finished,
            metadata.clone(),
        )
        .await;
    ledger
        .record_usage("openai", "gpt-test", &usage(), started, finished, metadata)
        .await;

    let reopened = UsageLedger::open(temp.path()).unwrap();
    let summary = reopened.query(&TokenUsageFilter::default()).unwrap();
    assert_eq!(summary.total_calls, 1);
    assert_eq!(
        summary.total_prompt_tokens + summary.total_completion_tokens,
        150
    );
    assert!(std::fs::metadata(reopened.database_path()).unwrap().len() < 200_000);
}

#[tokio::test]
async fn filters_and_aggregates_records() {
    let temp = tempfile::tempdir().unwrap();
    let ledger = UsageLedger::open(temp.path()).unwrap();
    for (id, provider, model, day) in [
        ("one", "openai", "gpt", 26),
        ("two", "openai", "gpt", 27),
        ("three", "anthropic", "claude", 27),
    ] {
        let at = Utc.with_ymd_and_hms(2026, 8, day, 12, 0, 0).unwrap();
        ledger
            .record_usage(
                provider,
                model,
                &usage(),
                at,
                at,
                UsageRecordMetadata {
                    usage_id: Some(id.into()),
                    ..Default::default()
                },
            )
            .await;
    }

    let summary = ledger
        .query(&TokenUsageFilter {
            start_date: Some("2026-08-27".into()),
            end_date: Some("2026-08-27".into()),
            provider_id: Some("openai".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(summary.total_calls, 1);
    assert_eq!(summary.by_model["openai:gpt"].model, "gpt");
    assert_eq!(summary.by_date["2026-08-27"].call_count, 1);
}

#[test]
fn successful_open_removes_legacy_ledgers() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("token_usage_ledger.json"), b"discarded").unwrap();
    std::fs::create_dir(temp.path().join("token_usage_ledger")).unwrap();
    std::fs::write(
        temp.path().join("token_usage_ledger/2026-08-27.json"),
        b"discarded",
    )
    .unwrap();
    std::fs::write(
        temp.path().join(".token_usage_ledger.json.tmp.1.1"),
        b"discarded",
    )
    .unwrap();

    UsageLedger::open(temp.path()).unwrap();

    assert!(!temp.path().join("token_usage_ledger.json").exists());
    assert!(!temp.path().join("token_usage_ledger").exists());
    assert!(!temp
        .path()
        .join(".token_usage_ledger.json.tmp.1.1")
        .exists());
    assert!(temp.path().join("token-usage.sqlite3").exists());
}

#[test]
fn lazy_ledger_removes_legacy_ledgers_on_first_database_access() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("token_usage_ledger.json"), b"discarded").unwrap();
    std::fs::create_dir(temp.path().join("token_usage_ledger")).unwrap();

    let ledger = UsageLedger::new(temp.path());
    assert!(temp.path().join("token_usage_ledger.json").exists());
    ledger.query(&TokenUsageFilter::default()).unwrap();

    assert!(!temp.path().join("token_usage_ledger.json").exists());
    assert!(!temp.path().join("token_usage_ledger").exists());
}
