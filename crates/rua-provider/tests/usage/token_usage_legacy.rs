use rua_provider::usage::{normalize_legacy_format, UsageTracker};
use serde_json::json;
use std::fs;

#[test]
fn normalize_flat_format_wraps_under_today() {
    let flat = json!({
        "openai:gpt-4": {
            "provider_id": "openai",
            "model_name": "gpt-4",
            "prompt_tokens": 500,
            "completion_tokens": 200,
            "call_count": 3
        },
        "dashscope:qwen3-max": {
            "provider_id": "dashscope",
            "model_name": "qwen3-max",
            "prompt_tokens": 1000,
            "completion_tokens": 400,
            "call_count": 5
        }
    });

    let result = normalize_legacy_format(flat);
    let obj = result.as_object().unwrap();

    assert_eq!(obj.len(), 1, "should have exactly one date key");
    let (date_key, models) = obj.iter().next().unwrap();
    assert!(
        chrono::NaiveDate::parse_from_str(date_key, "%Y-%m-%d").is_ok(),
        "top-level key should be a date"
    );
    let models = models.as_object().unwrap();
    assert_eq!(models.len(), 2);
    assert_eq!(models["openai:gpt-4"]["prompt_tokens"], 500);
    assert_eq!(models["dashscope:qwen3-max"]["call_count"], 5);
}

#[test]
fn normalize_backfills_missing_provider_and_model() {
    let input = json!({
        "2024-06-01": {
            "dashscope:qwen3-max": {
                "prompt_tokens": 800,
                "completion_tokens": 300,
                "call_count": 2
            }
        }
    });

    let result = normalize_legacy_format(input);
    let entry = &result["2024-06-01"]["dashscope:qwen3-max"];
    assert_eq!(entry["provider_id"], "dashscope");
    assert_eq!(entry["model_name"], "qwen3-max");
    assert_eq!(entry["prompt_tokens"], 800);
}

#[test]
fn normalize_preserves_existing_provider_and_model() {
    let input = json!({
        "2024-06-01": {
            "dashscope:qwen3-max": {
                "provider_id": "dashscope",
                "model_name": "qwen3-max",
                "prompt_tokens": 100,
                "completion_tokens": 50,
                "call_count": 1
            }
        }
    });

    let result = normalize_legacy_format(input.clone());
    assert_eq!(result, input, "already-valid format should be unchanged");
}

#[test]
fn normalize_roundtrip_new_format_is_identity() {
    let new_format = json!({
        "2024-01-15": {
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
    });

    let result = normalize_legacy_format(new_format.clone());
    assert_eq!(result, new_format);
}

#[test]
fn normalize_empty_object_returns_empty() {
    let result = normalize_legacy_format(json!({}));
    assert_eq!(result, json!({}));
}

#[test]
fn normalize_flat_backfills_missing_fields() {
    let flat = json!({
        "openai:gpt-4": {
            "prompt_tokens": 100
        }
    });

    let result = normalize_legacy_format(flat);
    let obj = result.as_object().unwrap();
    let (_, models) = obj.iter().next().unwrap();
    let entry = &models["openai:gpt-4"];
    assert_eq!(entry["provider_id"], "openai");
    assert_eq!(entry["model_name"], "gpt-4");
    assert_eq!(entry["completion_tokens"], 0);
    assert_eq!(entry["call_count"], 0);
}

#[test]
fn load_python_legacy_flat_fixture() {
    let dir = tempfile::TempDir::new().unwrap();
    let fixture = r#"{
  "dashscope:qwen3-max": {
    "provider_id": "dashscope",
    "model_name": "qwen3-max",
    "prompt_tokens": 12000,
    "completion_tokens": 4500,
    "call_count": 42
  },
  "openai:gpt-4": {
    "provider_id": "openai",
    "model_name": "gpt-4",
    "prompt_tokens": 8000,
    "completion_tokens": 3200,
    "call_count": 15
  }
}"#;
    fs::write(dir.path().join("token_usage.json"), fixture).unwrap();

    let tracker = UsageTracker::new(dir.path().to_path_buf());
    let snap = tracker.snapshot();

    assert_eq!(snap.total_requests, 57);
    assert_eq!(snap.grand_total.prompt_tokens, 20000);
    assert_eq!(snap.grand_total.completion_tokens, 7700);
}

#[test]
fn load_python_legacy_missing_fields_fixture() {
    let dir = tempfile::TempDir::new().unwrap();
    let fixture = r#"{
  "2024-03-10": {
    "dashscope:qwen3-max": {
      "prompt_tokens": 5000,
      "completion_tokens": 2000,
      "call_count": 10
    }
  }
}"#;
    fs::write(dir.path().join("token_usage.json"), fixture).unwrap();

    let tracker = UsageTracker::new(dir.path().to_path_buf());
    let snap = tracker.snapshot();

    assert_eq!(snap.total_requests, 10);
    assert_eq!(snap.grand_total.prompt_tokens, 5000);
    assert_eq!(snap.grand_total.completion_tokens, 2000);
    assert_eq!(snap.by_model.len(), 1);
    assert_eq!(snap.by_model[0].provider_id, "dashscope");
    assert_eq!(snap.by_model[0].model_id, "qwen3-max");
}

#[test]
fn load_python_legacy_then_record_new() {
    let dir = tempfile::TempDir::new().unwrap();
    let fixture = r#"{
  "2024-01-15": {
    "openai:gpt-4": {
      "prompt_tokens": 100,
      "completion_tokens": 50,
      "call_count": 1
    }
  }
}"#;
    fs::write(dir.path().join("token_usage.json"), fixture).unwrap();

    let tracker = UsageTracker::new(dir.path().to_path_buf());
    tracker.record(
        "openai",
        "gpt-4",
        &agent_types::TokenUsage::new(200, 80),
        None,
    );

    let snap = tracker.snapshot();
    assert_eq!(snap.grand_total.prompt_tokens, 300);
    assert_eq!(snap.grand_total.completion_tokens, 130);
}
