use std::sync::Arc;
use std::time::Duration;

use mcp_manager::manager::McpManager;
use mcp_manager::types::{McpConfig, McpServerConfig, McpServerId, McpTransportConfig};
use rua_mcp::manager::{McpClientStatus, McpManagerImpl};
use rua_mcp::{McpMobilePolicy, PlatformProfile};

fn make_config_with_server(name: &str) -> McpConfig {
    let mut config = McpConfig::default();
    config.clients.insert(
        McpServerId::new(name),
        McpServerConfig {
            name: name.to_string(),
            description: String::new(),
            enabled: true,
            transport: McpTransportConfig::StreamableHttp {
                url: format!("http://127.0.0.1:1/{name}"),
                headers: Default::default(),
                timeout_secs: 5,
            },
            auto_connect: Default::default(),
            call_timeout_secs: 10,
            max_concurrency: 4,
            oauth: None,
            disabled_tools: Vec::new(),
            extra: Default::default(),
        },
    );
    config
}

#[test]
fn detached_multi_thread_runtime_keeps_spawned_tasks_alive() {
    let manager = Arc::new(McpManagerImpl::new(McpMobilePolicy::new(
        PlatformProfile::Desktop,
    )));
    let mgr = Arc::clone(&manager);
    let config = make_config_with_server("test-bootstrap");

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("failed to build runtime");
        rt.block_on(async {
            mgr.load_config(config).await.expect("load_config");
            std::future::pending::<()>().await;
        });
    });

    std::thread::sleep(Duration::from_millis(800));

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let servers = manager.servers.read().await;
        let entry = servers
            .get(&McpServerId::new("test-bootstrap"))
            .expect("server entry should exist");
        let status = entry.status.read().await;

        assert!(
            matches!(&*status, McpClientStatus::Failed(_)),
            "expected Failed (task ran), got {:?} (task may have been aborted)",
            *status
        );
    });
}
