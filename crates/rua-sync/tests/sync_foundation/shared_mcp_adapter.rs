use std::sync::Arc;

use rua_sync::hlc::HlcTimestamp;
use rua_sync::merge_store::MergeStore;
use rua_sync::shared_mcp_adapter::{
    SharedMcpSyncAdapter, SHARED_MCP_DOMAIN, SHARED_MCP_KIND_SNAPSHOT, SHARED_MCP_KIND_UPSERT,
};
use rua_sync::sync_registry::{DomainSyncAdapter, RegistryApplyStatus, SyncRegistry};

const DEVICE_A_KEY: [u8; 32] = [0xA1; 32];
const DEVICE_B_KEY: [u8; 32] = [0xB2; 32];

fn hlc(wall_ms: u64) -> HlcTimestamp {
    HlcTimestamp {
        wall_ms,
        counter: 0,
        device_id: [0; 8],
    }
}

fn shared_mcp_path(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("shared_mcp.json")
}

#[tokio::test]
async fn export_shared_mcp_snapshot_decrypts_local_secret_fields() {
    let data = tempfile::tempdir().unwrap();
    let encrypted = rua_mcp::secret_field_io::encrypt_client_value(
        &serde_json::json!({
            "clients": {
                "shared-a": {
                    "name": "Shared A",
                    "transport": "streamable_http",
                    "url": "https://mcp.example.test",
                    "headers": {
                        "Authorization": "Bearer local-token"
                    }
                }
            }
        }),
        Some(&DEVICE_A_KEY),
        "shared:mcp",
    )
    .unwrap();
    rua_config::atomic_write_json(&shared_mcp_path(data.path()), &encrypted).unwrap();

    let adapter = SharedMcpSyncAdapter::new(data.path().to_path_buf(), "device-a")
        .with_local_mcp_key(DEVICE_A_KEY);
    let records = adapter.export_records().await.unwrap();

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].domain, SHARED_MCP_DOMAIN);
    assert_eq!(records[0].kind, SHARED_MCP_KIND_SNAPSHOT);
    assert_eq!(
        records[0].payload["clients"]["shared-a"]["headers"]["Authorization"],
        "Bearer local-token"
    );
}

#[tokio::test]
async fn inbound_shared_mcp_snapshot_reencrypts_for_local_device() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = Arc::new(
        SharedMcpSyncAdapter::new(data.path().to_path_buf(), "device-b")
            .with_local_mcp_key(DEVICE_B_KEY),
    );
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    let change = SharedMcpSyncAdapter::upsert_change(
        "op-shared-mcp",
        serde_json::json!({
            "clients": {
                "shared-a": {
                    "name": "Shared A",
                    "transport": "stdio",
                    "command": "node",
                    "args": ["server.js"],
                    "env": {
                        "TOKEN": "remote-token"
                    }
                }
            }
        }),
        "device-a",
        hlc(100),
    );

    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(shared_mcp_path(data.path())).unwrap())
            .unwrap();
    assert!(rua_mcp::secret_field_io::contains_encrypted_fields(&raw));
    assert_ne!(
        raw["clients"]["shared-a"]["env"]["TOKEN"],
        serde_json::json!("remote-token")
    );

    let decrypted =
        rua_mcp::secret_field_io::decrypt_client_value(&raw, Some(&DEVICE_B_KEY)).unwrap();
    assert_eq!(
        decrypted["clients"]["shared-a"]["env"]["TOKEN"],
        "remote-token"
    );
}

#[tokio::test]
async fn inbound_shared_mcp_replaces_catalog_by_lww() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    rua_config::atomic_write_json(
        &shared_mcp_path(data.path()),
        &serde_json::json!({
            "clients": {
                "local-only": {
                    "name": "Local",
                    "transport": "stdio",
                    "command": "local"
                }
            }
        }),
    )
    .unwrap();
    let adapter = Arc::new(SharedMcpSyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    let change = SharedMcpSyncAdapter::upsert_change(
        "op-shared-mcp-2",
        serde_json::json!({
            "clients": {
                "remote-only": {
                    "name": "Remote",
                    "transport": "stdio",
                    "command": "remote"
                }
            }
        }),
        "device-a",
        hlc(200),
    );

    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(shared_mcp_path(data.path())).unwrap())
            .unwrap();
    assert!(stored["clients"].get("local-only").is_none());
    assert_eq!(
        stored["clients"]["remote-only"]["command"],
        serde_json::json!("remote")
    );
    let cursor = merge_store
        .get_record_cursor("shared_mcp:catalog")
        .unwrap()
        .expect("shared mcp cursor");
    assert_eq!(cursor.base_blob_hash, None);
}

#[tokio::test]
async fn inbound_shared_mcp_same_client_uses_lww_and_encrypts_local_disk() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = Arc::new(
        SharedMcpSyncAdapter::new(data.path().to_path_buf(), "device-b")
            .with_local_mcp_key(DEVICE_B_KEY),
    );
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    let base_change = SharedMcpSyncAdapter::upsert_change(
        "op-shared-mcp-base",
        serde_json::json!({
            "clients": {
                "shared-a": {
                    "name": "Shared A",
                    "transport": "stdio",
                    "command": "node",
                    "env": {
                        "TOKEN": "base-token"
                    }
                }
            }
        }),
        "device-a",
        hlc(100),
    );
    registry
        .apply_change(&base_change, &merge_store)
        .await
        .unwrap();

    let path = shared_mcp_path(data.path());
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let mut logical =
        rua_mcp::secret_field_io::decrypt_client_value(&raw, Some(&DEVICE_B_KEY)).unwrap();
    logical["clients"]["shared-a"]["name"] = serde_json::json!("Local Shared A");
    let local_disk =
        rua_mcp::secret_field_io::encrypt_client_value(&logical, Some(&DEVICE_B_KEY), "shared:mcp")
            .unwrap();
    rua_config::atomic_write_json(&path, &local_disk).unwrap();

    let remote_change = SharedMcpSyncAdapter::upsert_change(
        "op-shared-mcp-remote",
        serde_json::json!({
            "clients": {
                "shared-a": {
                    "name": "Shared A",
                    "transport": "stdio",
                    "command": "node",
                    "env": {
                        "TOKEN": "remote-token"
                    }
                }
            }
        }),
        "device-a",
        hlc(200),
    );

    let result = registry
        .apply_change(&remote_change, &merge_store)
        .await
        .unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(shared_mcp_path(data.path())).unwrap())
            .unwrap();
    let decrypted =
        rua_mcp::secret_field_io::decrypt_client_value(&stored, Some(&DEVICE_B_KEY)).unwrap();
    assert_eq!(decrypted["clients"]["shared-a"]["name"], "Shared A");
    assert_eq!(
        decrypted["clients"]["shared-a"]["env"]["TOKEN"],
        "remote-token"
    );
    assert_ne!(
        stored["clients"]["shared-a"]["env"]["TOKEN"],
        serde_json::json!("remote-token")
    );
    let cursor = merge_store
        .get_record_cursor("shared_mcp:catalog")
        .unwrap()
        .expect("shared mcp cursor");
    assert_eq!(cursor.base_blob_hash, None);
}

#[tokio::test]
async fn inbound_shared_mcp_client_delete_removes_client_without_catalog_base() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    rua_config::atomic_write_json(
        &shared_mcp_path(data.path()),
        &serde_json::json!({
            "clients": {
                "delete-me": {
                    "name": "Delete Me",
                    "transport": "stdio",
                    "command": "node"
                },
                "keep-me": {
                    "name": "Keep Me",
                    "transport": "stdio",
                    "command": "python"
                }
            }
        }),
    )
    .unwrap();
    let adapter = Arc::new(SharedMcpSyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    let change =
        SharedMcpSyncAdapter::client_delete_change("op-delete", "delete-me", "device-a", hlc(300));

    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(shared_mcp_path(data.path())).unwrap())
            .unwrap();
    assert!(stored["clients"].get("delete-me").is_none());
    assert_eq!(
        stored["clients"]["keep-me"]["command"],
        serde_json::json!("python")
    );
}

#[test]
fn upsert_change_uses_stable_catalog_record() {
    let change = SharedMcpSyncAdapter::upsert_change(
        "op",
        serde_json::json!({ "clients": {} }),
        "device-a",
        hlc(1),
    );

    assert_eq!(change.domain, SHARED_MCP_DOMAIN);
    assert_eq!(change.record_id, "catalog");
    assert_eq!(change.kind, SHARED_MCP_KIND_UPSERT);
}
