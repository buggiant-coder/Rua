use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn fresh_desktop_root_initializes_configuration_and_publishes_endpoint() {
    let root = tempfile::tempdir().unwrap();
    let endpoint_path = root.path().join("desktop-endpoint.json");
    assert!(!root.path().join("config.json").exists());
    let mut child = Command::new(env!("CARGO_BIN_EXE_rua-server"))
        .args(["--profile", "desktop", "--port", "0", "--rua-root"])
        .arg(root.path())
        .arg("--desktop-endpoint-file")
        .arg(&endpoint_path)
        .env_remove("RUA_CONFIG_FILE")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    let ready = loop {
        if endpoint_path.is_file() {
            break true;
        }
        if child.try_wait().unwrap().is_some() || Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let _ = child.kill();
    child.wait().unwrap();
    assert!(
        ready,
        "fresh Desktop Runtime did not become ready: {}",
        std::fs::read_to_string(root.path().join("logs/startup.log")).unwrap_or_default()
    );
    let endpoint: serde_json::Value =
        serde_json::from_slice(&std::fs::read(endpoint_path).unwrap()).unwrap();
    assert_eq!(endpoint["rua_version"], env!("CARGO_PKG_VERSION"));
    assert!(endpoint["port"].as_u64().unwrap() > 0);
    assert!(root.path().join("config.json").is_file());
}

#[test]
fn invalid_existing_desktop_configuration_is_not_replaced() {
    let root = tempfile::tempdir().unwrap();
    let config_path = root.path().join("config.json");
    std::fs::write(&config_path, b"invalid existing configuration").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_rua-server"))
        .args(["--profile", "desktop", "--port", "0", "--rua-root"])
        .arg(root.path())
        .env_remove("RUA_CONFIG_FILE")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if Instant::now() >= deadline {
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let _ = child.kill();
    child.wait().unwrap();
    assert!(status.is_some_and(|status| !status.success()));
    assert_eq!(
        std::fs::read(config_path).unwrap(),
        b"invalid existing configuration"
    );
}
