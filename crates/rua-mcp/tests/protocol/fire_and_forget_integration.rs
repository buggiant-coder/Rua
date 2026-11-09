use std::time::{Duration, Instant};

use mcp_manager::manager::McpManager;
use mcp_manager::types::{McpConfig, McpServerConfig, McpServerId, McpTransportConfig};

use rua_mcp::manager::{McpClientStatus, McpManagerImpl};
use rua_mcp::{McpMobilePolicy, PlatformProfile};

fn make_server_config(name: &str) -> McpServerConfig {
    McpServerConfig {
        name: name.to_string(),
        description: String::new(),
        enabled: true,
        transport: McpTransportConfig::StreamableHttp {
            url: format!("http://127.0.0.1:1/{name}"),
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

fn make_config(servers: &[&str]) -> McpConfig {
    let mut config = McpConfig::default();
    for name in servers {
        config
            .clients
            .insert(McpServerId::new(*name), make_server_config(name));
    }
    config
}

#[tokio::test]
async fn load_config_returns_within_5s_with_slow_server() {
    let manager = McpManagerImpl::new(McpMobilePolicy::new(PlatformProfile::Desktop));

    let config = make_config(&["fast-server", "slow-server"]);

    let start = Instant::now();
    let report = manager.load_config(config).await.expect("load_config");
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(5),
        "load_config took {:?}, expected < 5s (fire-and-forget)",
        elapsed
    );
    assert_eq!(report.added.len(), 2);
}

#[tokio::test]
async fn background_task_updates_status_after_connect() {
    let manager = McpManagerImpl::new(McpMobilePolicy::new(PlatformProfile::Desktop));

    let config = make_config(&["test-srv"]);
    manager.load_config(config).await.expect("load_config");

    let servers = manager.servers.read().await;
    let entry = servers.get(&McpServerId::new("test-srv")).expect("entry");
    let status = entry.status.read().await;

    assert!(
        matches!(
            &*status,
            McpClientStatus::Failed(_) | McpClientStatus::Connecting
        ),
        "expected Failed or Connecting, got {:?}",
        *status
    );
}

#[tokio::test]
async fn mcp_client_status_enum_exists_and_serializes() {
    let connecting = McpClientStatus::Connecting;
    let ready = McpClientStatus::Ready;
    let failed = McpClientStatus::Failed("timeout".into());

    let j_connecting = serde_json::to_value(&connecting).unwrap();
    let j_ready = serde_json::to_value(&ready).unwrap();
    let j_failed = serde_json::to_value(&failed).unwrap();

    assert_eq!(j_connecting["status"], "Connecting");
    assert_eq!(j_ready["status"], "Ready");
    assert_eq!(j_failed["status"], "Failed");
    assert_eq!(j_failed["error"], "timeout");
}

#[tokio::test]
async fn list_servers_returns_status_for_all_configured() {
    let manager = McpManagerImpl::new(McpMobilePolicy::new(PlatformProfile::Desktop));

    let config = make_config(&["server-a", "server-b"]);
    manager.load_config(config).await.expect("load_config");

    tokio::time::sleep(Duration::from_millis(500)).await;

    let statuses = manager.list_servers().await.expect("list_servers");
    assert_eq!(statuses.len(), 2);

    let names: Vec<&str> = statuses.iter().map(|s| s.key.as_str()).collect();
    assert!(names.contains(&"server-a"));
    assert!(names.contains(&"server-b"));
}

#[tokio::test]
async fn shutdown_all_removes_server_while_fire_and_forget_retry_is_sleeping() {
    let manager = McpManagerImpl::new(McpMobilePolicy::new(PlatformProfile::Desktop))
        .with_retry_interval(Duration::from_millis(20));

    manager
        .load_config(make_config(&["retrying-server"]))
        .await
        .expect("load_config");

    let server_id = McpServerId::new("retrying-server");
    let start = Instant::now();
    loop {
        {
            let servers = manager.servers.read().await;
            let entry = servers.get(&server_id).expect("entry");
            if matches!(&*entry.status.read().await, McpClientStatus::Failed(_)) {
                break;
            }
        }
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "server should fail first connect quickly before retry sleep"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let report = manager.shutdown_all().await.expect("shutdown_all");
    assert_eq!(report.success, vec![server_id.clone()]);
    assert!(report.errors.is_empty());

    tokio::time::sleep(Duration::from_millis(60)).await;

    let statuses = manager.list_servers().await.expect("list_servers");
    assert!(
        statuses.is_empty(),
        "background retry must not reinsert a server after shutdown_all"
    );
}
