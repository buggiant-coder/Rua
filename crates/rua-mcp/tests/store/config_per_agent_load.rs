use std::sync::Arc;

use mcp_manager::manager::McpManager;
use mcp_manager::types::{McpServerId, McpTransportConfig};
use rua_mcp::manager::McpManagerImpl;
use rua_mcp::{McpConfigLoader, McpMobilePolicy, PlatformProfile};

fn agent_json_with_fs_server(command: &str) -> serde_json::Value {
    serde_json::json!({
        "mcp": {
            "clients": {
                "fs": {
                    "name": "fs",
                    "transport": "stdio",
                    "command": command,
                    "args": ["--dummy"]
                }
            }
        }
    })
}

#[test]
fn per_agent_lazy_load_same_id_independent_managers() {
    let agent_a = agent_json_with_fs_server("cat");
    let agent_b = agent_json_with_fs_server("head");

    let mcp_val_a = agent_a.get("mcp").unwrap().clone();
    let mcp_val_b = agent_b.get("mcp").unwrap().clone();

    let config_a = McpConfigLoader::load_from_value(mcp_val_a).expect("parse config A");
    let config_b = McpConfigLoader::load_from_value(mcp_val_b).expect("parse config B");

    assert!(config_a.clients.contains_key(&McpServerId::new("fs")));
    assert!(config_b.clients.contains_key(&McpServerId::new("fs")));

    let cmd_a = match &config_a.clients[&McpServerId::new("fs")].transport {
        McpTransportConfig::Stdio { command, .. } => command.clone(),
        _ => panic!("expected stdio"),
    };
    let cmd_b = match &config_b.clients[&McpServerId::new("fs")].transport {
        McpTransportConfig::Stdio { command, .. } => command.clone(),
        _ => panic!("expected stdio"),
    };
    assert_ne!(cmd_a, cmd_b, "commands should differ between agents");

    let mgr_a = Arc::new(McpManagerImpl::new(McpMobilePolicy::new(
        PlatformProfile::Desktop,
    )));
    let mgr_b = Arc::new(McpManagerImpl::new(McpMobilePolicy::new(
        PlatformProfile::Desktop,
    )));

    let ma = Arc::clone(&mgr_a);
    let mb = Arc::clone(&mgr_b);

    let handle_a = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async { ma.load_config(config_a).await.expect("load A") })
    });
    let handle_b = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async { mb.load_config(config_b).await.expect("load B") })
    });

    let report_a = handle_a.join().unwrap();
    let report_b = handle_b.join().unwrap();

    assert!(report_a.errors.is_empty(), "{:?}", report_a.errors);
    assert!(report_b.errors.is_empty(), "{:?}", report_b.errors);
    assert!(
        report_a.added.contains(&McpServerId::new("fs")),
        "manager A should have added 'fs'"
    );
    assert!(
        report_b.added.contains(&McpServerId::new("fs")),
        "manager B should have added 'fs'"
    );

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let servers_a = mgr_a.servers.read().await;
        let servers_b = mgr_b.servers.read().await;

        let entry_a = servers_a.get(&McpServerId::new("fs")).expect("A has 'fs'");
        let entry_b = servers_b.get(&McpServerId::new("fs")).expect("B has 'fs'");

        match (&entry_a.config.transport, &entry_b.config.transport) {
            (
                McpTransportConfig::Stdio { command: ca, .. },
                McpTransportConfig::Stdio { command: cb, .. },
            ) => {
                assert_ne!(
                    ca, cb,
                    "per-agent managers must have different commands for same ID"
                );
            }
            _ => panic!("expected stdio transports"),
        }
    });
}
