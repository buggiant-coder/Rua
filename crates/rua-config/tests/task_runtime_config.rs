use rua_config::types::RootConfig;

#[test]
fn task_runtime_defaults_are_finite_and_round_trip_unknown_fields() {
    let config: RootConfig = serde_json::from_value(serde_json::json!({
        "task_runtime": {
            "max_concurrent_tasks_per_chat": 2,
            "future_limit": 17
        }
    }))
    .unwrap();

    assert_eq!(config.task_runtime.max_active_tasks_per_chat, 128);
    assert_eq!(config.task_runtime.max_concurrent_tasks_per_chat, 2);
    assert_eq!(config.task_runtime.max_concurrent_tasks_per_runner, 8);
    assert_eq!(config.task_runtime.activity_channel_capacity, 512);
    assert_eq!(config.task_runtime.max_result_output_chars, 16_000);
    assert_eq!(config.task_runtime.max_raw_result_bytes, 1024 * 1024);
    assert_eq!(config.task_runtime.max_attempt_history, 20);
    assert_eq!(config.task_runtime.max_dependencies_per_task, 16);
    assert_eq!(config.task_runtime.max_dependency_depth, 16);
    assert_eq!(config.task_runtime.cancellation_grace_secs, 5);
    assert_eq!(config.task_runtime.extra["future_limit"], 17);

    let round_trip = serde_json::to_value(config).unwrap();
    assert_eq!(round_trip["task_runtime"]["future_limit"], 17);
}

#[test]
fn null_task_runtime_uses_defaults() {
    let config: RootConfig =
        serde_json::from_value(serde_json::json!({"task_runtime": null})).unwrap();
    assert_eq!(config.task_runtime.max_concurrent_tasks_per_chat, 4);
}
