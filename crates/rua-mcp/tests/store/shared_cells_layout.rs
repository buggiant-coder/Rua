use std::collections::BTreeMap;
use std::sync::Arc;

use cell_store::FileSystemCellStore;
use mcp_manager::types::{
    AutoConnectPolicy, McpOAuthConfig, McpServerConfig, McpServerId, McpTransportConfig,
    ReconnectPolicy,
};
use tempfile::TempDir;

use rua_mcp::{secret_field_io, SharedMcpCells};

const TEST_DEVICE_KEY: [u8; 32] = [8; 32];

fn make_cells() -> (TempDir, SharedMcpCells) {
    let tmp = TempDir::new().expect("tempdir");
    let file = tmp.path().join("shared_mcp.json");
    let store = Arc::new(FileSystemCellStore::new());
    (tmp, SharedMcpCells::new(store, file))
}

fn make_encrypted_cells() -> (TempDir, SharedMcpCells) {
    let tmp = TempDir::new().expect("tempdir");
    let file = tmp.path().join("shared_mcp.json");
    let store = Arc::new(FileSystemCellStore::new());
    (tmp, SharedMcpCells::with_key(store, file, TEST_DEVICE_KEY))
}

fn sample_stdio_config(name: &str) -> McpServerConfig {
    let mut env = BTreeMap::new();
    env.insert("API_KEY".to_string(), "shared-cell-secret-123".to_string());
    McpServerConfig {
        name: name.to_string(),
        description: format!("{name} description"),
        enabled: true,
        transport: McpTransportConfig::Stdio {
            command: "/bin/echo".to_string(),
            args: vec!["hello".to_string()],
            env,
            cwd: None,
            use_proot: false,
        },
        auto_connect: AutoConnectPolicy::OnStartup,
        call_timeout_secs: 60,
        max_concurrency: 4,
        oauth: None,
        disabled_tools: Vec::new(),
        extra: BTreeMap::new(),
    }
}

fn sample_oauth_config(name: &str) -> McpServerConfig {
    let mut config = sample_sse_config(name);
    config.oauth = Some(McpOAuthConfig {
        client_id: "public-client".to_string(),
        scope: "read".to_string(),
        access_token: "oauth-access-token".to_string(),
        refresh_token: "oauth-refresh-token".to_string(),
        expires_at: 4_102_444_800.0,
        token_endpoint: "https://auth.example.test/token".to_string(),
        auth_endpoint: "https://auth.example.test/auth".to_string(),
        extra: BTreeMap::from([(
            "client_secret".to_string(),
            serde_json::Value::String("oauth-client-secret".to_string()),
        )]),
    });
    config
}

fn sample_sse_config(name: &str) -> McpServerConfig {
    McpServerConfig {
        name: name.to_string(),
        description: String::new(),
        enabled: true,
        transport: McpTransportConfig::Sse {
            url: format!("https://{name}.example/sse"),
            headers: BTreeMap::new(),
            reconnect: ReconnectPolicy::default(),
        },
        auto_connect: AutoConnectPolicy::OnStartup,
        call_timeout_secs: 60,
        max_concurrency: 4,
        oauth: None,
        disabled_tools: Vec::new(),
        extra: BTreeMap::new(),
    }
}

#[tokio::test]
async fn cell_read_returns_none_for_missing_server() {
    let (_tmp, cells) = make_cells();
    let id = McpServerId::new("absent");
    let got = cells.cell(&id).read().await.expect("read");
    assert!(got.is_none(), "expected None for missing cell, got {got:?}");
}

#[tokio::test]
async fn cell_write_persists_to_correct_json_path() {
    let (tmp, cells) = make_cells();
    let id = McpServerId::new("alpha");
    cells
        .cell(&id)
        .write(sample_stdio_config("alpha"))
        .await
        .expect("write");

    let path = tmp.path().join("shared_mcp.json");
    let raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read shared_mcp.json")).expect("json");

    assert_eq!(
        raw["clients"]["alpha"]["name"].as_str(),
        Some("alpha"),
        "config must land at $.clients.alpha, got {raw:#}",
    );
    assert_eq!(
        raw["clients"]["alpha"]["transport"].as_str(),
        Some("stdio"),
        "transport tag must be preserved as flattened field",
    );
}

#[tokio::test]
async fn cell_update_atomic_mutator_sees_current_value() {
    let (_tmp, cells) = make_cells();
    let id = McpServerId::new("beta");
    cells
        .cell(&id)
        .write(sample_stdio_config("beta"))
        .await
        .expect("seed");

    let updated = cells
        .cell(&id)
        .update(|current| {
            let mut cfg = current.expect("update must see existing value");
            cfg.name = "beta-renamed".to_string();
            cfg
        })
        .await
        .expect("update");
    assert_eq!(updated.name, "beta-renamed");

    let on_disk = cells
        .cell(&id)
        .read()
        .await
        .expect("read")
        .expect("present");
    assert_eq!(on_disk.name, "beta-renamed");
}

#[tokio::test]
async fn cell_update_creates_when_absent() {
    let (_tmp, cells) = make_cells();
    let id = McpServerId::new("gamma");

    let created = cells
        .cell(&id)
        .update(|current| {
            assert!(current.is_none(), "expected None when cell absent");
            sample_stdio_config("gamma")
        })
        .await
        .expect("update");
    assert_eq!(created.name, "gamma");

    let on_disk = cells
        .cell(&id)
        .read()
        .await
        .expect("read")
        .expect("present");
    assert_eq!(on_disk.name, "gamma");
}

#[tokio::test]
async fn cell_delete_removes_entry_only_for_target() {
    let (tmp, cells) = make_cells();
    let alpha = McpServerId::new("alpha");
    let delta = McpServerId::new("delta");
    cells
        .cell(&alpha)
        .write(sample_stdio_config("alpha"))
        .await
        .expect("write alpha");
    cells
        .cell(&delta)
        .write(sample_sse_config("delta"))
        .await
        .expect("write delta");

    cells.cell(&alpha).delete().await.expect("delete alpha");

    assert!(cells.cell(&alpha).read().await.expect("read").is_none());
    let delta_on_disk = cells
        .cell(&delta)
        .read()
        .await
        .expect("read")
        .expect("present");
    assert_eq!(delta_on_disk.name, "delta");

    let raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(tmp.path().join("shared_mcp.json")).expect("read"))
            .expect("json");
    assert!(
        raw["clients"].get("alpha").is_none(),
        "alpha key must be removed from clients map, got {raw:#}",
    );
    assert!(raw["clients"].get("delta").is_some());
}

#[tokio::test]
async fn cell_delete_absent_is_noop() {
    let (_tmp, cells) = make_cells();
    let id = McpServerId::new("never-existed");
    cells.cell(&id).delete().await.expect("delete absent");
}

#[tokio::test]
async fn list_returns_all_servers_in_file() {
    let (_tmp, cells) = make_cells();
    let alpha = McpServerId::new("alpha");
    let beta = McpServerId::new("beta");
    cells
        .cell(&alpha)
        .write(sample_stdio_config("alpha"))
        .await
        .expect("write alpha");
    cells
        .cell(&beta)
        .write(sample_sse_config("beta"))
        .await
        .expect("write beta");

    let listed = cells.list().await.expect("list");
    let mut ids: Vec<String> = listed.keys().map(|k| k.as_str().to_string()).collect();
    ids.sort();
    assert_eq!(ids, vec!["alpha".to_string(), "beta".to_string()]);
    assert_eq!(listed[&alpha].name, "alpha");
    assert_eq!(listed[&beta].name, "beta");
}

#[tokio::test]
async fn list_returns_empty_when_file_missing() {
    let (_tmp, cells) = make_cells();
    let listed = cells.list().await.expect("list");
    assert!(listed.is_empty(), "expected empty list, got {listed:?}");
}

#[tokio::test]
async fn encrypted_cell_write_hides_secret_on_disk_and_reads_decrypted() {
    let (tmp, cells) = make_encrypted_cells();
    let id = McpServerId::new("encrypted-cell");

    cells
        .cell(&id)
        .write(sample_stdio_config("encrypted-cell"))
        .await
        .expect("write encrypted shared cell");

    let raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(tmp.path().join("shared_mcp.json")).expect("read"))
            .expect("json");
    let raw_text = serde_json::to_string(&raw).unwrap();
    assert!(secret_field_io::contains_encrypted_fields(&raw));
    assert!(!raw_text.contains("shared-cell-secret-123"));

    let read = cells
        .cell(&id)
        .read()
        .await
        .expect("read encrypted cell")
        .expect("present");
    match read.transport {
        McpTransportConfig::Stdio { env, .. } => {
            assert_eq!(
                env.get("API_KEY").map(|s| s.as_str()),
                Some("shared-cell-secret-123")
            );
        }
        other => panic!("expected stdio config, got {other:?}"),
    }
}

#[tokio::test]
async fn encrypted_cell_list_decrypts_before_typed_deserialization() {
    let (_tmp, cells) = make_encrypted_cells();
    let id = McpServerId::new("list-encrypted");
    cells
        .cell(&id)
        .write(sample_stdio_config("list-encrypted"))
        .await
        .expect("write");

    let listed = cells.list().await.expect("list encrypted cells");
    let server = listed.get(&id).expect("listed server");
    match &server.transport {
        McpTransportConfig::Stdio { env, .. } => {
            assert_eq!(
                env.get("API_KEY").map(|s| s.as_str()),
                Some("shared-cell-secret-123")
            );
        }
        other => panic!("expected stdio config, got {other:?}"),
    }
}

#[tokio::test]
async fn encrypted_cell_preserves_oauth_extra_client_secret() {
    let (tmp, cells) = make_encrypted_cells();
    let id = McpServerId::new("oauth-extra");
    cells
        .cell(&id)
        .write(sample_oauth_config("oauth-extra"))
        .await
        .expect("write");

    let raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(tmp.path().join("shared_mcp.json")).expect("read"))
            .expect("json");
    let raw_text = serde_json::to_string(&raw).unwrap();
    assert!(!raw_text.contains("oauth-client-secret"));
    assert!(secret_field_io::contains_encrypted_fields(&raw));

    let read = cells
        .cell(&id)
        .read()
        .await
        .expect("read encrypted oauth")
        .expect("present");
    let oauth = read.oauth.expect("oauth");
    assert_eq!(oauth.extra["client_secret"], "oauth-client-secret");
}
