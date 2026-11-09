use std::collections::BTreeMap;
use std::sync::Arc;

use cell_store::FileSystemCellStore;
use mcp_manager::types::{McpConfig, McpServerConfig, McpServerId, McpTransportConfig};
use tempfile::TempDir;

use rua_mcp::AgentMcpCells;

fn make_cells() -> (TempDir, AgentMcpCells) {
    let tmp = TempDir::new().expect("tempdir");
    let workspaces = tmp.path().join("workspaces");
    std::fs::create_dir_all(&workspaces).expect("create workspaces dir");
    let store = Arc::new(FileSystemCellStore::new());
    (tmp, AgentMcpCells::new(store, workspaces))
}

fn sample_mcp_config(server_name: &str) -> McpConfig {
    let mut clients = BTreeMap::new();
    clients.insert(
        McpServerId::new(server_name),
        McpServerConfig {
            name: server_name.to_string(),
            description: format!("{server_name} test server"),
            enabled: true,
            transport: McpTransportConfig::Stdio {
                command: "/bin/echo".to_string(),
                args: vec!["hi".to_string()],
                env: BTreeMap::new(),
                cwd: None,
                use_proot: false,
            },
            auto_connect: Default::default(),
            call_timeout_secs: 60,
            max_concurrency: 4,
            oauth: None,
            disabled_tools: Vec::new(),
            extra: BTreeMap::new(),
        },
    );
    McpConfig { clients }
}

fn ensure_agent_dir(tmp: &TempDir, agent_id: &str) {
    let dir = tmp.path().join("workspaces").join(agent_id);
    std::fs::create_dir_all(&dir).expect("create agent dir");
}

#[tokio::test]
async fn cell_read_returns_none_for_missing_section() {
    let (_tmp, cells) = make_cells();
    let got = cells.cell("agent-1").read().await.expect("read");
    assert!(got.is_none(), "expected None when agent.json absent");
}

#[tokio::test]
async fn cell_write_persists_to_agent_json_mcp_section() {
    let (tmp, cells) = make_cells();
    ensure_agent_dir(&tmp, "agent-2");
    let cfg = sample_mcp_config("my-server");
    cells.cell("agent-2").write(cfg).await.expect("write");

    let path = tmp.path().join("workspaces/agent-2/agent.json");
    let raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read file")).expect("json");

    assert!(
        raw.get("mcp").is_some(),
        "agent.json must have $.mcp section, got {raw:#}"
    );
    assert_eq!(
        raw["mcp"]["clients"]["my-server"]["name"].as_str(),
        Some("my-server"),
        "config must land at $.mcp.clients.my-server"
    );
}

#[tokio::test]
async fn cell_write_does_not_affect_other_sections() {
    let (tmp, cells) = make_cells();
    ensure_agent_dir(&tmp, "agent-3");

    let agent_json = tmp.path().join("workspaces/agent-3/agent.json");
    let initial = serde_json::json!({
        "name": "Test Agent",
        "description": "pre-existing agent",
        "running": {}
    });
    std::fs::write(&agent_json, serde_json::to_string_pretty(&initial).unwrap())
        .expect("seed agent.json");

    cells
        .cell("agent-3")
        .write(sample_mcp_config("srv"))
        .await
        .expect("write mcp");

    let raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&agent_json).expect("read")).expect("json");

    assert_eq!(raw["name"].as_str(), Some("Test Agent"));
    assert_eq!(raw["description"].as_str(), Some("pre-existing agent"));
    assert!(raw.get("mcp").is_some());
}

#[tokio::test]
async fn cell_update_atomic_mutator() {
    let (tmp, cells) = make_cells();
    ensure_agent_dir(&tmp, "agent-4");
    cells
        .cell("agent-4")
        .write(sample_mcp_config("original"))
        .await
        .expect("seed");

    let updated = cells
        .cell("agent-4")
        .update(|current| {
            let mut cfg = current.expect("must see existing");
            cfg.clients.insert(
                McpServerId::new("added"),
                McpServerConfig {
                    name: "added".to_string(),
                    description: String::new(),
                    enabled: true,
                    transport: McpTransportConfig::Stdio {
                        command: "/bin/true".to_string(),
                        args: vec![],
                        env: BTreeMap::new(),
                        cwd: None,
                        use_proot: false,
                    },
                    auto_connect: Default::default(),
                    call_timeout_secs: 30,
                    max_concurrency: 2,
                    oauth: None,
                    disabled_tools: Vec::new(),
                    extra: BTreeMap::new(),
                },
            );
            cfg
        })
        .await
        .expect("update");

    assert_eq!(updated.clients.len(), 2);
    assert!(updated.clients.contains_key(&McpServerId::new("original")));
    assert!(updated.clients.contains_key(&McpServerId::new("added")));

    let on_disk = cells
        .cell("agent-4")
        .read()
        .await
        .expect("read")
        .expect("present");
    assert_eq!(on_disk.clients.len(), 2);
}

#[tokio::test]
async fn cell_delete_removes_section_keeps_file() {
    let (tmp, cells) = make_cells();
    ensure_agent_dir(&tmp, "agent-5");

    let agent_json = tmp.path().join("workspaces/agent-5/agent.json");
    let initial = serde_json::json!({
        "name": "Keep Me",
        "mcp": { "clients": { "x": { "name": "x", "transport": "stdio", "command": "", "args": [], "env": {} } } }
    });
    std::fs::write(&agent_json, serde_json::to_string_pretty(&initial).unwrap()).expect("seed");

    cells.cell("agent-5").delete().await.expect("delete");

    let got = cells.cell("agent-5").read().await.expect("read");
    assert!(got.is_none(), "mcp section should be gone after delete");

    let raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&agent_json).expect("read")).expect("json");
    assert_eq!(
        raw["name"].as_str(),
        Some("Keep Me"),
        "other sections preserved"
    );
    assert!(
        raw.get("mcp").is_none() || raw["mcp"].is_null(),
        "$.mcp must be removed, got {raw:#}"
    );
}

#[tokio::test]
async fn multiple_agents_isolated() {
    let (tmp, cells) = make_cells();
    ensure_agent_dir(&tmp, "alice");
    ensure_agent_dir(&tmp, "bob");

    cells
        .cell("alice")
        .write(sample_mcp_config("alice-srv"))
        .await
        .expect("write alice");
    cells
        .cell("bob")
        .write(sample_mcp_config("bob-srv"))
        .await
        .expect("write bob");

    let alice_cfg = cells
        .cell("alice")
        .read()
        .await
        .expect("read")
        .expect("present");
    let bob_cfg = cells
        .cell("bob")
        .read()
        .await
        .expect("read")
        .expect("present");

    assert!(alice_cfg
        .clients
        .contains_key(&McpServerId::new("alice-srv")));
    assert!(!alice_cfg.clients.contains_key(&McpServerId::new("bob-srv")));
    assert!(bob_cfg.clients.contains_key(&McpServerId::new("bob-srv")));
    assert!(!bob_cfg.clients.contains_key(&McpServerId::new("alice-srv")));

    let alice_json = tmp.path().join("workspaces/alice/agent.json");
    let bob_json = tmp.path().join("workspaces/bob/agent.json");
    assert!(alice_json.exists());
    assert!(bob_json.exists());
}

#[tokio::test]
async fn cell_delete_absent_is_noop() {
    let (_tmp, cells) = make_cells();
    cells
        .cell("nonexistent")
        .delete()
        .await
        .expect("delete absent should succeed");
}

#[tokio::test]
async fn read_value_and_write_value_roundtrip() {
    let (tmp, cells) = make_cells();
    ensure_agent_dir(&tmp, "agent-rv");

    let mcp_json = serde_json::json!({
        "clients": {
            "test-srv": {
                "name": "test-srv",
                "transport": "stdio",
                "command": "/bin/cat",
                "args": [],
                "env": {}
            }
        }
    });

    cells
        .cell("agent-rv")
        .write_value(mcp_json.clone())
        .await
        .expect("write_value");

    let got = cells
        .cell("agent-rv")
        .read_value()
        .await
        .expect("read_value")
        .expect("present");

    assert_eq!(
        got["clients"]["test-srv"]["name"].as_str(),
        Some("test-srv"),
    );
}
