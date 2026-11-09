use std::process::Command;

fn rua_bin() -> String {
    env!("CARGO_BIN_EXE_rua").to_string()
}

#[test]
fn init_tracing_creates_log_file_on_any_subcommand() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path();

    let status = Command::new(rua_bin())
        .args([
            "--rua-root",
            workspace.to_str().unwrap(),
            "clean",
            "--dry-run",
        ])
        .env("RUST_LOG", "rua=info")
        .status()
        .expect("failed to run rua");

    assert!(status.success(), "rua clean --dry-run should succeed");
    assert!(
        !workspace.join("config.json").exists(),
        "logging must not initialize application configuration"
    );

    let log_dir = workspace.join("logs");
    assert!(log_dir.exists(), "logs/ directory should be created");

    let log_files: Vec<_> = std::fs::read_dir(&log_dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("rua.log"))
        .collect();

    assert!(
        !log_files.is_empty(),
        "Expected rua.log.* file in {}, found none",
        log_dir.display()
    );
}

#[test]
fn workspace_flag_writes_logs_to_specified_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("custom-workspace");

    let status = Command::new(rua_bin())
        .args([
            "--rua-root",
            workspace.to_str().unwrap(),
            "clean",
            "--dry-run",
        ])
        .env("RUST_LOG", "rua=info")
        .status()
        .expect("failed to run rua");

    assert!(status.success());

    let log_dir = workspace.join("logs");
    assert!(
        log_dir.exists(),
        "logs/ should be created under custom workspace: {}",
        log_dir.display()
    );

    let log_files: Vec<_> = std::fs::read_dir(&log_dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("rua.log"))
        .collect();

    assert!(
        !log_files.is_empty(),
        "Expected log files in custom workspace"
    );
}

#[test]
fn invalid_existing_configuration_is_not_replaced_by_logging_defaults() {
    let temp = tempfile::tempdir().unwrap();
    let config_path = temp.path().join("config.json");
    std::fs::write(&config_path, "invalid JSON").unwrap();

    let output = Command::new(rua_bin())
        .args([
            "--rua-root",
            temp.path().to_str().unwrap(),
            "clean",
            "--dry-run",
        ])
        .output()
        .expect("failed to run rua");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("parse configuration"));
    assert_eq!(
        std::fs::read_to_string(config_path).unwrap(),
        "invalid JSON"
    );
}
