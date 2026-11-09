use std::sync::Arc;

use cell_store::FileSystemCellStore;
use mcp_manager::types::McpOAuthConfig;
use rua_mcp::secret_field_io;
use rua_mcp::secret_mask;
use rua_mcp::store_dto::{McpClientCreate, McpClientUpdate, McpServerConfig, McpTransportConfig};
use rua_mcp::{AgentMcpCells, McpClientStore};
use serde_json::json;
use tempfile::TempDir;

const TEST_DEVICE_KEY: [u8; 32] = [7; 32];

fn make_store() -> (TempDir, McpClientStore) {
    let tmp = TempDir::new().expect("tempdir");
    let store = McpClientStore::new(tmp.path().to_path_buf());
    (tmp, store)
}

fn make_store_with_cells() -> (TempDir, Arc<AgentMcpCells>, McpClientStore) {
    let tmp = TempDir::new().expect("tempdir");
    let workspaces = tmp.path().join("workspaces");
    std::fs::create_dir_all(&workspaces).expect("create workspaces dir");
    let cells = Arc::new(AgentMcpCells::new(
        Arc::new(FileSystemCellStore::new()),
        workspaces,
    ));
    let store = McpClientStore::with_agent_mcp_cells(tmp.path().to_path_buf(), Arc::clone(&cells));
    (tmp, cells, store)
}

fn make_encrypted_store() -> (TempDir, McpClientStore) {
    let tmp = TempDir::new().expect("tempdir");
    let store = McpClientStore::new(tmp.path().to_path_buf()).with_device_key(TEST_DEVICE_KEY);
    (tmp, store)
}

fn make_encrypted_store_with_cells() -> (TempDir, Arc<AgentMcpCells>, McpClientStore) {
    let tmp = TempDir::new().expect("tempdir");
    let workspaces = tmp.path().join("workspaces");
    std::fs::create_dir_all(&workspaces).expect("create workspaces dir");
    let cells = Arc::new(AgentMcpCells::new(
        Arc::new(FileSystemCellStore::new()),
        workspaces,
    ));
    let store = McpClientStore::with_agent_mcp_cells(tmp.path().to_path_buf(), Arc::clone(&cells))
        .with_device_key(TEST_DEVICE_KEY);
    (tmp, cells, store)
}

fn read_agent_json(data_dir: &std::path::Path, agent_id: &str) -> serde_json::Value {
    let path = data_dir
        .join("workspaces")
        .join(agent_id)
        .join("agent.json");
    let text = std::fs::read_to_string(&path).expect("agent.json should exist");
    serde_json::from_str(&text).expect("agent.json should be valid JSON")
}

fn sample_create(key: &str) -> McpClientCreate {
    McpClientCreate {
        key: key.to_string(),
        name: format!("{key}-name"),
        enabled: true,
        transport: "stdio".to_string(),
        command: Some("/bin/echo".to_string()),
        args: vec!["hello".to_string()],
        env: Some(json!({"API_KEY": "sk-supersecretvalue1234"})),
        url: None,
        headers: Some(json!({"Authorization": "Bearer toplevelsecret-9999"})),
        timeout: None,
        auto_start: None,
        oauth: None,
        disabled_tools: Vec::new(),
        extra: serde_json::Map::new(),
    }
}

fn get_env_value<'a>(
    client: &'a rua_mcp::store_dto::McpClientConfig,
    env_key: &str,
) -> Option<&'a str> {
    match &client.server.transport {
        McpTransportConfig::Stdio { env, .. } => env.get(env_key).map(|s| s.as_str()),
        _ => None,
    }
}

fn get_extra_header<'a>(
    client: &'a rua_mcp::store_dto::McpClientConfig,
    header_key: &str,
) -> Option<&'a str> {
    client
        .server
        .extra
        .get("headers")
        .and_then(|h| h.get(header_key))
        .and_then(|v| v.as_str())
}

#[tokio::test]
async fn create_then_list_round_trip_persists_and_masks() {
    let (_tmp, store) = make_store();

    let created = store
        .create_client(Some("default"), sample_create("foo"))
        .await
        .expect("create");

    assert_eq!(created.key, "foo");

    let env_val = get_env_value(&created, "API_KEY").expect("API_KEY present");
    assert!(
        !env_val.contains("supersecret"),
        "create response must mask env: {env_val}"
    );

    let listed = store.list_clients(Some("default")).await.expect("list");
    assert_eq!(listed.len(), 1);
    let header_val = get_extra_header(&listed[0], "Authorization").expect("Authorization present");
    assert!(
        !header_val.contains("toplevelsecret"),
        "list response must mask headers: {header_val}"
    );

    let raw = read_agent_json(store.data_dir(), "default");
    let on_disk_env = raw["mcp"]["clients"]["foo"]["env"]["API_KEY"]
        .as_str()
        .expect("on-disk env still string");
    assert_eq!(
        on_disk_env, "sk-supersecretvalue1234",
        "on-disk env value must NOT be masked"
    );
}

#[tokio::test]
async fn cell_backed_store_writes_the_same_mcp_section_read_by_reload() {
    let (_tmp, cells, store) = make_store_with_cells();

    store
        .create_client(Some("default"), sample_create("cell-tool"))
        .await
        .expect("create via cell-backed store");
    let cfg = cells
        .cell("default")
        .read()
        .await
        .expect("cell read")
        .expect("mcp section");
    assert!(cfg
        .clients
        .contains_key(&mcp_manager::types::McpServerId::new("cell-tool")));

    store
        .delete_client(Some("default"), "cell-tool")
        .await
        .expect("delete via cell-backed store");
    let cfg = cells
        .cell("default")
        .read()
        .await
        .expect("cell read")
        .expect("mcp section remains");
    assert!(
        cfg.clients.is_empty(),
        "delete must update the same cell that reload_from_disk reads"
    );
}

#[tokio::test]
async fn get_client_for_execution_returns_unmasked_secrets() {
    let (_tmp, store) = make_store();
    store
        .create_client(Some("default"), sample_create("bar"))
        .await
        .expect("create");

    let exec = store
        .get_client_for_execution(Some("default"), "bar")
        .await
        .expect("exec lookup")
        .expect("exec value");

    let env_val = get_env_value(&exec, "API_KEY").expect("API_KEY present");
    assert_eq!(
        env_val, "sk-supersecretvalue1234",
        "execution lookup must NOT mask env"
    );
}

#[tokio::test]
async fn encrypted_store_writes_envelopes_masks_api_and_decrypts_execution() {
    let (_tmp, store) = make_encrypted_store();
    store
        .create_client(Some("default"), sample_create("encrypted"))
        .await
        .expect("create");

    let raw = read_agent_json(store.data_dir(), "default");
    let mcp = &raw["mcp"];
    assert!(
        secret_field_io::contains_encrypted_fields(mcp),
        "on-disk mcp section must contain encrypted field envelopes: {mcp:#}"
    );
    assert!(
        !serde_json::to_string(mcp)
            .unwrap()
            .contains("sk-supersecretvalue1234"),
        "on-disk mcp section must not contain plaintext env secret: {mcp:#}"
    );
    assert!(
        !serde_json::to_string(mcp)
            .unwrap()
            .contains("toplevelsecret-9999"),
        "on-disk mcp section must not contain plaintext header secret: {mcp:#}"
    );

    let listed = store.list_clients(Some("default")).await.expect("list");
    let listed_env = get_env_value(&listed[0], "API_KEY").expect("masked env");
    assert!(secret_mask::is_masked_value(listed_env));
    assert!(!listed_env.contains("supersecret"));
    let listed_json = serde_json::to_string(&listed).unwrap();
    assert!(!listed_json.contains("rua_encrypted"));
    assert!(!listed_json.contains("supersecret"));

    let exec = store
        .get_client_for_execution(Some("default"), "encrypted")
        .await
        .expect("exec lookup")
        .expect("exec value");
    assert_eq!(
        get_env_value(&exec, "API_KEY"),
        Some("sk-supersecretvalue1234")
    );
    assert_eq!(
        get_extra_header(&exec, "Authorization"),
        Some("Bearer toplevelsecret-9999")
    );
}

#[tokio::test]
async fn cell_backed_encrypted_store_decrypts_raw_value_for_reload_boundary() {
    let (_tmp, cells, store) = make_encrypted_store_with_cells();
    store
        .create_client(Some("default"), sample_create("cell-encrypted"))
        .await
        .expect("create via cell-backed encrypted store");

    let raw_mcp = cells
        .cell("default")
        .read_value()
        .await
        .expect("cell read")
        .expect("mcp section");
    assert!(secret_field_io::contains_encrypted_fields(&raw_mcp));

    let cfg = McpClientStore::decrypt_mcp_config(&raw_mcp, Some(&TEST_DEVICE_KEY))
        .expect("reload boundary decrypts into typed config");
    let server = cfg
        .clients
        .get(&mcp_manager::types::McpServerId::new("cell-encrypted"))
        .expect("server");
    match &server.transport {
        McpTransportConfig::Stdio { env, .. } => {
            assert_eq!(
                env.get("API_KEY").map(|s| s.as_str()),
                Some("sk-supersecretvalue1234")
            );
        }
        other => panic!("expected Stdio, got {other:?}"),
    }
}

#[tokio::test]
async fn update_with_masked_value_preserves_original() {
    let (_tmp, store) = make_store();
    store
        .create_client(Some("default"), sample_create("baz"))
        .await
        .expect("create");

    let masked = store
        .get_client(Some("default"), "baz")
        .await
        .expect("get")
        .expect("present");

    let masked_json = serde_json::to_value(&masked.server).unwrap();
    let masked_env = masked_json.get("env").cloned();

    let update = McpClientUpdate {
        name: Some("baz-renamed".into()),
        env: masked_env,
        ..Default::default()
    };
    let updated = store
        .update_client(Some("default"), "baz", update)
        .await
        .expect("update");
    assert_eq!(updated.server.name, "baz-renamed");

    let raw = read_agent_json(store.data_dir(), "default");
    let on_disk_env = raw["mcp"]["clients"]["baz"]["env"]["API_KEY"]
        .as_str()
        .expect("on-disk env still string");
    assert_eq!(
        on_disk_env, "sk-supersecretvalue1234",
        "masked echo-back must not overwrite real secret"
    );
}

#[tokio::test]
async fn encrypted_store_masked_echo_back_preserves_existing_secret() {
    let (_tmp, store) = make_encrypted_store();
    store
        .create_client(Some("default"), sample_create("masked-encrypted"))
        .await
        .expect("create");

    let masked = store
        .get_client(Some("default"), "masked-encrypted")
        .await
        .expect("get")
        .expect("present");
    let masked_json = serde_json::to_value(&masked.server).unwrap();
    let update = McpClientUpdate {
        name: Some("masked-encrypted-renamed".into()),
        env: masked_json.get("env").cloned(),
        headers: masked_json.get("headers").cloned(),
        ..Default::default()
    };

    store
        .update_client(Some("default"), "masked-encrypted", update)
        .await
        .expect("update");

    let raw = read_agent_json(store.data_dir(), "default");
    let mcp = &raw["mcp"];
    assert!(
        !serde_json::to_string(mcp).unwrap().contains("***"),
        "masked placeholders must not be written to disk: {mcp:#}"
    );

    let exec = store
        .get_client_for_execution(Some("default"), "masked-encrypted")
        .await
        .expect("exec lookup")
        .expect("exec value");
    assert_eq!(
        get_env_value(&exec, "API_KEY"),
        Some("sk-supersecretvalue1234")
    );
}

#[tokio::test]
async fn encrypted_store_set_oauth_encrypts_tokens_and_decrypts_execution() {
    let (_tmp, store) = make_encrypted_store();
    store
        .create_client(Some("default"), sample_create("oauth-encrypted"))
        .await
        .expect("create");

    store
        .set_oauth(
            Some("default"),
            "oauth-encrypted",
            Some(McpOAuthConfig {
                client_id: "client-1".into(),
                scope: "read".into(),
                access_token: "access-secret-123".into(),
                refresh_token: "refresh-secret-456".into(),
                expires_at: 1234.0,
                token_endpoint: "https://example.test/token".into(),
                auth_endpoint: "https://example.test/auth".into(),
                extra: Default::default(),
            }),
        )
        .await
        .expect("set oauth");

    let raw = read_agent_json(store.data_dir(), "default");
    let mcp = &raw["mcp"];
    let raw_text = serde_json::to_string(mcp).unwrap();
    assert!(!raw_text.contains("access-secret-123"));
    assert!(!raw_text.contains("refresh-secret-456"));
    assert!(secret_field_io::contains_encrypted_fields(mcp));

    let api = store
        .get_client(Some("default"), "oauth-encrypted")
        .await
        .expect("get")
        .expect("api value");
    let oauth = api.server.oauth.expect("oauth api value");
    assert!(secret_mask::is_masked_value(&oauth.access_token));
    assert!(secret_mask::is_masked_value(&oauth.refresh_token));
    assert!(!oauth.access_token.contains("secret"));
    assert!(!oauth.refresh_token.contains("secret"));

    let exec = store
        .get_client_for_execution(Some("default"), "oauth-encrypted")
        .await
        .expect("exec lookup")
        .expect("exec value");
    let oauth = exec.server.oauth.expect("oauth exec value");
    assert_eq!(oauth.access_token, "access-secret-123");
    assert_eq!(oauth.refresh_token, "refresh-secret-456");
}

#[tokio::test]
async fn plaintext_legacy_config_loads_and_rewrites_encrypted() {
    let (_tmp, store) = make_encrypted_store();
    let dir = store.data_dir().join("workspaces").join("legacy-encrypt");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("agent.json"),
        serde_json::to_vec(&json!({
            "id": "legacy-encrypt",
            "name": "legacy",
            "mcp": {
                "clients": {
                    "old-tool": {
                        "name": "old-tool",
                        "transport": "stdio",
                        "command": "/bin/cat",
                        "args": [],
                        "env": {"API_KEY": "legacy-secret-789"}
                    }
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let exec = store
        .get_client_for_execution(Some("legacy-encrypt"), "old-tool")
        .await
        .expect("legacy plaintext loads")
        .expect("client");
    assert_eq!(get_env_value(&exec, "API_KEY"), Some("legacy-secret-789"));

    store
        .toggle_client(Some("legacy-encrypt"), "old-tool", false)
        .await
        .expect("rewrite");

    let raw = read_agent_json(store.data_dir(), "legacy-encrypt");
    let mcp = &raw["mcp"];
    assert!(
        !serde_json::to_string(mcp)
            .unwrap()
            .contains("legacy-secret-789"),
        "legacy plaintext secret must be encrypted after rewrite: {mcp:#}"
    );
    assert!(secret_field_io::contains_encrypted_fields(mcp));

    let encrypted_once = std::fs::read(
        store
            .data_dir()
            .join("workspaces")
            .join("legacy-encrypt")
            .join("agent.json"),
    )
    .expect("read encrypted legacy agent");

    store
        .toggle_client(Some("legacy-encrypt"), "old-tool", false)
        .await
        .expect("idempotent same-value rewrite");

    let encrypted_twice = std::fs::read(
        store
            .data_dir()
            .join("workspaces")
            .join("legacy-encrypt")
            .join("agent.json"),
    )
    .expect("read encrypted legacy agent again");
    assert_eq!(
        encrypted_once, encrypted_twice,
        "same logical MCP value must not be re-encrypted or rewritten"
    );
}

#[tokio::test]
async fn encrypted_store_noop_masked_update_does_not_churn_file() {
    let (_tmp, store) = make_encrypted_store();
    store
        .create_client(Some("default"), sample_create("idempotent"))
        .await
        .expect("create");

    let agent_path = store
        .data_dir()
        .join("workspaces")
        .join("default")
        .join("agent.json");
    let before = std::fs::read(&agent_path).expect("read before");

    let masked = store
        .get_client(Some("default"), "idempotent")
        .await
        .expect("get")
        .expect("present");
    let masked_json = serde_json::to_value(&masked.server).unwrap();
    store
        .update_client(
            Some("default"),
            "idempotent",
            McpClientUpdate {
                env: masked_json.get("env").cloned(),
                headers: masked_json.get("headers").cloned(),
                ..Default::default()
            },
        )
        .await
        .expect("noop masked update");

    let after = std::fs::read(&agent_path).expect("read after");
    assert_eq!(
        before, after,
        "masked echo-back with no logical changes must not rewrite encrypted MCP config"
    );
}

#[tokio::test]
async fn toggle_and_delete_work() {
    let (_tmp, store) = make_store();
    store
        .create_client(Some("default"), sample_create("qux"))
        .await
        .expect("create");

    let toggled = store
        .toggle_client(Some("default"), "qux", false)
        .await
        .expect("toggle");
    assert!(!toggled.server.enabled);

    store
        .delete_client(Some("default"), "qux")
        .await
        .expect("delete");

    let after = store.list_clients(Some("default")).await.expect("list");
    assert!(after.is_empty(), "delete should remove client");
}

#[tokio::test]
async fn duplicate_create_returns_validation_error() {
    let (_tmp, store) = make_store();
    store
        .create_client(Some("default"), sample_create("dup"))
        .await
        .expect("first");

    let err = store
        .create_client(Some("default"), sample_create("dup"))
        .await
        .expect_err("second create must fail");
    let msg = format!("{err}");
    assert!(
        msg.contains("already exists"),
        "expected duplicate error, got: {msg}"
    );
}

#[tokio::test]
async fn round_trip_store_to_manager_type() {
    let (_tmp, store) = make_store();

    let create = McpClientCreate {
        key: "rt".to_string(),
        name: "round-trip-server".to_string(),
        enabled: true,
        transport: "stdio".to_string(),
        command: Some("/usr/bin/node".to_string()),
        args: vec!["server.js".to_string(), "--port=3000".to_string()],
        env: Some(json!({"NODE_ENV": "production", "API_KEY": "sk-abc123"})),
        url: None,
        headers: None,
        timeout: None,
        auto_start: None,
        oauth: None,
        disabled_tools: Vec::new(),
        extra: serde_json::Map::new(),
    };
    store
        .create_client(Some("default"), create)
        .await
        .expect("create");

    let raw = read_agent_json(store.data_dir(), "default");
    let client_json = raw["mcp"]["clients"]["rt"].clone();

    let server: McpServerConfig = serde_json::from_value(client_json)
        .expect("on-disk JSON must deserialize as McpServerConfig");

    assert_eq!(server.name, "round-trip-server");
    assert!(server.enabled);
    match &server.transport {
        McpTransportConfig::Stdio {
            command, args, env, ..
        } => {
            assert_eq!(command, "/usr/bin/node");
            assert_eq!(args, &["server.js", "--port=3000"]);
            assert_eq!(env.get("NODE_ENV").map(|s| s.as_str()), Some("production"));
            assert_eq!(env.get("API_KEY").map(|s| s.as_str()), Some("sk-abc123"));
        }
        other => panic!("expected Stdio transport, got {other:?}"),
    }
}

#[tokio::test]
async fn legacy_format_loads_without_error() {
    let (_tmp, store) = make_store();

    let dir = store.data_dir().join("workspaces").join("legacy-agent");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("agent.json"),
        serde_json::to_vec(&json!({
            "id": "legacy-agent",
            "name": "legacy",
            "mcp": {
                "clients": {
                    "old-tool": {
                        "name": "old-tool",
                        "type": "stdio",
                        "isActive": false,
                        "command": "/bin/cat",
                        "args": null,
                        "env": null,
                        "description": null,
                        "baseUrl": null
                    }
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let listed = store
        .list_clients(Some("legacy-agent"))
        .await
        .expect("legacy format must parse");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].key, "old-tool");
    assert_eq!(listed[0].server.name, "old-tool");
    assert!(!listed[0].server.enabled);
    match &listed[0].server.transport {
        McpTransportConfig::Stdio { command, args, .. } => {
            assert_eq!(command, "/bin/cat");
            assert!(args.is_empty(), "null args must become empty vec");
        }
        other => panic!("expected Stdio, got {other:?}"),
    }
}
