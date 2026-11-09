use std::sync::Arc;
use std::time::Duration;

use mcp_manager::manager::McpManager;
use mcp_manager::types::{
    AutoConnectPolicy, McpConfig, McpServerConfig, McpServerId, McpTransportConfig,
};

use rua_mcp::manager::{McpClientStatus, McpManagerImpl};
use rua_mcp::watcher::McpConfigWatcher;
use rua_mcp::{McpMobilePolicy, PlatformProfile};

fn make_server_config(name: &str, port: u16) -> McpServerConfig {
    McpServerConfig {
        name: name.to_string(),
        description: String::new(),
        enabled: true,
        transport: McpTransportConfig::StreamableHttp {
            url: format!("http://127.0.0.1:{port}/{name}"),
            headers: Default::default(),
            timeout_secs: 30,
        },
        auto_connect: Default::default(),
        call_timeout_secs: 60,
        max_concurrency: 4,
        oauth: None,
        disabled_tools: Vec::new(),
        extra: Default::default(),
    }
}

fn make_config_3() -> McpConfig {
    let mut config = McpConfig::default();
    for (name, port) in [("alpha", 9001), ("beta", 9002), ("gamma", 9003)] {
        config
            .clients
            .insert(McpServerId::new(name), make_server_config(name, port));
    }
    config
}

fn make_manual_config_3() -> McpConfig {
    let mut config = make_config_3();
    for server in config.clients.values_mut() {
        server.auto_connect = AutoConnectPolicy::Manual;
    }
    config
}

#[tokio::test]
async fn replace_client_only_affects_target_server() {
    let manager = McpManagerImpl::new(McpMobilePolicy::new(PlatformProfile::Desktop));
    let config = make_config_3();
    manager.load_config(config).await.expect("load_config");

    tokio::time::sleep(Duration::from_millis(200)).await;

    let statuses = manager.list_servers().await.expect("list_servers");
    assert_eq!(statuses.len(), 3);

    let new_beta = make_server_config("beta", 9999);
    manager
        .replace_client(&McpServerId::new("beta"), new_beta)
        .await
        .expect("replace_client");

    tokio::time::sleep(Duration::from_millis(200)).await;

    let statuses = manager.list_servers().await.expect("list_servers");
    assert_eq!(statuses.len(), 3);

    let keys: Vec<&str> = statuses.iter().map(|s| s.key.as_str()).collect();
    assert!(keys.contains(&"alpha"));
    assert!(keys.contains(&"beta"));
    assert!(keys.contains(&"gamma"));

    let servers = manager.servers.read().await;
    let beta_entry = servers.get(&McpServerId::new("beta")).expect("beta exists");
    match &beta_entry.config.transport {
        McpTransportConfig::StreamableHttp { url, .. } => {
            assert!(
                url.contains("9999"),
                "beta URL should contain port 9999, got: {url}"
            );
        }
        other => panic!("expected StreamableHttp, got: {other:?}"),
    }

    let alpha_entry = servers.get(&McpServerId::new("alpha")).expect("alpha");
    match &alpha_entry.config.transport {
        McpTransportConfig::StreamableHttp { url, .. } => {
            assert!(url.contains("9001"), "alpha should still be on 9001: {url}");
        }
        other => panic!("expected StreamableHttp, got: {other:?}"),
    }

    let gamma_entry = servers.get(&McpServerId::new("gamma")).expect("gamma");
    match &gamma_entry.config.transport {
        McpTransportConfig::StreamableHttp { url, .. } => {
            assert!(url.contains("9003"), "gamma should still be on 9003: {url}");
        }
        other => panic!("expected StreamableHttp, got: {other:?}"),
    }
}

#[tokio::test]
async fn replace_client_old_server_entry_is_removed_before_new() {
    let manager = McpManagerImpl::new(McpMobilePolicy::new(PlatformProfile::Desktop));

    let mut config = McpConfig::default();
    config
        .clients
        .insert(McpServerId::new("srv"), make_server_config("srv", 8000));
    manager.load_config(config).await.expect("load_config");
    tokio::time::sleep(Duration::from_millis(200)).await;

    let new_config = make_server_config("srv", 8888);
    manager
        .replace_client(&McpServerId::new("srv"), new_config)
        .await
        .expect("replace_client");

    tokio::time::sleep(Duration::from_millis(200)).await;

    let servers = manager.servers.read().await;
    assert_eq!(servers.len(), 1);

    let entry = servers.get(&McpServerId::new("srv")).expect("srv exists");
    match &entry.config.transport {
        McpTransportConfig::StreamableHttp { url, .. } => {
            assert!(url.contains("8888"), "srv should use new port 8888: {url}");
        }
        other => panic!("expected StreamableHttp, got: {other:?}"),
    }
}

#[tokio::test]
async fn replace_nonexistent_server_still_adds_it() {
    let manager = McpManagerImpl::new(McpMobilePolicy::new(PlatformProfile::Desktop));

    let new_config = make_server_config("new-srv", 7777);
    manager
        .replace_client(&McpServerId::new("new-srv"), new_config)
        .await
        .expect("replace_client on empty manager");

    let servers = manager.servers.read().await;
    assert_eq!(servers.len(), 1);
    assert!(servers.contains_key(&McpServerId::new("new-srv")));
}

#[tokio::test]
async fn reload_removal_preserves_unchanged_servers() {
    let manager = McpManagerImpl::new(McpMobilePolicy::new(PlatformProfile::Desktop));
    manager
        .load_config(make_manual_config_3())
        .await
        .expect("load_config");

    {
        let servers = manager.servers.read().await;
        let alpha = servers.get(&McpServerId::new("alpha")).expect("alpha");
        *alpha.status.write().await = McpClientStatus::Ready;
    }

    let mut next = make_manual_config_3();
    next.clients.remove(&McpServerId::new("beta"));
    let report = manager.reload(next).await.expect("reload without beta");

    assert_eq!(report.removed, vec![McpServerId::new("beta")]);
    assert!(report.updated.is_empty());
    assert_eq!(
        report.unchanged,
        vec![McpServerId::new("alpha"), McpServerId::new("gamma")]
    );

    let servers = manager.servers.read().await;
    assert_eq!(servers.len(), 2);
    assert!(!servers.contains_key(&McpServerId::new("beta")));
    assert!(servers.contains_key(&McpServerId::new("gamma")));

    let alpha = servers.get(&McpServerId::new("alpha")).expect("alpha");
    assert_eq!(
        *alpha.status.read().await,
        McpClientStatus::Ready,
        "unchanged server status must be preserved; reset would indicate a reconnect/re-add"
    );
}

fn write_config(path: &std::path::Path, config: &McpConfig) {
    let json = serde_json::to_string_pretty(config).expect("serialize config");
    std::fs::write(path, json).expect("write config file");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn watcher_triggers_replace_client_on_single_server_change() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let config_path = tmp.path().join("mcp.json");

    let initial = make_config_3();
    write_config(&config_path, &initial);

    let manager = Arc::new(McpManagerImpl::new(McpMobilePolicy::new(
        PlatformProfile::Desktop,
    )));
    manager.load_config(initial).await.expect("load_config");
    tokio::time::sleep(Duration::from_millis(200)).await;

    let watcher = McpConfigWatcher::start_watching(
        vec![config_path.clone()],
        Arc::clone(&manager) as Arc<dyn McpManager>,
    );
    tokio::time::sleep(Duration::from_millis(500)).await;

    let mut updated = make_config_3();
    updated
        .clients
        .insert(McpServerId::new("beta"), make_server_config("beta", 9999));
    write_config(&config_path, &updated);

    tokio::time::sleep(Duration::from_secs(5)).await;

    {
        let servers = manager.servers.read().await;
        assert_eq!(servers.len(), 3, "should still have 3 servers");

        let beta = servers.get(&McpServerId::new("beta")).expect("beta");
        match &beta.config.transport {
            McpTransportConfig::StreamableHttp { url, .. } => {
                assert!(
                    url.contains("9999"),
                    "beta should have new port 9999: {url}"
                );
            }
            other => panic!("expected StreamableHttp, got: {other:?}"),
        }

        let alpha = servers.get(&McpServerId::new("alpha")).expect("alpha");
        match &alpha.config.transport {
            McpTransportConfig::StreamableHttp { url, .. } => {
                assert!(url.contains("9001"), "alpha unchanged: {url}");
            }
            other => panic!("expected StreamableHttp, got: {other:?}"),
        }

        let gamma = servers.get(&McpServerId::new("gamma")).expect("gamma");
        match &gamma.config.transport {
            McpTransportConfig::StreamableHttp { url, .. } => {
                assert!(url.contains("9003"), "gamma unchanged: {url}");
            }
            other => panic!("expected StreamableHttp, got: {other:?}"),
        }
    }

    watcher.stop_watching().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn watcher_replace_disconnects_old_client() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let config_path = tmp.path().join("mcp.json");

    let mut initial = McpConfig::default();
    initial.clients.insert(
        McpServerId::new("target"),
        make_server_config("target", 5000),
    );
    write_config(&config_path, &initial);

    let manager = Arc::new(McpManagerImpl::new(McpMobilePolicy::new(
        PlatformProfile::Desktop,
    )));
    manager.load_config(initial).await.expect("load_config");
    tokio::time::sleep(Duration::from_millis(200)).await;

    let old_config_url = {
        let servers = manager.servers.read().await;
        let entry = servers.get(&McpServerId::new("target")).expect("target");
        match &entry.config.transport {
            McpTransportConfig::StreamableHttp { url, .. } => url.clone(),
            _ => panic!("expected StreamableHttp"),
        }
    };
    assert!(old_config_url.contains("5000"));

    let watcher = McpConfigWatcher::start_watching(
        vec![config_path.clone()],
        Arc::clone(&manager) as Arc<dyn McpManager>,
    );
    tokio::time::sleep(Duration::from_millis(500)).await;

    let mut updated = McpConfig::default();
    updated.clients.insert(
        McpServerId::new("target"),
        make_server_config("target", 6000),
    );
    write_config(&config_path, &updated);

    tokio::time::sleep(Duration::from_secs(5)).await;

    {
        let servers = manager.servers.read().await;
        assert_eq!(servers.len(), 1);
        let entry = servers.get(&McpServerId::new("target")).expect("target");
        match &entry.config.transport {
            McpTransportConfig::StreamableHttp { url, .. } => {
                assert!(
                    url.contains("6000"),
                    "old client (5000) should be disconnected, new client at 6000: {url}"
                );
                assert!(!url.contains("5000"), "old URL should not remain: {url}");
            }
            other => panic!("expected StreamableHttp, got: {other:?}"),
        }
    }

    watcher.stop_watching().await;
}
