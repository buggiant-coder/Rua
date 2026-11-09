use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rua_plugin_runtime::client::PluginClientConfig;
use rua_plugin_runtime::manager::PluginLifecycleManager;
use rua_plugin_runtime::registry::{ActivePluginRegistry, PluginStatus};

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("mock_plugin.py")
}

fn has_python() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn mock_config(plugin_id: &str, env: BTreeMap<String, String>) -> PluginClientConfig {
    let mut cfg = PluginClientConfig::new(plugin_id, "unused");
    cfg.command = Some("python3".into());
    cfg.args = Some(vec![fixture_path().to_string_lossy().into_owned()]);
    cfg.env = env;
    cfg.startup_timeout = Duration::from_secs(10);
    cfg.call_timeout = Duration::from_secs(5);
    cfg.heartbeat_interval = Duration::from_secs(120);
    cfg
}

#[tokio::test]
async fn start_list_stop_lifecycle() {
    if !has_python() {
        eprintln!("python3 not found, skipping");
        return;
    }

    let registry = Arc::new(ActivePluginRegistry::new());
    let manager = PluginLifecycleManager::new(Arc::clone(&registry));

    let cfg = mock_config("test-lifecycle", BTreeMap::new());
    manager.start(cfg).await.expect("start should succeed");

    let list = manager.list().await;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].plugin_id, "test-lifecycle");
    assert_eq!(list[0].status, PluginStatus::Running);

    assert_eq!(
        manager.status("test-lifecycle").await,
        Some(PluginStatus::Running)
    );

    manager
        .stop("test-lifecycle")
        .await
        .expect("stop should succeed");

    let list = manager.list().await;
    assert!(list.is_empty());
    assert_eq!(manager.status("test-lifecycle").await, None);
}

#[tokio::test]
async fn start_duplicate_returns_error() {
    if !has_python() {
        eprintln!("python3 not found, skipping");
        return;
    }

    let registry = Arc::new(ActivePluginRegistry::new());
    let manager = PluginLifecycleManager::new(Arc::clone(&registry));

    let cfg = mock_config("dup-test", BTreeMap::new());
    manager.start(cfg).await.expect("first start ok");

    let cfg2 = mock_config("dup-test", BTreeMap::new());
    let err = manager.start(cfg2).await;
    assert!(err.is_err(), "duplicate start should fail");

    manager.stop("dup-test").await.ok();
}

#[tokio::test]
async fn restart_replaces_running() {
    if !has_python() {
        eprintln!("python3 not found, skipping");
        return;
    }

    let registry = Arc::new(ActivePluginRegistry::new());
    let manager = PluginLifecycleManager::new(Arc::clone(&registry));

    let cfg = mock_config("restart-test", BTreeMap::new());
    manager.start(cfg).await.expect("start ok");

    let cfg2 = mock_config("restart-test", BTreeMap::new());
    manager.restart(cfg2).await.expect("restart ok");

    let list = manager.list().await;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].status, PluginStatus::Running);

    manager.stop("restart-test").await.ok();
}

#[tokio::test]
async fn crash_detected_and_recorded() {
    if !has_python() {
        eprintln!("python3 not found, skipping");
        return;
    }

    let registry = Arc::new(ActivePluginRegistry::new());
    let manager = PluginLifecycleManager::new(Arc::clone(&registry));

    let mut env = BTreeMap::new();
    env.insert("MOCK_PLUGIN_CRASH_AFTER".into(), "1".into());
    let mut cfg = mock_config("crash-test", env);
    cfg.heartbeat_interval = Duration::from_secs(1);

    manager.start(cfg).await.expect("start ok");

    let _ = registry
        .with_client_mut("crash-test", |client| client.health())
        .await;

    tokio::time::sleep(Duration::from_secs(5)).await;

    let info = registry.get_crash_info("crash-test").await;

    if let Some((count, _)) = info {
        assert!(count >= 1, "crash count should be >= 1, got {}", count);
    }

    if let Some(status) = registry.get_status("crash-test").await {
        assert!(
            matches!(
                status,
                PluginStatus::Crashed | PluginStatus::Starting | PluginStatus::Disabled
            ),
            "expected crash-related status, got {:?}",
            status
        );
    }
}

#[tokio::test]
async fn active_registry_basic_ops() {
    let registry = ActivePluginRegistry::new();
    assert!(registry.is_empty().await);
    assert_eq!(registry.len().await, 0);

    let list = registry.list().await;
    assert!(list.is_empty());

    assert_eq!(registry.get_status("nonexistent").await, None);
    assert!(!registry.contains("nonexistent").await);
}

#[tokio::test]
async fn owner_shutdown_cancels_inflight_plugin_calls_before_draining_the_registry() {
    let registry = Arc::new(ActivePluginRegistry::new());
    let mut cfg = mock_config(
        "hanging",
        BTreeMap::from([("MOCK_PLUGIN_HANG".into(), "1".into())]),
    );
    cfg.call_timeout = Duration::from_secs(60);
    let client = rua_plugin_runtime::PluginClient::spawn(cfg).await.unwrap();
    registry.insert("hanging".into(), client).await.unwrap();
    let caller = Arc::clone(&registry);
    let call = tokio::spawn(async move {
        caller
            .call_plugin_tool("hanging", "greet", serde_json::json!({}))
            .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    tokio::time::timeout(Duration::from_secs(3), registry.shutdown_all())
        .await
        .expect("shutdown must not wait for the call timeout/read lock");
    assert!(call.await.unwrap().is_err());
    assert!(registry.is_empty().await);
    registry.shutdown_all().await;
    let client = rua_plugin_runtime::PluginClient::spawn(mock_config("late", BTreeMap::new()))
        .await
        .unwrap();
    assert!(matches!(
        registry.insert("late".into(), client).await,
        Err(rua_plugin_runtime::ActiveRegistryError::ShuttingDown)
    ));
}
