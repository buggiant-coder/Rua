use rua_config::types::{
    AgentProfileConfig, AgentsRunningConfig, ChatRunningConfig, MemorySearchMode, ModelSlotConfig,
    ToolDisclosureEnabled,
};
use serde_json::json;

#[test]
fn running_config_llm_retry_defaults_are_stable() {
    let running = AgentsRunningConfig::default();

    assert!(!running.show_debug_menu);
    assert!(!running.llm_retry_enabled);
    assert_eq!(running.llm_max_retries, 1);
    assert_eq!(running.llm_backoff_base, 1.0);
    assert_eq!(running.llm_backoff_cap, 30.0);
}

#[test]
fn running_config_blocking_compaction_defaults_to_ninety_percent() {
    let running = AgentsRunningConfig::default();

    assert_eq!(running.context_compact.blocking_compact_ratio, 0.9);
}

#[test]
fn zero_iterations_roundtrips_as_unlimited_in_agent_and_chat_configuration() {
    let running: AgentsRunningConfig = serde_json::from_value(json!({"max_iters": 0})).unwrap();
    running
        .validate()
        .expect("zero disables the tool-round limit");
    let chat = ChatRunningConfig::from_agent_defaults(&running);
    chat.validate().unwrap();
    let restored: ChatRunningConfig =
        serde_json::from_value(serde_json::to_value(chat).unwrap()).unwrap();
    assert_eq!(restored.resolve().unwrap().max_iters, 0);
    assert!(serde_json::from_value::<AgentsRunningConfig>(json!({"max_iters": -1})).is_err());
}

#[test]
fn chat_running_config_snapshots_chat_owned_fields_without_credentials() {
    let mut defaults = AgentsRunningConfig {
        max_iters: 80,
        max_input_length: 200_000,
        ..AgentsRunningConfig::default()
    };
    defaults.context_compact.active_work_plan_injection_enabled = true;
    defaults
        .context_compact
        .active_work_plan_injection_interval_ratio = 0.07;
    defaults.embedding_config.api_key = "secret".to_string();
    defaults.llm_max_concurrent = 9;

    let snapshot = ChatRunningConfig::from_agent_defaults(&defaults);
    let serialized = serde_json::to_value(&snapshot).unwrap();

    assert_eq!(snapshot.max_iters, 80);
    assert_eq!(snapshot.max_input_length, 200_000);
    assert!(snapshot.context_compact.active_work_plan_injection_enabled);
    assert_eq!(
        snapshot
            .context_compact
            .active_work_plan_injection_interval_ratio,
        0.07
    );
    assert!(serialized.get("embedding_config").is_none());
    assert_eq!(serialized["llm_max_concurrent"], 9);

    let mut changed_agent = defaults;
    changed_agent.max_iters = 100;
    changed_agent.max_input_length = 1_000_000;
    changed_agent.llm_max_concurrent = 3;
    let new_snapshot = ChatRunningConfig::from_agent_defaults(&changed_agent);
    assert_eq!(new_snapshot.max_input_length, 1_000_000);
    let effective = snapshot.resolve().unwrap();
    assert_eq!(effective.max_iters, 80);
    assert_eq!(effective.max_input_length, 200_000);
    assert_eq!(effective.llm_max_concurrent, 9);
}

#[test]
fn chat_running_config_roundtrip_freezes_all_behavior_fields() {
    let original: AgentsRunningConfig = serde_json::from_value(json!({
        "max_iters": 17,
        "auto_continue_on_text_only": true,
        "show_debug_menu": true,
        "llm_retry_enabled": true,
        "llm_max_retries": 7,
        "llm_backoff_base": 1.5,
        "llm_backoff_cap": 19.0,
        "llm_max_concurrent": 2,
        "llm_max_qpm": 27,
        "llm_rate_limit_pause": 3.0,
        "llm_rate_limit_jitter": 0.2,
        "llm_acquire_timeout": 11.0,
        "history_max_length": 37,
        "future_behavior": {"enabled": true}
    }))
    .unwrap();
    let snapshot = ChatRunningConfig::from_agent_defaults(&original);
    let restored: ChatRunningConfig =
        serde_json::from_value(serde_json::to_value(snapshot).unwrap()).unwrap();
    let effective = restored.resolve().unwrap();
    assert_eq!(effective, original);
}

#[test]
fn running_config_tool_disclosure_defaults_are_stable() {
    let running = AgentsRunningConfig::default();
    let disclosure = running.tool_disclosure.normalized();

    assert_eq!(disclosure.enabled, ToolDisclosureEnabled::Auto);
    assert_eq!(disclosure.threshold_pct, 10.0);
    assert_eq!(disclosure.search_default_limit, 5);
    assert_eq!(disclosure.search_max_limit, 20);
}

#[test]
fn running_config_tool_output_bounds_have_independent_defaults() {
    let tool_output = AgentsRunningConfig::default().tool_result_compact;

    assert_eq!(tool_output.execution_output_max_tokens, 10_000);
    assert_eq!(tool_output.raw_capture_max_bytes, 1024 * 1024);
    assert_eq!(tool_output.recall_slice_max_tokens, 10_000);
    assert_eq!(tool_output.old_max_bytes, 3_000);
}

#[test]
fn projection_truncation_batch_size_defaults_and_roundtrips() {
    for tool_config in [json!({}), json!({"projection_truncation_batch_size": null})] {
        let running: AgentsRunningConfig = serde_json::from_value(json!({
            "tool_result_compact": tool_config
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(running).unwrap()["tool_result_compact"]
                ["projection_truncation_batch_size"],
            1
        );
    }

    let running: AgentsRunningConfig = serde_json::from_value(json!({
        "tool_result_compact": {
            "projection_truncation_batch_size": 10,
            "future_tool_option": "preserved"
        }
    }))
    .unwrap();
    running.validate().unwrap();
    let chat = ChatRunningConfig::from_agent_defaults(&running);
    let restored: ChatRunningConfig =
        serde_json::from_value(serde_json::to_value(chat).unwrap()).unwrap();
    let resolved = restored.resolve().unwrap();
    let value = serde_json::to_value(resolved).unwrap();
    assert_eq!(
        value["tool_result_compact"]["projection_truncation_batch_size"],
        10
    );
    assert_eq!(
        value["tool_result_compact"]["future_tool_option"],
        "preserved"
    );
}

#[test]
fn projection_truncation_batch_size_rejects_zero_in_agent_and_chat_config() {
    let running: AgentsRunningConfig = serde_json::from_value(json!({
        "tool_result_compact": {"projection_truncation_batch_size": 0}
    }))
    .unwrap();
    assert!(running
        .validate()
        .unwrap_err()
        .contains("projection_truncation_batch_size"));
    assert!(ChatRunningConfig::from_agent_defaults(&running)
        .validate()
        .unwrap_err()
        .contains("projection_truncation_batch_size"));
}

#[test]
fn running_config_migrates_legacy_default_recent_bytes_to_new_token_default() {
    let running: AgentsRunningConfig = serde_json::from_value(json!({
        "tool_result_compact": {
            "recent_max_bytes": 10000
        }
    }))
    .expect("legacy default tool output config should migrate");

    assert_eq!(
        running.tool_result_compact.execution_output_max_tokens,
        10_000
    );
}

#[test]
fn running_config_migrates_legacy_recent_bytes_without_reinterpreting_as_tokens() {
    let running: AgentsRunningConfig = serde_json::from_value(json!({
        "tool_result_compact": {
            "recent_max_bytes": 8000,
            "old_max_bytes": 1234
        }
    }))
    .expect("legacy tool output config should migrate");

    assert_eq!(
        running.tool_result_compact.execution_output_max_tokens,
        2_000
    );
    assert_eq!(running.tool_result_compact.old_max_bytes, 1_234);
    let value = serde_json::to_value(running).expect("running config should serialize");
    assert_eq!(
        value["tool_result_compact"]["execution_output_max_tokens"],
        2_000
    );
    assert!(value["tool_result_compact"]
        .get("recent_max_bytes")
        .is_none());
}

#[test]
fn running_config_new_tool_output_token_limit_wins_over_legacy_bytes() {
    let running: AgentsRunningConfig = serde_json::from_value(json!({
        "tool_result_compact": {
            "execution_output_max_tokens": 9000,
            "recent_max_bytes": 8000,
            "raw_capture_max_bytes": 500000,
            "recall_slice_max_tokens": 3000
        }
    }))
    .expect("new tool output fields should deserialize");

    assert_eq!(
        running.tool_result_compact.execution_output_max_tokens,
        9_000
    );
    assert_eq!(running.tool_result_compact.raw_capture_max_bytes, 500_000);
    assert_eq!(running.tool_result_compact.recall_slice_max_tokens, 3_000);
}

#[test]
fn running_config_null_fields_deserialize_to_defaults() {
    let running: AgentsRunningConfig = serde_json::from_value(json!({
        "max_iters": null,
        "llm_retry_enabled": null,
        "llm_max_retries": null,
        "llm_backoff_base": null,
        "llm_backoff_cap": null,
        "llm_max_concurrent": null,
        "llm_rate_limit_pause": null,
        "llm_acquire_timeout": null,
        "max_input_length": null,
        "history_max_length": null,
        "tool_disclosure": null,
        "memory_manager_backend": null
    }))
    .expect("running config with nulls should deserialize");

    assert_eq!(running.max_iters, 100);
    assert!(!running.llm_retry_enabled);
    assert_eq!(running.llm_max_retries, 1);
    assert_eq!(running.llm_backoff_base, 1.0);
    assert_eq!(running.llm_backoff_cap, 30.0);
    assert_eq!(running.llm_max_concurrent, 5);
    assert_eq!(running.llm_rate_limit_pause, 30.0);
    assert_eq!(running.llm_acquire_timeout, 120.0);
    assert_eq!(running.max_input_length, 131072);
    assert_eq!(running.history_max_length, 10000);
    assert_eq!(
        running.tool_disclosure.normalized().enabled,
        ToolDisclosureEnabled::Auto
    );
    assert_eq!(running.memory_manager_backend, "remelight");
}

#[test]
fn early_background_compaction_migrates_legacy_perpetual_keys_on_write() {
    let running: AgentsRunningConfig = serde_json::from_value(json!({
        "context_compact": {
            "perpetual_conversation_enabled": true,
            "perpetual_sweet_spot_ratio": 0.42
        }
    }))
    .expect("legacy early-compaction keys should deserialize");

    assert!(running.context_compact.early_background_compaction_enabled);
    assert_eq!(running.context_compact.early_compaction_target_ratio, 0.42);

    let serialized = serde_json::to_value(running).expect("running config should serialize");
    let context = serialized["context_compact"]
        .as_object()
        .expect("context_compact should be an object");
    assert_eq!(
        context.get("early_background_compaction_enabled"),
        Some(&json!(true))
    );
    assert_eq!(
        context.get("early_compaction_target_ratio"),
        Some(&json!(0.42))
    );
    assert!(!context.contains_key("perpetual_conversation_enabled"));
    assert!(!context.contains_key("perpetual_sweet_spot_ratio"));
}

#[test]
fn memory_summary_dream_cron_preserves_empty_but_defaults_null_and_missing() {
    let explicit_empty: AgentsRunningConfig = serde_json::from_value(json!({
        "memory_summary": {
            "dream_cron": ""
        }
    }))
    .expect("running config should preserve explicit empty dream_cron");

    assert_eq!(
        explicit_empty.memory_summary.dream_cron, "",
        "empty dream_cron disables the dream job and must not be replaced by the placeholder"
    );

    let null_cron: AgentsRunningConfig = serde_json::from_value(json!({
        "memory_summary": {
            "dream_cron": null
        }
    }))
    .expect("running config should default null dream_cron");
    let missing_cron: AgentsRunningConfig = serde_json::from_value(json!({
        "memory_summary": {}
    }))
    .expect("running config should default missing dream_cron");
    let null_summary: AgentsRunningConfig = serde_json::from_value(json!({
        "memory_summary": null
    }))
    .expect("running config should default null memory_summary");

    for running in [null_cron, missing_cron, null_summary] {
        assert_eq!(running.memory_summary.dream_cron, "0 23 * * *");
    }
}

#[test]
fn memory_summary_search_mode_defaults_precise_and_preserves_grep() {
    let default_running: AgentsRunningConfig = serde_json::from_value(json!({
        "memory_summary": {}
    }))
    .expect("running config should default missing search_mode");
    assert_eq!(
        default_running.memory_summary.search_mode,
        MemorySearchMode::Precise
    );

    let grep_running: AgentsRunningConfig = serde_json::from_value(json!({
        "memory_summary": {
            "search_mode": "grep"
        }
    }))
    .expect("running config should preserve grep search_mode");
    assert_eq!(
        grep_running.memory_summary.search_mode,
        MemorySearchMode::Grep
    );
}

#[test]
fn running_config_tool_disclosure_preserves_explicit_settings() {
    let running: AgentsRunningConfig = serde_json::from_value(json!({
        "tool_disclosure": {
            "enabled": "on",
            "threshold_pct": 7.5,
            "search_default_limit": 3,
            "search_max_limit": 9
        }
    }))
    .expect("running config should deserialize");

    let disclosure = running.tool_disclosure.normalized();
    assert_eq!(disclosure.enabled, ToolDisclosureEnabled::On);
    assert_eq!(disclosure.threshold_pct, 7.5);
    assert_eq!(disclosure.search_default_limit, 3);
    assert_eq!(disclosure.search_max_limit, 9);
}

#[test]
fn running_config_tool_disclosure_defaults_invalid_mode_and_clamps_numbers() {
    let running: AgentsRunningConfig = serde_json::from_value(json!({
        "tool_disclosure": {
            "enabled": "surprise",
            "threshold_pct": 0.0,
            "search_default_limit": 0,
            "search_max_limit": 0
        }
    }))
    .expect("invalid disclosure mode should fall back safely");

    let disclosure = running.tool_disclosure.normalized();
    assert_eq!(disclosure.enabled, ToolDisclosureEnabled::Auto);
    assert_eq!(disclosure.threshold_pct, 10.0);
    assert_eq!(disclosure.search_default_limit, 5);
    assert_eq!(disclosure.search_max_limit, 20);
}

#[test]
fn running_config_preserves_retry_settings_and_unknown_fields() {
    let running: AgentsRunningConfig = serde_json::from_value(json!({
        "llm_retry_enabled": true,
        "llm_max_retries": 5,
        "llm_backoff_base": 0.5,
        "llm_backoff_cap": 8.0,
        "future_retry_field": "kept"
    }))
    .expect("running config should deserialize");

    assert!(running.llm_retry_enabled);
    assert_eq!(running.llm_max_retries, 5);
    assert_eq!(running.llm_backoff_base, 0.5);
    assert_eq!(running.llm_backoff_cap, 8.0);
    assert_eq!(running.extra["future_retry_field"], "kept");
}

#[test]
fn running_config_rejects_backoff_cap_below_base() {
    let running = AgentsRunningConfig {
        llm_backoff_base: 2.0,
        llm_backoff_cap: 1.0,
        ..Default::default()
    };

    let err = running.validate().expect_err("cap below base should fail");

    assert!(err.contains("llm_backoff_cap"));
    assert!(err.contains("llm_backoff_base"));
}

#[test]
fn running_config_rejects_values_below_console_and_provider_safety_floor() {
    let cases = [
        (
            AgentsRunningConfig {
                llm_max_retries: 0,
                ..Default::default()
            },
            "llm_max_retries",
        ),
        (
            AgentsRunningConfig {
                llm_backoff_base: 0.01,
                ..Default::default()
            },
            "llm_backoff_base",
        ),
        (
            AgentsRunningConfig {
                llm_backoff_cap: 0.1,
                llm_backoff_base: 0.1,
                ..Default::default()
            },
            "llm_backoff_cap",
        ),
        (
            AgentsRunningConfig {
                llm_max_concurrent: 0,
                ..Default::default()
            },
            "llm_max_concurrent",
        ),
        (
            AgentsRunningConfig {
                llm_rate_limit_pause: 0.1,
                ..Default::default()
            },
            "llm_rate_limit_pause",
        ),
        (
            AgentsRunningConfig {
                llm_rate_limit_jitter: -1.0,
                ..Default::default()
            },
            "llm_rate_limit_jitter",
        ),
        (
            AgentsRunningConfig {
                llm_acquire_timeout: 1.0,
                ..Default::default()
            },
            "llm_acquire_timeout",
        ),
        (
            AgentsRunningConfig {
                max_input_length: 999,
                ..Default::default()
            },
            "max_input_length",
        ),
    ];

    for (running, field) in cases {
        let err = running
            .validate()
            .expect_err(&format!("{field} should be rejected"));
        assert!(
            err.contains(field),
            "error for {field} should name the field, got {err}"
        );
    }
}

#[test]
fn default_model_slot_roundtrips_provider_model_and_unknown_fields() {
    let slot: ModelSlotConfig = serde_json::from_value(json!({
        "provider_id": "dashscope",
        "model": "qwen-max",
        "display_name": "Qwen Max"
    }))
    .expect("model slot should deserialize");

    assert_eq!(slot.provider_id, "dashscope");
    assert_eq!(slot.model, "qwen-max");
    assert_eq!(slot.extra["display_name"], "Qwen Max");

    let value = serde_json::to_value(slot).expect("model slot should serialize");
    assert_eq!(value["provider_id"], "dashscope");
    assert_eq!(value["model"], "qwen-max");
    assert_eq!(value["display_name"], "Qwen Max");
}

#[test]
fn agent_profile_serializes_absent_default_model_as_explicit_null() {
    let profile = AgentProfileConfig {
        id: "agent-1".into(),
        name: "Agent 1".into(),
        description: String::new(),
        workspace_dir: "/tmp/agent-1".into(),
        template_id: None,
        language: String::new(),
        system_prompt_files: Vec::new(),
        running: AgentsRunningConfig::default(),
        default_model: None,
        channels: None,
        mcp: None,
        heartbeat: None,
        last_dispatch: None,
        tools: None,
        extra: serde_json::Map::new(),
    };

    let value = serde_json::to_value(profile).expect("agent profile should serialize");

    assert!(
        value.get("default_model").is_some_and(|v| v.is_null()),
        "Rust keeps default_model as explicit null; do not copy QwenPaw's omit-when-none shape"
    );
}
