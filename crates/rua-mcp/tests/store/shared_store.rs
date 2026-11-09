use rua_mcp::store_dto::{SharedMcpBroadcastRequest, SharedMcpClientCreate, SharedMcpSyncRequest};
use rua_mcp::{secret_field_io, McpClientStore, SharedMcpStore};
use serde_json::json;
use tempfile::TempDir;

const TEST_DEVICE_KEY: [u8; 32] = [9; 32];

fn make_store() -> (TempDir, SharedMcpStore) {
    let tmp = TempDir::new().expect("tempdir");
    let store = SharedMcpStore::new(tmp.path().to_path_buf());
    (tmp, store)
}

fn make_encrypted_store() -> (TempDir, SharedMcpStore) {
    let tmp = TempDir::new().expect("tempdir");
    let store = SharedMcpStore::new(tmp.path().to_path_buf()).with_device_key(TEST_DEVICE_KEY);
    (tmp, store)
}

fn sample_create(key: &str) -> SharedMcpClientCreate {
    SharedMcpClientCreate {
        key: key.to_string(),
        name: format!("{key}-name"),
        enabled: true,
        transport: "stdio".to_string(),
        command: Some("/bin/echo".to_string()),
        args: vec!["hi".to_string()],
        env: Some(json!({"API_KEY": "shared-secret-123456"})),
        url: None,
        headers: Some(json!({"Authorization": "Bearer shared-token-abcdef"})),
        timeout: None,
        auto_start: None,
        oauth: None,
        disabled_tools: Vec::new(),
        extra: serde_json::Map::new(),
    }
}

fn seed_shared_client(data_dir: &std::path::Path, create: &SharedMcpClientCreate) {
    let path = data_dir.join("shared_mcp.json");
    let mut file: serde_json::Value = if path.exists() {
        serde_json::from_slice(&std::fs::read(&path).expect("read shared_mcp.json"))
            .expect("parse shared_mcp.json")
    } else {
        json!({"clients": {}})
    };
    let value = SharedMcpStore::create_to_value(create).expect("create_to_value");
    file["clients"][&create.key] = value;
    std::fs::write(&path, serde_json::to_vec_pretty(&file).expect("serialize"))
        .expect("write shared_mcp.json");
}

fn write_agent_json(data_dir: &std::path::Path, agent_id: &str, json: serde_json::Value) {
    let dir = data_dir.join("workspaces").join(agent_id);
    std::fs::create_dir_all(&dir).expect("mkdir agent dir");
    std::fs::write(dir.join("agent.json"), serde_json::to_vec(&json).unwrap())
        .expect("write agent.json");
}

fn read_json(path: &std::path::Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(path).expect("read json")).expect("parse json")
}

#[tokio::test]
async fn broadcast_to_explicit_agents_writes_into_agent_json() {
    let (_tmp, store) = make_store();
    seed_shared_client(store.data_dir(), &sample_create("broadcast-me"));

    write_agent_json(
        store.data_dir(),
        "default",
        json!({"id": "default", "name": "default"}),
    );

    let resp = store
        .broadcast(SharedMcpBroadcastRequest {
            agent_ids: vec!["default".into()],
            client_keys: vec!["broadcast-me".into()],
            extra: serde_json::Map::new(),
        })
        .await
        .expect("broadcast");

    assert!(resp.results.iter().any(|r| r.success), "at least one OK");

    let path = store
        .data_dir()
        .join("workspaces")
        .join("default")
        .join("agent.json");
    let raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read agent.json")).unwrap();
    assert_eq!(
        raw["mcp"]["clients"]["broadcast-me"]["name"]
            .as_str()
            .unwrap(),
        "broadcast-me-name",
        "broadcast must populate mcp.clients on default"
    );
}

#[tokio::test]
async fn encrypted_broadcast_writes_agent_copy_encrypted_but_decryptable() {
    let (_tmp, store) = make_encrypted_store();
    seed_shared_client(store.data_dir(), &sample_create("broadcast-secret"));
    write_agent_json(
        store.data_dir(),
        "default",
        json!({"id": "default", "name": "default"}),
    );

    store
        .broadcast(SharedMcpBroadcastRequest {
            agent_ids: vec!["default".into()],
            client_keys: vec!["broadcast-secret".into()],
            extra: serde_json::Map::new(),
        })
        .await
        .expect("broadcast");

    let path = store
        .data_dir()
        .join("workspaces")
        .join("default")
        .join("agent.json");
    let raw = read_json(&path);
    let mcp = &raw["mcp"];
    let text = serde_json::to_string(mcp).unwrap();
    assert!(secret_field_io::contains_encrypted_fields(mcp));
    assert!(!text.contains("shared-secret-123456"));
    assert!(!text.contains("shared-token-abcdef"));

    let cfg = McpClientStore::decrypt_mcp_config(mcp, Some(&TEST_DEVICE_KEY))
        .expect("agent mcp decrypts for execution");
    let server = cfg
        .clients
        .get(&mcp_manager::types::McpServerId::new("broadcast-secret"))
        .expect("broadcasted server");
    match &server.transport {
        mcp_manager::types::McpTransportConfig::Stdio { env, .. } => {
            assert_eq!(
                env.get("API_KEY").map(|s| s.as_str()),
                Some("shared-secret-123456")
            );
        }
        other => panic!("expected stdio config, got {other:?}"),
    }
}

#[tokio::test]
async fn broadcast_with_conflict_mode_skip_does_not_overwrite() {
    let (_tmp, store) = make_store();
    seed_shared_client(store.data_dir(), &sample_create("client-1"));

    write_agent_json(
        store.data_dir(),
        "default",
        json!({
            "id": "default",
            "name": "default",
            "mcp": {"clients": {"client-1": {"name": "agent-original", "command": "/bin/cat"}}}
        }),
    );

    let mut req = SharedMcpBroadcastRequest {
        agent_ids: vec!["default".into()],
        client_keys: vec!["client-1".into()],
        extra: serde_json::Map::new(),
    };
    req.extra.insert("conflict_mode".into(), json!("skip"));

    let resp = store.broadcast(req).await.expect("broadcast");
    let agent_result = &resp.results[0];
    assert!(agent_result.success);
    assert_eq!(agent_result.error.as_deref(), Some("skipped"));

    let path = store
        .data_dir()
        .join("workspaces")
        .join("default")
        .join("agent.json");
    let raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read")).unwrap();
    assert_eq!(
        raw["mcp"]["clients"]["client-1"]["name"].as_str().unwrap(),
        "agent-original"
    );
}

#[tokio::test]
async fn sync_from_agent_pulls_into_shared_catalog() {
    let (_tmp, store) = make_store();

    write_agent_json(
        store.data_dir(),
        "coder",
        json!({
            "id": "coder",
            "name": "coder",
            "mcp": {"clients": {"my-tool": {"name": "my-tool", "command": "/bin/true"}}}
        }),
    );

    store
        .sync_from_agent(SharedMcpSyncRequest {
            agent_id: "coder".into(),
            client_keys: Vec::new(),
            extra: serde_json::Map::new(),
        })
        .await
        .expect("sync");

    let path = store.data_dir().join("shared_mcp.json");
    let raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read shared_mcp.json")).unwrap();
    assert_eq!(
        raw["clients"]["my-tool"]["name"].as_str(),
        Some("my-tool"),
        "sync_from_agent must populate shared catalog, got {raw:#}"
    );
}

#[tokio::test]
async fn encrypted_sync_from_agent_stores_shared_copy_encrypted() {
    let (_tmp, store) = make_encrypted_store();
    let plaintext_mcp = json!({
        "clients": {
            "synced-secret": {
                "name": "synced-secret",
                "transport": "stdio",
                "command": "/bin/true",
                "args": [],
                "env": {"API_KEY": "agent-secret-987654"}
            }
        }
    });
    let encrypted_mcp = secret_field_io::encrypt_client_value(
        &plaintext_mcp,
        Some(&TEST_DEVICE_KEY),
        "agent:coder:mcp",
    )
    .expect("encrypt agent mcp");
    write_agent_json(
        store.data_dir(),
        "coder",
        json!({
            "id": "coder",
            "name": "coder",
            "mcp": encrypted_mcp
        }),
    );

    store
        .sync_from_agent(SharedMcpSyncRequest {
            agent_id: "coder".into(),
            client_keys: Vec::new(),
            extra: serde_json::Map::new(),
        })
        .await
        .expect("sync");

    let path = store.data_dir().join("shared_mcp.json");
    let raw = read_json(&path);
    let text = serde_json::to_string(&raw).unwrap();
    assert!(secret_field_io::contains_encrypted_fields(&raw));
    assert!(!text.contains("agent-secret-987654"));

    let decrypted = secret_field_io::decrypt_client_value(&raw, Some(&TEST_DEVICE_KEY))
        .expect("decrypt shared");
    assert_eq!(
        decrypted["clients"]["synced-secret"]["env"]["API_KEY"].as_str(),
        Some("agent-secret-987654")
    );
}

#[tokio::test]
async fn plaintext_agent_sync_migrates_shared_copy_once_without_churn() {
    let (_tmp, store) = make_encrypted_store();
    write_agent_json(
        store.data_dir(),
        "legacy-agent",
        json!({
            "id": "legacy-agent",
            "name": "legacy-agent",
            "mcp": {
                "clients": {
                    "legacy-shared": {
                        "name": "legacy-shared",
                        "transport": "stdio",
                        "command": "/bin/true",
                        "args": [],
                        "env": {"API_KEY": "legacy-agent-secret-13579"}
                    }
                }
            }
        }),
    );

    let req = SharedMcpSyncRequest {
        agent_id: "legacy-agent".into(),
        client_keys: Vec::new(),
        extra: serde_json::Map::new(),
    };
    store
        .sync_from_agent(req.clone())
        .await
        .expect("first sync");

    let path = store.data_dir().join("shared_mcp.json");
    let first = std::fs::read(&path).expect("read first shared_mcp.json");
    let raw: serde_json::Value = serde_json::from_slice(&first).expect("parse first");
    let text = serde_json::to_string(&raw).unwrap();
    assert!(secret_field_io::contains_encrypted_fields(&raw));
    assert!(
        !text.contains("legacy-agent-secret-13579"),
        "plaintext legacy secret must be encrypted after sync migration: {raw:#}"
    );

    store.sync_from_agent(req).await.expect("second sync");
    let second = std::fs::read(&path).expect("read second shared_mcp.json");
    assert_eq!(
        first, second,
        "same logical shared MCP sync must not re-encrypt or rewrite unchanged file"
    );
}
