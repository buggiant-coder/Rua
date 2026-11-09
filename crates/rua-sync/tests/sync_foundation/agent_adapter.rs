use std::sync::Arc;

use rua_sync::agent_adapter::{
    AgentConfigSyncAdapter, AGENT_CONFIG_DOMAIN, AGENT_CONFIG_KIND_DELETE,
    AGENT_CONFIG_KIND_SNAPSHOT, AGENT_CONFIG_KIND_UPSERT,
};
use rua_sync::hlc::HlcTimestamp;
use rua_sync::merge_store::MergeStore;
use rua_sync::sidecar::RecordState;
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

fn agent_json(data_dir: &std::path::Path, agent_id: &str) -> std::path::PathBuf {
    data_dir
        .join("workspaces")
        .join(agent_id)
        .join("agent.json")
}

fn write_agent(data_dir: &std::path::Path, agent_id: &str, name: &str) {
    let path = agent_json(data_dir, agent_id);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    rua_config::atomic_write_json(
        &path,
        &serde_json::json!({
            "id": agent_id,
            "name": name,
            "workspace_dir": data_dir.join("workspaces").join(agent_id).to_string_lossy(),
            "enabled": true
        }),
    )
    .unwrap();
}

fn read_agent_order(data_dir: &std::path::Path) -> Vec<String> {
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(data_dir.join("config.json")).unwrap())
            .unwrap();
    config["agents"]["agent_order"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn inbound_agent_config_creates_file_and_registry_with_local_workspace_dir() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = Arc::new(AgentConfigSyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    let change = AgentConfigSyncAdapter::upsert_change(
        "op-agent",
        "test1",
        serde_json::json!({
            "id": "test1",
            "name": "Test Agent",
            "workspace_dir": "/remote/device/workspaces/test1",
            "enabled": true
        }),
        "device-a",
        hlc(100),
    );

    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let local_workspace = data.path().join("workspaces").join("test1");
    let agent: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(agent_json(data.path(), "test1")).unwrap())
            .unwrap();
    assert_eq!(agent["id"], "test1");
    assert_eq!(agent["name"], "Test Agent");
    assert_eq!(
        agent["workspace_dir"],
        local_workspace.to_string_lossy().as_ref()
    );

    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(data.path().join("config.json")).unwrap())
            .unwrap();
    assert_eq!(config["agents"]["profiles"]["test1"]["id"], "test1");
    assert_eq!(
        config["agents"]["profiles"]["test1"]["workspace_dir"],
        local_workspace.to_string_lossy().as_ref()
    );
    assert_eq!(config["agents"]["agent_order"][0], "test1");
    let cursor = merge_store
        .get_record_cursor("agent_config:test1")
        .unwrap()
        .expect("agent config cursor");
    assert_eq!(cursor.base_blob_hash, None);
}

#[tokio::test]
async fn inbound_agent_config_encrypts_mcp_secrets_for_local_device() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = Arc::new(
        AgentConfigSyncAdapter::new(data.path().to_path_buf(), "device-b")
            .with_local_mcp_key(DEVICE_B_KEY),
    );
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    let logical_mcp = serde_json::json!({
        "clients": {
            "sync-client": {
                "transport": "streamable_http",
                "url": "https://sync.invalid/mcp",
                "env": {
                    "API_KEY": "agent-mcp-env-secret"
                },
                "headers": {
                    "Authorization": "Bearer agent-mcp-auth-secret",
                    "User-Agent": "rua-test"
                },
                "nested": {
                    "database_password": "agent-mcp-db-secret",
                    "display_name": "Nested"
                }
            }
        }
    });
    let change = AgentConfigSyncAdapter::upsert_change(
        "op-agent-mcp",
        "test1",
        serde_json::json!({
            "id": "test1",
            "name": "MCP Agent",
            "workspace_dir": "/remote/device/workspaces/test1",
            "mcp": logical_mcp
        }),
        "device-a",
        hlc(100),
    );

    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let disk_text = std::fs::read_to_string(agent_json(data.path(), "test1")).unwrap();
    assert!(!disk_text.contains("agent-mcp-env-secret"));
    assert!(!disk_text.contains("agent-mcp-auth-secret"));
    assert!(!disk_text.contains("agent-mcp-db-secret"));

    let disk_agent: serde_json::Value = serde_json::from_str(&disk_text).unwrap();
    let disk_mcp = disk_agent.get("mcp").unwrap();
    assert!(rua_mcp::secret_field_io::contains_encrypted_fields(
        disk_mcp
    ));
    let decrypted =
        rua_mcp::secret_field_io::decrypt_client_value(disk_mcp, Some(&DEVICE_B_KEY)).unwrap();
    assert_eq!(decrypted, logical_mcp);
}

#[tokio::test]
async fn export_reads_local_agent_json_files() {
    let data = tempfile::tempdir().unwrap();
    let path = agent_json(data.path(), "default");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "id": "default",
            "name": "Default"
        }))
        .unwrap(),
    )
    .unwrap();

    let adapter = AgentConfigSyncAdapter::new(data.path().to_path_buf(), "device-a");
    let exported = adapter.export_records().await.unwrap();

    assert_eq!(exported.len(), 1);
    assert_eq!(exported[0].domain, AGENT_CONFIG_DOMAIN);
    assert_eq!(exported[0].record_id, "default");
    assert_eq!(exported[0].kind, AGENT_CONFIG_KIND_SNAPSHOT);
    assert_eq!(exported[0].payload["name"], "Default");
}

#[tokio::test]
async fn export_filters_untouched_seeded_agent_config() {
    let data = tempfile::tempdir().unwrap();
    write_agent(data.path(), "default", "Default");
    write_agent(data.path(), "user", "User");
    rua_core::seed_manifest::record_seed_entry(
        data.path(),
        "agent:default",
        "agent_seed_catalog",
        &["workspaces/default/agent.json"],
    )
    .unwrap();

    let adapter = AgentConfigSyncAdapter::new(data.path().to_path_buf(), "device-a");
    let exported = adapter.export_records().await.unwrap();
    let ids: Vec<_> = exported
        .iter()
        .map(|change| change.record_id.as_str())
        .collect();

    assert_eq!(ids, vec!["user"]);
}

#[tokio::test]
async fn export_keeps_seeded_agent_config_after_modification() {
    let data = tempfile::tempdir().unwrap();
    write_agent(data.path(), "default", "Default");
    rua_core::seed_manifest::record_seed_entry(
        data.path(),
        "agent:default",
        "agent_seed_catalog",
        &["workspaces/default/agent.json"],
    )
    .unwrap();
    write_agent(data.path(), "default", "Customized Default");

    let adapter = AgentConfigSyncAdapter::new(data.path().to_path_buf(), "device-a");
    let exported = adapter.export_records().await.unwrap();

    assert_eq!(exported.len(), 1);
    assert_eq!(exported[0].record_id, "default");
    assert_eq!(exported[0].payload["name"], "Customized Default");
}

#[tokio::test]
async fn export_normalizes_local_mcp_envelopes_before_sync() {
    let data = tempfile::tempdir().unwrap();
    let path = agent_json(data.path(), "default");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let logical_mcp = serde_json::json!({
        "clients": {
            "sync-client": {
                "transport": "streamable_http",
                "url": "https://sync.invalid/mcp",
                "env": {
                    "API_KEY": "agent-mcp-export-secret"
                }
            }
        }
    });
    let encrypted_mcp = rua_mcp::secret_field_io::encrypt_client_value(
        &logical_mcp,
        Some(&DEVICE_A_KEY),
        "agent:default:mcp",
    )
    .unwrap();
    rua_config::atomic_write_json(
        &path,
        &serde_json::json!({
            "id": "default",
            "name": "Default",
            "mcp": encrypted_mcp
        }),
    )
    .unwrap();

    let adapter = AgentConfigSyncAdapter::new(data.path().to_path_buf(), "device-a")
        .with_local_mcp_key(DEVICE_A_KEY);
    let exported = adapter.export_records().await.unwrap();

    assert_eq!(exported.len(), 1);
    assert_eq!(exported[0].payload["mcp"], logical_mcp);
    assert!(!rua_mcp::secret_field_io::contains_encrypted_fields(
        &exported[0].payload["mcp"]
    ));
}

#[tokio::test]
async fn export_skips_agent_with_unreadable_mcp_secret_without_blocking_others() {
    let data = tempfile::tempdir().unwrap();
    write_agent(data.path(), "good", "Good");

    let bad_path = agent_json(data.path(), "bad");
    std::fs::create_dir_all(bad_path.parent().unwrap()).unwrap();
    let encrypted_mcp = rua_mcp::secret_field_io::encrypt_client_value(
        &serde_json::json!({
            "servers": {
                "bad": {
                    "command": "bad-tool",
                    "env": { "TOKEN": "secret" }
                }
            }
        }),
        Some(&DEVICE_B_KEY),
        "agent:bad:mcp",
    )
    .unwrap();
    rua_config::atomic_write_json(
        &bad_path,
        &serde_json::json!({
            "id": "bad",
            "name": "Bad",
            "mcp": encrypted_mcp
        }),
    )
    .unwrap();

    let adapter = AgentConfigSyncAdapter::new(data.path().to_path_buf(), "device-a")
        .with_local_mcp_key(DEVICE_A_KEY);
    let exported = adapter.export_records().await.unwrap();

    assert_eq!(exported.len(), 1);
    assert_eq!(exported[0].record_id, "good");
}

#[tokio::test]
async fn export_omits_device_local_agent_fields() {
    let data = tempfile::tempdir().unwrap();
    let path = agent_json(data.path(), "default");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    rua_config::atomic_write_json(
        &path,
        &serde_json::json!({
            "id": "default",
            "name": "Default",
            "channels": {
                "dingtalk": {"enabled": true, "token": "local-only"}
            },
            "running": {"max_iters": 17},
            "default_model": {"provider": "local", "model": "device-model"}
        }),
    )
    .unwrap();

    let adapter = AgentConfigSyncAdapter::new(data.path().to_path_buf(), "device-a");
    let exported = adapter.export_records().await.unwrap();

    assert_eq!(exported.len(), 1);
    assert!(exported[0].payload.get("channels").is_none());
    assert!(exported[0].payload.get("running").is_none());
    assert!(exported[0].payload.get("default_model").is_none());
}

#[tokio::test]
async fn export_omits_agent_security_profile() {
    let data = tempfile::tempdir().unwrap();
    let path = agent_json(data.path(), "default");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    rua_config::atomic_write_json(
        &path,
        &serde_json::json!({
            "id": "default",
            "name": "Default",
            "security": {
                "tool_guard": {"approval_mode": "strict"}
            }
        }),
    )
    .unwrap();

    let adapter = AgentConfigSyncAdapter::new(data.path().to_path_buf(), "device-a");
    let exported = adapter.export_records().await.unwrap();

    assert_eq!(exported.len(), 1);
    assert!(
        exported[0].payload.get("security").is_none(),
        "per-agent security profiles are synced by agent_security, not agent_config"
    );
}

#[tokio::test]
async fn export_omits_device_local_acp_runner_fields() {
    let data = tempfile::tempdir().unwrap();
    let path = agent_json(data.path(), "default");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    rua_config::atomic_write_json(
        &path,
        &serde_json::json!({
            "id": "default",
            "name": "Default",
            "acp": {
                "agents": {
                    "coder": {
                        "enabled": true,
                        "command": "/usr/local/bin/opencode",
                        "args": ["acp"],
                        "env": {"API_KEY": "local-only"},
                        "trusted": false,
                        "tool_parse_mode": "call_detail"
                    }
                }
            }
        }),
    )
    .unwrap();

    let adapter = AgentConfigSyncAdapter::new(data.path().to_path_buf(), "device-a");
    let exported = adapter.export_records().await.unwrap();

    let runner = &exported[0].payload["acp"]["agents"]["coder"];
    assert_eq!(runner["enabled"], serde_json::json!(true));
    assert_eq!(runner["trusted"], serde_json::json!(false));
    assert_eq!(runner["tool_parse_mode"], serde_json::json!("call_detail"));
    assert!(runner.get("command").is_none());
    assert!(runner.get("args").is_none());
    assert!(runner.get("env").is_none());
}

#[tokio::test]
async fn inbound_agent_config_preserves_local_channels_and_ignores_remote_channels() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = Arc::new(AgentConfigSyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    let path = agent_json(data.path(), "test1");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    rua_config::atomic_write_json(
        &path,
        &serde_json::json!({
            "id": "test1",
            "name": "Local Agent",
            "workspace_dir": data.path().join("workspaces").join("test1").to_string_lossy(),
            "channels": {
                "dingtalk": {"enabled": true, "token": "local-token"}
            },
            "running": {"max_iters": 17},
            "default_model": {"provider": "local", "model": "local-model"}
        }),
    )
    .unwrap();

    let change = AgentConfigSyncAdapter::upsert_change(
        "op-agent-channels",
        "test1",
        serde_json::json!({
            "id": "test1",
            "name": "Remote Agent",
            "workspace_dir": "/remote/device/workspaces/test1",
            "channels": {
                "dingtalk": {"enabled": false, "token": "remote-token"},
                "telegram": {"enabled": true}
            },
            "running": {"max_iters": 99},
            "default_model": {"provider": "remote", "model": "remote-model"}
        }),
        "device-a",
        hlc(100),
    );

    registry.apply_change(&change, &merge_store).await.unwrap();

    let agent: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(agent_json(data.path(), "test1")).unwrap())
            .unwrap();
    assert_eq!(agent["name"], "Remote Agent");
    assert_eq!(
        agent["channels"]["dingtalk"]["token"],
        serde_json::json!("local-token")
    );
    assert!(agent["channels"].get("telegram").is_none());
    assert_eq!(agent["running"]["max_iters"], 17);
    assert_eq!(agent["default_model"]["provider"], "local");
    assert_eq!(agent["default_model"]["model"], "local-model");
}

#[tokio::test]
async fn inbound_agent_config_preserves_local_security_and_ignores_remote_security() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = Arc::new(AgentConfigSyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    let path = agent_json(data.path(), "test1");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    rua_config::atomic_write_json(
        &path,
        &serde_json::json!({
            "id": "test1",
            "name": "Local Agent",
            "workspace_dir": data.path().join("workspaces").join("test1").to_string_lossy(),
            "security": {
                "tool_guard": {"approval_mode": "custom"}
            }
        }),
    )
    .unwrap();

    let change = AgentConfigSyncAdapter::upsert_change(
        "op-agent-security",
        "test1",
        serde_json::json!({
            "id": "test1",
            "name": "Remote Agent",
            "workspace_dir": "/remote/device/workspaces/test1",
            "security": {
                "tool_guard": {"approval_mode": "strict"}
            }
        }),
        "device-a",
        hlc(100),
    );

    registry.apply_change(&change, &merge_store).await.unwrap();

    let agent: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(agent_json(data.path(), "test1")).unwrap())
            .unwrap();
    assert_eq!(agent["name"], "Remote Agent");
    assert_eq!(agent["security"]["tool_guard"]["approval_mode"], "custom");
}

#[tokio::test]
async fn inbound_agent_config_preserves_local_acp_runner_command_args_env() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = Arc::new(AgentConfigSyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    let path = agent_json(data.path(), "test1");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    rua_config::atomic_write_json(
        &path,
        &serde_json::json!({
            "id": "test1",
            "name": "Local Agent",
            "workspace_dir": data.path().join("workspaces").join("test1").to_string_lossy(),
            "acp": {
                "agents": {
                    "coder": {
                        "enabled": true,
                        "command": "/local/bin/coder",
                        "args": ["--local"],
                        "env": {"TOKEN": "local-token"},
                        "trusted": false
                    }
                }
            }
        }),
    )
    .unwrap();

    let change = AgentConfigSyncAdapter::upsert_change(
        "op-agent-acp",
        "test1",
        serde_json::json!({
            "id": "test1",
            "name": "Remote Agent",
            "workspace_dir": "/remote/device/workspaces/test1",
            "acp": {
                "agents": {
                    "coder": {
                        "enabled": false,
                        "command": "/remote/bin/coder",
                        "args": ["--remote"],
                        "env": {"TOKEN": "remote-token"},
                        "trusted": true,
                        "tool_parse_mode": "update_detail"
                    }
                }
            }
        }),
        "device-a",
        hlc(100),
    );

    registry.apply_change(&change, &merge_store).await.unwrap();

    let agent: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(agent_json(data.path(), "test1")).unwrap())
            .unwrap();
    let runner = &agent["acp"]["agents"]["coder"];
    assert_eq!(runner["enabled"], serde_json::json!(false));
    assert_eq!(runner["trusted"], serde_json::json!(true));
    assert_eq!(
        runner["tool_parse_mode"],
        serde_json::json!("update_detail")
    );
    assert_eq!(runner["command"], serde_json::json!("/local/bin/coder"));
    assert_eq!(runner["args"], serde_json::json!(["--local"]));
    assert_eq!(runner["env"]["TOKEN"], serde_json::json!("local-token"));
}

#[tokio::test]
async fn invalid_agent_id_is_rejected() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = Arc::new(AgentConfigSyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    let change = AgentConfigSyncAdapter::upsert_change(
        "op-bad",
        "../test1",
        serde_json::json!({ "id": "../test1" }),
        "device-a",
        hlc(100),
    );

    let err = registry
        .apply_change(&change, &merge_store)
        .await
        .unwrap_err();

    assert!(err.to_string().contains("invalid agent id"));
    assert!(!agent_json(data.path(), "test1").exists());
}

#[tokio::test]
async fn delete_commits_tombstone_without_copying_remote_path() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = Arc::new(AgentConfigSyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    let change = AgentConfigSyncAdapter::delete_change("op-delete", "test1", "device-a", hlc(120));

    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let cursor = merge_store
        .get_record_cursor("agent_config:test1")
        .unwrap()
        .unwrap();
    assert_eq!(cursor.state, RecordState::Tombstoned);
}

#[tokio::test]
async fn delete_removes_agent_file_and_registry_entry() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = Arc::new(AgentConfigSyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    let upsert = AgentConfigSyncAdapter::upsert_change(
        "op-upsert",
        "test1",
        serde_json::json!({
            "id": "test1",
            "name": "Test Agent",
            "workspace_dir": "/remote/device/workspaces/test1",
            "enabled": true
        }),
        "device-a",
        hlc(100),
    );
    registry.apply_change(&upsert, &merge_store).await.unwrap();
    assert!(agent_json(data.path(), "test1").exists());
    assert_eq!(read_agent_order(data.path()), vec!["test1"]);

    let delete = AgentConfigSyncAdapter::delete_change("op-delete", "test1", "device-a", hlc(120));
    let result = registry.apply_change(&delete, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    assert!(!agent_json(data.path(), "test1").exists());
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(data.path().join("config.json")).unwrap())
            .unwrap();
    assert!(config["agents"]["profiles"].get("test1").is_none());
    assert!(read_agent_order(data.path()).is_empty());
}

#[tokio::test]
async fn stale_agent_delete_does_not_remove_newer_agent_config() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = Arc::new(AgentConfigSyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    write_agent(data.path(), "test1", "Local");
    let newer_upsert = AgentConfigSyncAdapter::upsert_change(
        "op-newer-upsert",
        "test1",
        serde_json::json!({
            "id": "test1",
            "name": "Newer Agent",
            "workspace_dir": "/remote/device/workspaces/test1",
            "enabled": true
        }),
        "device-b",
        hlc(200),
    );
    registry
        .apply_change(&newer_upsert, &merge_store)
        .await
        .unwrap();
    let stale_delete =
        AgentConfigSyncAdapter::delete_change("op-stale-delete", "test1", "device-a", hlc(100));

    let result = registry
        .apply_change(&stale_delete, &merge_store)
        .await
        .unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Skipped);
    let agent: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(agent_json(data.path(), "test1")).unwrap())
            .unwrap();
    assert_eq!(agent["name"], "Newer Agent");
}

#[tokio::test]
async fn duplicate_agent_upsert_is_idempotent() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = Arc::new(AgentConfigSyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    let change = AgentConfigSyncAdapter::upsert_change(
        "op-same",
        "test1",
        serde_json::json!({
            "id": "test1",
            "name": "Test Agent",
            "workspace_dir": "/remote/device/workspaces/test1",
            "enabled": true
        }),
        "device-a",
        hlc(100),
    );

    let first = registry.apply_change(&change, &merge_store).await.unwrap();
    let second = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(first.status, RegistryApplyStatus::Applied);
    assert_eq!(second.status, RegistryApplyStatus::AlreadyApplied);
    assert!(agent_json(data.path(), "test1").exists());
}

#[test]
fn helper_changes_use_agent_domain_and_do_not_embed_agent_id_in_op_id() {
    let change = AgentConfigSyncAdapter::upsert_change(
        AgentConfigSyncAdapter::new_op_id(),
        "test1",
        serde_json::json!({ "id": "test1" }),
        "device-a",
        hlc(10),
    );

    assert_eq!(change.domain, AGENT_CONFIG_DOMAIN);
    assert_eq!(change.kind, AGENT_CONFIG_KIND_UPSERT);
    assert_eq!(change.record_id, "test1");
    assert!(!change.op_id.contains("test1"));
    assert_eq!(
        AgentConfigSyncAdapter::delete_change("op", "test1", "device-a", hlc(20)).kind,
        AGENT_CONFIG_KIND_DELETE
    );
}
