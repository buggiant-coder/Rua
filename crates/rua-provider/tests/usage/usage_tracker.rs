use std::fs;

use agent_types::TokenUsage;
use chrono::{Local, NaiveDate};
use rua_config::types::token_usage::TokenUsageFile;
use rua_provider::usage::UsageTracker;
use tempfile::TempDir;

#[test]
fn record_creates_file_with_correct_format() {
    let dir = TempDir::new().unwrap();
    let tracker = UsageTracker::new(dir.path().to_path_buf());
    let usage = TokenUsage::new(100, 50);
    tracker.record("openai", "gpt-4", &usage, None);

    let content = fs::read_to_string(dir.path().join("token_usage.json")).unwrap();
    let parsed: TokenUsageFile = serde_json::from_str(&content).unwrap();

    let today = Local::now().date_naive().format("%Y-%m-%d").to_string();
    assert!(parsed.contains_key(&today));
    let day = &parsed[&today];
    assert!(day.contains_key("openai:gpt-4"));
    let entry = &day["openai:gpt-4"];
    assert_eq!(entry.prompt_tokens, 100);
    assert_eq!(entry.completion_tokens, 50);
    assert_eq!(entry.call_count, 1);
    assert_eq!(entry.provider_id, "openai");
    assert_eq!(entry.model_name, "gpt-4");
}

#[test]
fn multiple_records_accumulate() {
    let dir = TempDir::new().unwrap();
    let tracker = UsageTracker::new(dir.path().to_path_buf());
    tracker.record("openai", "gpt-4", &TokenUsage::new(100, 50), None);
    tracker.record("openai", "gpt-4", &TokenUsage::new(200, 80), None);

    let content = fs::read_to_string(dir.path().join("token_usage.json")).unwrap();
    let parsed: TokenUsageFile = serde_json::from_str(&content).unwrap();
    let today = Local::now().date_naive().format("%Y-%m-%d").to_string();
    let entry = &parsed[&today]["openai:gpt-4"];
    assert_eq!(entry.prompt_tokens, 300);
    assert_eq!(entry.completion_tokens, 130);
    assert_eq!(entry.call_count, 2);
}

#[test]
fn snapshot_reflects_all_records() {
    let dir = TempDir::new().unwrap();
    let tracker = UsageTracker::new(dir.path().to_path_buf());
    tracker.record("openai", "gpt-4", &TokenUsage::new(100, 50), None);
    tracker.record("dashscope", "qwen3", &TokenUsage::new(200, 80), None);

    let snap = tracker.snapshot();
    assert_eq!(snap.total_requests, 2);
    assert_eq!(snap.grand_total.prompt_tokens, 300);
    assert_eq!(snap.grand_total.completion_tokens, 130);
    assert_eq!(snap.by_model.len(), 2);
}

#[test]
fn concurrent_writes_no_data_loss() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_path_buf();

    let handles: Vec<_> = (0..2)
        .map(|i| {
            let p = path.clone();
            std::thread::spawn(move || {
                let tracker = UsageTracker::new(p);
                for _ in 0..10 {
                    tracker.record(
                        "provider",
                        &format!("model-{i}"),
                        &TokenUsage::new(1, 1),
                        None,
                    );
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    let content = fs::read_to_string(path.join("token_usage.json")).unwrap();
    let parsed: TokenUsageFile = serde_json::from_str(&content).unwrap();
    let today = Local::now().date_naive().format("%Y-%m-%d").to_string();
    let day = &parsed[&today];
    let total: u64 = day.values().map(|e| e.call_count).sum();
    assert_eq!(total, 20);
}

#[test]
fn python_compat_read() {
    let dir = TempDir::new().unwrap();
    let sample = r#"{
  "2024-01-15": {
    "dashscope:qwen3-max": {
      "provider_id": "dashscope",
      "model_name": "qwen3-max",
      "prompt_tokens": 1000,
      "completion_tokens": 500,
      "call_count": 5
    },
    "openai:gpt-4": {
      "provider_id": "openai",
      "model_name": "gpt-4",
      "prompt_tokens": 2000,
      "completion_tokens": 800,
      "call_count": 3
    }
  },
  "2024-01-16": {
    "dashscope:qwen3-max": {
      "provider_id": "dashscope",
      "model_name": "qwen3-max",
      "prompt_tokens": 500,
      "completion_tokens": 250,
      "call_count": 2
    }
  }
}"#;
    fs::write(dir.path().join("token_usage.json"), sample).unwrap();

    let tracker = UsageTracker::new(dir.path().to_path_buf());
    let snap = tracker.snapshot();

    assert_eq!(snap.total_requests, 10);
    assert_eq!(snap.grand_total.prompt_tokens, 3500);
    assert_eq!(snap.grand_total.completion_tokens, 1550);
    assert_eq!(snap.by_model.len(), 3);

    let qwen_jan15 = tracker
        .get(
            NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(),
            "dashscope",
            "qwen3-max",
        )
        .unwrap();
    assert_eq!(qwen_jan15.total.prompt_tokens, 1000);
    assert_eq!(qwen_jan15.request_count, 5);
}

#[test]
fn python_compat_write_then_python_read() {
    let dir = TempDir::new().unwrap();
    let tracker = UsageTracker::new(dir.path().to_path_buf());
    tracker.record("dashscope", "qwen3-max", &TokenUsage::new(1000, 500), None);

    let content = fs::read_to_string(dir.path().join("token_usage.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();

    let today = Local::now().date_naive().format("%Y-%m-%d").to_string();
    let entry = &parsed[&today]["dashscope:qwen3-max"];
    assert_eq!(entry["provider_id"], "dashscope");
    assert_eq!(entry["model_name"], "qwen3-max");
    assert_eq!(entry["prompt_tokens"], 1000);
    assert_eq!(entry["completion_tokens"], 500);
    assert_eq!(entry["call_count"], 1);
}

#[test]
fn load_merges_with_new_records() {
    let dir = TempDir::new().unwrap();
    let sample = r#"{
  "2024-01-15": {
    "openai:gpt-4": {
      "provider_id": "openai",
      "model_name": "gpt-4",
      "prompt_tokens": 100,
      "completion_tokens": 50,
      "call_count": 1
    }
  }
}"#;
    fs::write(dir.path().join("token_usage.json"), sample).unwrap();

    let tracker = UsageTracker::new(dir.path().to_path_buf());
    tracker.record("openai", "gpt-4", &TokenUsage::new(200, 80), None);

    let snap = tracker.snapshot();
    assert_eq!(snap.grand_total.prompt_tokens, 300);
    if Local::now().date_naive() != NaiveDate::from_ymd_opt(2024, 1, 15).unwrap() {
        assert_eq!(snap.by_model.len(), 2);
    }
}

#[test]
fn empty_usage_not_recorded() {
    let dir = TempDir::new().unwrap();
    let tracker = UsageTracker::new(dir.path().to_path_buf());
    tracker.record("openai", "gpt-4", &TokenUsage::default(), None);
    assert!(!dir.path().join("token_usage.json").exists());
}

#[test]
fn old_format_migration() {
    let dir = TempDir::new().unwrap();
    let old_format = r#"{
  "openai:gpt-4": {
    "provider_id": "openai",
    "model_name": "gpt-4",
    "prompt_tokens": 500,
    "completion_tokens": 200,
    "call_count": 3
  }
}"#;
    fs::write(dir.path().join("token_usage.json"), old_format).unwrap();

    let tracker = UsageTracker::new(dir.path().to_path_buf());

    assert!(dir.path().join("token_usage.json.bak").exists());

    let snap = tracker.snapshot();
    assert_eq!(snap.total_requests, 3);
    assert_eq!(snap.grand_total.prompt_tokens, 500);
    assert_eq!(snap.grand_total.completion_tokens, 200);
}

#[test]
fn date_key_uses_local_timezone() {
    let dir = TempDir::new().unwrap();
    let tracker = UsageTracker::new(dir.path().to_path_buf());
    tracker.record("test", "model", &TokenUsage::new(1, 1), None);

    let content = fs::read_to_string(dir.path().join("token_usage.json")).unwrap();
    let parsed: TokenUsageFile = serde_json::from_str(&content).unwrap();

    let local_today = Local::now().date_naive().format("%Y-%m-%d").to_string();
    assert!(
        parsed.contains_key(&local_today),
        "expected local date key {local_today}, got keys: {:?}",
        parsed.keys().collect::<Vec<_>>()
    );
}

#[test]
fn get_returns_none_for_missing() {
    let dir = TempDir::new().unwrap();
    let tracker = UsageTracker::new(dir.path().to_path_buf());
    assert!(tracker.get(Local::now().date_naive(), "x", "y").is_none());
}

#[test]
fn reset_clears_in_memory() {
    let dir = TempDir::new().unwrap();
    let tracker = UsageTracker::new(dir.path().to_path_buf());
    tracker.record("p", "m", &TokenUsage::new(1, 1), None);
    tracker.reset();
    assert_eq!(tracker.total().prompt_tokens, 0);
}

#[test]
fn record_with_explicit_date() {
    let dir = TempDir::new().unwrap();
    let tracker = UsageTracker::new(dir.path().to_path_buf());
    let target = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap();
    tracker.record("openai", "gpt-4", &TokenUsage::new(100, 50), Some(target));

    let content = fs::read_to_string(dir.path().join("token_usage.json")).unwrap();
    let parsed: TokenUsageFile = serde_json::from_str(&content).unwrap();

    assert!(parsed.contains_key("2025-01-01"));
    let entry = &parsed["2025-01-01"]["openai:gpt-4"];
    assert_eq!(entry.prompt_tokens, 100);
    assert_eq!(entry.completion_tokens, 50);
    assert_eq!(entry.call_count, 1);

    let acc = tracker.get(target, "openai", "gpt-4").unwrap();
    assert_eq!(acc.total.prompt_tokens, 100);
    assert_eq!(acc.request_count, 1);
}
