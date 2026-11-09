use rua_config::loader::load_config;

#[test]
fn root_config_errors_are_reported_without_repair_or_backups() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    assert!(load_config(&path, dir.path()).is_err());
    for content in [
        "",
        "[]",
        "{ broken",
        r#"{"task_runtime":{"max_active_tasks_per_chat":"invalid"},"user_timezone":"UTC"}"#,
    ] {
        std::fs::write(&path, content).unwrap();
        let error = load_config(&path, dir.path()).unwrap_err();
        assert!(error.to_string().contains("config.json"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}

#[test]
fn valid_config_preserves_explicit_values_and_extensions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    std::fs::write(&path, r#"{"user_timezone":"UTC","future":{"x":1}}"#).unwrap();
    let config = load_config(&path, dir.path()).unwrap();
    assert_eq!(config.user_timezone.as_deref(), Some("UTC"));
    assert_eq!(config.extra["future"]["x"], 1);
}
