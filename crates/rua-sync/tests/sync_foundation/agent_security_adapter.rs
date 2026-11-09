use std::sync::Arc;

use rua_sync::agent_security_adapter::{
    AgentSecuritySyncAdapter, AGENT_SECURITY_DOMAIN, AGENT_SECURITY_KIND_DELETE,
    AGENT_SECURITY_KIND_SNAPSHOT, AGENT_SECURITY_KIND_UPSERT,
};
use rua_sync::hlc::HlcTimestamp;
use rua_sync::merge_store::MergeStore;
use rua_sync::sync_registry::{DomainSyncAdapter, RegistryApplyStatus, SyncRegistry};

fn hlc(wall_ms: u64) -> HlcTimestamp {
    HlcTimestamp {
        wall_ms,
        counter: 0,
        device_id: [0; 8],
    }
}

fn security_json(data_dir: &std::path::Path, agent_id: &str) -> std::path::PathBuf {
    data_dir
        .join("workspaces")
        .join(agent_id)
        .join("security.json")
}

fn write_security(data_dir: &std::path::Path, agent_id: &str, value: serde_json::Value) {
    let path = security_json(data_dir, agent_id);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    rua_config::atomic_write_json(&path, &value).unwrap();
}

#[tokio::test]
async fn export_reads_per_agent_security_profiles() {
    let data = tempfile::tempdir().unwrap();
    write_security(
        data.path(),
        "default",
        serde_json::json!({
            "tool_guard": {
                "enabled": true,
                "approval_mode": "custom",
                "denied_tools": ["bash"]
            },
            "allow_no_auth_hosts": ["203.0.113.10"]
        }),
    );

    let adapter = AgentSecuritySyncAdapter::new(data.path().to_path_buf(), "device-a");
    let exported = adapter.export_records().await.unwrap();

    assert_eq!(exported.len(), 1);
    assert_eq!(exported[0].domain, AGENT_SECURITY_DOMAIN);
    assert_eq!(exported[0].record_id, "default");
    assert_eq!(exported[0].kind, AGENT_SECURITY_KIND_SNAPSHOT);
    assert_eq!(exported[0].payload["tool_guard"]["denied_tools"][0], "bash");
    assert!(
        exported[0].payload.get("allow_no_auth_hosts").is_none(),
        "allow_no_auth_hosts is process-local and must not export through agent_security"
    );
}

#[tokio::test]
async fn inbound_agent_security_creates_security_json() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = Arc::new(AgentSecuritySyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    let change = AgentSecuritySyncAdapter::upsert_change(
        "op-security",
        "agent-a",
        serde_json::json!({
            "tool_guard": {
                "enabled": true,
                "approval_mode": "strict",
                "denied_tools": ["bash"]
            },
            "allow_no_auth_hosts": ["203.0.113.10"]
        }),
        "device-a",
        hlc(100),
    );

    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let disk: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(security_json(data.path(), "agent-a")).unwrap(),
    )
    .unwrap();
    assert_eq!(disk["tool_guard"]["approval_mode"], "strict");
    assert_eq!(disk["tool_guard"]["denied_tools"][0], "bash");
    assert!(
        disk.get("allow_no_auth_hosts").is_none(),
        "inbound agent_security must not write process-local auth bypass hosts"
    );
}

#[tokio::test]
async fn inbound_agent_security_uses_lww_without_base_snapshot() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = Arc::new(AgentSecuritySyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    let base = serde_json::json!({
        "tool_guard": {"enabled": true, "approval_mode": "smart"},
        "skill_scanner": {"mode": "warn"}
    });
    let first =
        AgentSecuritySyncAdapter::upsert_change("op-base", "agent-a", base, "device-a", hlc(100));
    registry.apply_change(&first, &merge_store).await.unwrap();

    write_security(
        data.path(),
        "agent-a",
        serde_json::json!({
            "tool_guard": {"enabled": true, "approval_mode": "strict"},
            "skill_scanner": {"mode": "warn"}
        }),
    );
    let remote = AgentSecuritySyncAdapter::upsert_change(
        "op-remote",
        "agent-a",
        serde_json::json!({
            "tool_guard": {"enabled": true, "approval_mode": "smart"},
            "skill_scanner": {"mode": "block"}
        }),
        "device-a",
        hlc(200),
    );

    registry.apply_change(&remote, &merge_store).await.unwrap();

    let disk: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(security_json(data.path(), "agent-a")).unwrap(),
    )
    .unwrap();
    assert_eq!(disk["tool_guard"]["approval_mode"], "smart");
    assert_eq!(disk["skill_scanner"]["mode"], "block");
    let cursor = merge_store
        .get_record_cursor("agent_security:agent-a")
        .unwrap()
        .expect("agent security cursor");
    assert_eq!(cursor.base_blob_hash, None);
}

#[tokio::test]
async fn inbound_agent_security_delete_removes_security_json() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    write_security(
        data.path(),
        "agent-a",
        serde_json::json!({"tool_guard": {}}),
    );

    let adapter = Arc::new(AgentSecuritySyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    let change =
        AgentSecuritySyncAdapter::delete_change("op-delete", "agent-a", "device-a", hlc(100));
    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    assert!(!security_json(data.path(), "agent-a").exists());
}

#[test]
fn builders_create_expected_kinds() {
    let upsert = AgentSecuritySyncAdapter::upsert_change(
        "op-upsert",
        "agent-a",
        serde_json::json!({}),
        "device-a",
        hlc(1),
    );
    assert_eq!(upsert.domain, AGENT_SECURITY_DOMAIN);
    assert_eq!(upsert.kind, AGENT_SECURITY_KIND_UPSERT);

    let delete =
        AgentSecuritySyncAdapter::delete_change("op-delete", "agent-a", "device-a", hlc(2));
    assert_eq!(delete.domain, AGENT_SECURITY_DOMAIN);
    assert_eq!(delete.kind, AGENT_SECURITY_KIND_DELETE);
}
