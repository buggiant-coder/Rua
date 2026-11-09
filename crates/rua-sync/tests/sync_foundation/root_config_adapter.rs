use std::sync::Arc;

use rua_sync::hlc::HlcTimestamp;
use rua_sync::merge_store::MergeStore;
use rua_sync::root_config_adapter::{
    RootConfigSyncAdapter, ROOT_CONFIG_DOMAIN, ROOT_CONFIG_KIND_SNAPSHOT,
    ROOT_CONFIG_RECORD_AGENTS, ROOT_CONFIG_RECORD_DEFAULT_MODEL, ROOT_CONFIG_RECORD_SECURITY,
    ROOT_CONFIG_RECORD_SETTINGS, ROOT_CONFIG_RECORD_SUBAGENT_TEMPLATES,
    ROOT_CONFIG_RECORD_USER_TIMEZONE,
};
use rua_sync::sync_registry::{DomainSyncAdapter, RegistryApplyStatus, SyncRegistry};

fn hlc(wall_ms: u64) -> HlcTimestamp {
    HlcTimestamp {
        wall_ms,
        counter: 0,
        device_id: [0; 8],
    }
}

#[tokio::test]
async fn export_reads_portable_root_config_files() {
    let data = tempfile::tempdir().unwrap();
    let secret = tempfile::tempdir().unwrap();
    rua_config::atomic_write_json(
        &data.path().join("settings.json"),
        &serde_json::json!({"language": "zh"}),
    )
    .unwrap();
    rua_config::atomic_write_json(
        &data.path().join("subagent_templates.json"),
        &serde_json::json!({"version": 1, "templates": [{"id": "sub-a"}]}),
    )
    .unwrap();
    rua_config::atomic_write_json(
        &secret.path().join("providers").join("default_model.json"),
        &serde_json::json!({"provider_id": "dashscope", "model": "qwen"}),
    )
    .unwrap();
    rua_config::atomic_write_json(
        &data.path().join("config.json"),
        &serde_json::json!({
            "agents": {
                "profiles": {
                    "default": {
                        "id": "default",
                        "workspace_dir": "/Users/desktop/.rua/workspaces/default"
                    }
                },
                "agent_order": ["default"]
            },
            "security": {
                "file_guard": {"enabled": false},
                "allow_no_auth_hosts": ["203.0.113.10"]
            },
            "user_timezone": "Asia/Shanghai",
            "channels": {"local": true}
        }),
    )
    .unwrap();

    let adapter = RootConfigSyncAdapter::new(
        data.path().to_path_buf(),
        secret.path().to_path_buf(),
        "device-a",
    );
    let changes = adapter.export_records().await.unwrap();
    let ids: Vec<_> = changes
        .iter()
        .map(|change| change.record_id.as_str())
        .collect();

    assert_eq!(
        ids,
        vec![
            ROOT_CONFIG_RECORD_SETTINGS,
            ROOT_CONFIG_RECORD_SUBAGENT_TEMPLATES,
            ROOT_CONFIG_RECORD_AGENTS,
            ROOT_CONFIG_RECORD_SECURITY,
            ROOT_CONFIG_RECORD_USER_TIMEZONE,
        ]
    );
    assert!(changes
        .iter()
        .all(|change| change.domain == ROOT_CONFIG_DOMAIN));
    assert!(changes
        .iter()
        .all(|change| change.kind == ROOT_CONFIG_KIND_SNAPSHOT));
    let security = changes
        .iter()
        .find(|change| change.record_id == ROOT_CONFIG_RECORD_SECURITY)
        .expect("security root config snapshot");
    assert_eq!(security.payload["file_guard"]["enabled"], false);
    assert!(
        security.payload.get("allow_no_auth_hosts").is_none(),
        "allow_no_auth_hosts is device-local and must not export"
    );
    let agents = changes
        .iter()
        .find(|change| change.record_id == ROOT_CONFIG_RECORD_AGENTS)
        .expect("agents root config snapshot");
    assert!(
        agents.payload["profiles"]["default"]
            .get("workspace_dir")
            .is_none(),
        "workspace_dir is device-local and must not export"
    );
}

#[tokio::test]
async fn export_filters_untouched_seeded_agents_from_root_agents_projection() {
    let data = tempfile::tempdir().unwrap();
    let secret = tempfile::tempdir().unwrap();
    write_agent_json(data.path(), "default", "小Rua");
    write_agent_json(data.path(), "rua_qa", "QA Agent");
    write_agent_json(data.path(), "user", "User Agent");
    rua_core::seed_manifest::record_seed_entry(
        data.path(),
        "agent:default",
        "agent_seed_catalog",
        &["workspaces/default/agent.json"],
    )
    .unwrap();
    rua_core::seed_manifest::record_seed_entry(
        data.path(),
        "agent:rua_qa",
        "agent_seed_catalog",
        &["workspaces/rua_qa/agent.json"],
    )
    .unwrap();
    rua_config::atomic_write_json(
        &data.path().join("config.json"),
        &serde_json::json!({
            "agents": {
                "active_agent": "default",
                "profiles": {
                    "default": {"id": "default", "workspace_dir": "/tmp/default"},
                    "rua_qa": {"id": "rua_qa", "workspace_dir": "/tmp/rua_qa"},
                    "user": {"id": "user", "workspace_dir": "/tmp/user"}
                },
                "agent_order": ["default", "rua_qa", "user"]
            }
        }),
    )
    .unwrap();

    let adapter = RootConfigSyncAdapter::new(
        data.path().to_path_buf(),
        secret.path().to_path_buf(),
        "device-a",
    );
    let changes = adapter.export_records().await.unwrap();
    let agents = changes
        .iter()
        .find(|change| change.record_id == ROOT_CONFIG_RECORD_AGENTS)
        .expect("agents root config snapshot");

    assert!(agents.payload["profiles"].get("default").is_none());
    assert!(agents.payload["profiles"].get("rua_qa").is_none());
    assert_eq!(agents.payload["profiles"]["user"]["id"], "user");
    assert_eq!(agents.payload["agent_order"], serde_json::json!(["user"]));
    assert!(
        agents.payload.get("active_agent").is_none(),
        "active_agent pointing at untouched seed must not export"
    );
}

#[tokio::test]
async fn export_keeps_seeded_agent_projection_after_agent_file_is_modified() {
    let data = tempfile::tempdir().unwrap();
    let secret = tempfile::tempdir().unwrap();
    write_agent_json(data.path(), "default", "小Rua");
    rua_core::seed_manifest::record_seed_entry(
        data.path(),
        "agent:default",
        "agent_seed_catalog",
        &["workspaces/default/agent.json"],
    )
    .unwrap();
    write_agent_json(data.path(), "default", "Customized Default");
    rua_config::atomic_write_json(
        &data.path().join("config.json"),
        &serde_json::json!({
            "agents": {
                "active_agent": "default",
                "profiles": {
                    "default": {"id": "default", "workspace_dir": "/tmp/default"}
                },
                "agent_order": ["default"]
            }
        }),
    )
    .unwrap();

    let adapter = RootConfigSyncAdapter::new(
        data.path().to_path_buf(),
        secret.path().to_path_buf(),
        "device-a",
    );
    let changes = adapter.export_records().await.unwrap();
    let agents = changes
        .iter()
        .find(|change| change.record_id == ROOT_CONFIG_RECORD_AGENTS)
        .expect("agents root config snapshot");

    assert_eq!(agents.payload["profiles"]["default"]["id"], "default");
    assert_eq!(
        agents.payload["agent_order"],
        serde_json::json!(["default"])
    );
    assert_eq!(agents.payload["active_agent"], "default");
}

#[tokio::test]
async fn inbound_agents_record_updates_only_config_agents_section() {
    let data = tempfile::tempdir().unwrap();
    let secret = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    rua_config::atomic_write_json(
        &data.path().join("config.json"),
        &serde_json::json!({
            "agents": {
                "profiles": {
                    "local": {
                        "id": "local",
                        "workspace_dir": "/tmp/local-only"
                    }
                },
                "agent_order": ["local"]
            },
            "channels": {"desktop": {"enabled": true}}
        }),
    )
    .unwrap();
    let adapter = Arc::new(RootConfigSyncAdapter::new(
        data.path().to_path_buf(),
        secret.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    let change = RootConfigSyncAdapter::upsert_change(
        "op-agents",
        ROOT_CONFIG_RECORD_AGENTS,
        serde_json::json!({
            "profiles": {
                "remote": {
                    "id": "remote",
                    "workspace_dir": "/Users/desktop/.rua/workspaces/remote"
                }
            },
            "agent_order": ["remote"]
        }),
        "device-a",
        hlc(150),
    );

    registry.apply_change(&change, &merge_store).await.unwrap();

    let file: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(data.path().join("config.json")).unwrap())
            .unwrap();
    assert_eq!(file["channels"]["desktop"]["enabled"], true);
    assert_eq!(file["agents"]["profiles"]["local"]["id"], "local");
    assert_eq!(file["agents"]["profiles"]["remote"]["id"], "remote");
    assert_eq!(
        file["agents"]["profiles"]["local"]["workspace_dir"],
        serde_json::Value::String(
            data.path()
                .join("workspaces/local")
                .to_string_lossy()
                .into_owned()
        )
    );
    assert_eq!(
        file["agents"]["profiles"]["remote"]["workspace_dir"],
        serde_json::Value::String(
            data.path()
                .join("workspaces/remote")
                .to_string_lossy()
                .into_owned()
        )
    );
    assert_eq!(file["agents"]["agent_order"][0], "remote");
}

#[tokio::test]
async fn inbound_agents_record_preserves_local_untouched_seeded_agents() {
    let data = tempfile::tempdir().unwrap();
    let secret = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    write_agent_json(data.path(), "default", "小Rua");
    rua_core::seed_manifest::record_seed_entry(
        data.path(),
        "agent:default",
        "agent_seed_catalog",
        &["workspaces/default/agent.json"],
    )
    .unwrap();
    rua_config::atomic_write_json(
        &data.path().join("config.json"),
        &serde_json::json!({
            "agents": {
                "active_agent": "default",
                "profiles": {
                    "default": {"id": "default", "workspace_dir": "/tmp/default"}
                },
                "agent_order": ["default"]
            }
        }),
    )
    .unwrap();
    let adapter = Arc::new(RootConfigSyncAdapter::new(
        data.path().to_path_buf(),
        secret.path().to_path_buf(),
        "device-a",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    let change = RootConfigSyncAdapter::upsert_change(
        "op-empty-seeded-agents",
        ROOT_CONFIG_RECORD_AGENTS,
        serde_json::json!({
            "profiles": {},
            "agent_order": []
        }),
        "device-b",
        hlc(200),
    );

    registry.apply_change(&change, &merge_store).await.unwrap();

    let file: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(data.path().join("config.json")).unwrap())
            .unwrap();
    assert_eq!(file["agents"]["profiles"]["default"]["id"], "default");
    assert_eq!(
        file["agents"]["agent_order"],
        serde_json::json!(["default"])
    );
    assert_eq!(file["agents"]["active_agent"], "default");
}

fn write_agent_json(data_dir: &std::path::Path, agent_id: &str, name: &str) {
    let path = data_dir
        .join("workspaces")
        .join(agent_id)
        .join("agent.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    rua_config::atomic_write_json(
        &path,
        &serde_json::json!({
            "id": agent_id,
            "name": name,
            "workspace_dir": data_dir.join("workspaces").join(agent_id).to_string_lossy()
        }),
    )
    .unwrap();
}

#[tokio::test]
async fn inbound_upsert_replaces_settings_by_lww() {
    let data = tempfile::tempdir().unwrap();
    let secret = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    rua_config::atomic_write_json(
        &data.path().join("settings.json"),
        &serde_json::json!({"theme": "dark"}),
    )
    .unwrap();
    let adapter = Arc::new(RootConfigSyncAdapter::new(
        data.path().to_path_buf(),
        secret.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    let change = RootConfigSyncAdapter::upsert_change(
        "op-settings",
        ROOT_CONFIG_RECORD_SETTINGS,
        serde_json::json!({"language": "zh"}),
        "device-a",
        hlc(100),
    );

    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let file: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(data.path().join("settings.json")).unwrap())
            .unwrap();
    assert!(file["theme"].is_null());
    assert_eq!(file["language"], "zh");
    let cursor = merge_store
        .get_record_cursor("root_config:settings")
        .unwrap()
        .expect("settings cursor");
    assert_eq!(cursor.base_blob_hash, None);
}

#[tokio::test]
async fn inbound_default_model_changes_preserve_device_local_selection() {
    let data = tempfile::tempdir().unwrap();
    let secret = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let default_model_path = secret.path().join("providers").join("default_model.json");
    rua_config::atomic_write_json(
        &default_model_path,
        &serde_json::json!({"provider_id": "dashscope", "model": "qwen"}),
    )
    .unwrap();
    let adapter = Arc::new(RootConfigSyncAdapter::new(
        data.path().to_path_buf(),
        secret.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    let upsert = RootConfigSyncAdapter::upsert_change(
        "op-active-upsert",
        ROOT_CONFIG_RECORD_DEFAULT_MODEL,
        serde_json::json!({"provider_id": "openai", "model": "remote"}),
        "device-a",
        hlc(100),
    );
    registry.apply_change(&upsert, &merge_store).await.unwrap();

    let delete = RootConfigSyncAdapter::delete_change(
        "op-active-delete",
        ROOT_CONFIG_RECORD_DEFAULT_MODEL,
        "device-a",
        hlc(200),
    );
    registry.apply_change(&delete, &merge_store).await.unwrap();

    let default_model: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&default_model_path).unwrap()).unwrap();
    assert_eq!(default_model["provider_id"], "dashscope");
    assert_eq!(default_model["model"], "qwen");
}

#[tokio::test]
async fn inbound_agents_delete_preserves_device_local_config_sections() {
    let data = tempfile::tempdir().unwrap();
    let secret = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let config_path = data.path().join("config.json");
    rua_config::atomic_write_json(
        &config_path,
        &serde_json::json!({
            "agents": {
                "profiles": {"remote": {"id": "remote"}},
                "agent_order": ["remote"]
            },
            "channels": {
                "desktop-only": {"enabled": true}
            }
        }),
    )
    .unwrap();
    let adapter = Arc::new(RootConfigSyncAdapter::new(
        data.path().to_path_buf(),
        secret.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    let change = RootConfigSyncAdapter::delete_change(
        "op-agents-delete",
        ROOT_CONFIG_RECORD_AGENTS,
        "device-a",
        hlc(250),
    );

    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let file: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(config_path).unwrap()).unwrap();
    assert!(file.get("agents").is_none());
    assert_eq!(
        file["channels"]["desktop-only"]["enabled"],
        serde_json::json!(true)
    );
}

#[tokio::test]
async fn inbound_user_timezone_updates_only_config_timezone_field() {
    let data = tempfile::tempdir().unwrap();
    let secret = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let config_path = data.path().join("config.json");
    rua_config::atomic_write_json(
        &config_path,
        &serde_json::json!({
            "agents": {
                "profiles": {"local": {"id": "local"}},
                "agent_order": ["local"]
            },
            "channels": {"desktop": {"enabled": true}},
            "user_timezone": "UTC"
        }),
    )
    .unwrap();
    let adapter = Arc::new(RootConfigSyncAdapter::new(
        data.path().to_path_buf(),
        secret.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    let change = RootConfigSyncAdapter::upsert_change(
        "op-user-timezone",
        ROOT_CONFIG_RECORD_USER_TIMEZONE,
        serde_json::json!("Asia/Shanghai"),
        "device-a",
        hlc(260),
    );

    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let file: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    assert_eq!(file["user_timezone"], serde_json::json!("Asia/Shanghai"));
    assert_eq!(
        file["channels"]["desktop"]["enabled"],
        serde_json::json!(true)
    );
    assert_eq!(file["agents"]["profiles"]["local"]["id"], "local");
    let cursor = merge_store
        .get_record_cursor("root_config:user_timezone")
        .unwrap()
        .expect("user timezone cursor");
    assert_eq!(cursor.base_blob_hash, None);
}

#[tokio::test]
async fn inbound_user_timezone_delete_preserves_other_config_sections() {
    let data = tempfile::tempdir().unwrap();
    let secret = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let config_path = data.path().join("config.json");
    rua_config::atomic_write_json(
        &config_path,
        &serde_json::json!({
            "agents": {
                "profiles": {"local": {"id": "local"}},
                "agent_order": ["local"]
            },
            "channels": {"desktop": {"enabled": true}},
            "user_timezone": "UTC"
        }),
    )
    .unwrap();
    let adapter = Arc::new(RootConfigSyncAdapter::new(
        data.path().to_path_buf(),
        secret.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    let change = RootConfigSyncAdapter::delete_change(
        "op-user-timezone-delete",
        ROOT_CONFIG_RECORD_USER_TIMEZONE,
        "device-a",
        hlc(270),
    );

    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let file: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(config_path).unwrap()).unwrap();
    assert!(file.get("user_timezone").is_none());
    assert_eq!(
        file["channels"]["desktop"]["enabled"],
        serde_json::json!(true)
    );
    assert_eq!(file["agents"]["profiles"]["local"]["id"], "local");
}

#[tokio::test]
async fn inbound_security_record_merges_portable_policy_and_preserves_auth_bypass_hosts() {
    let data = tempfile::tempdir().unwrap();
    let secret = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let config_path = data.path().join("config.json");
    rua_config::atomic_write_json(
        &config_path,
        &serde_json::json!({
            "agents": {
                "profiles": {"local": {"id": "local"}},
                "agent_order": ["local"]
            },
            "security": {
                "file_guard": {"enabled": true},
                "allow_no_auth_hosts": ["127.0.0.1"]
            },
            "channels": {"desktop": {"enabled": true}}
        }),
    )
    .unwrap();
    let adapter = Arc::new(RootConfigSyncAdapter::new(
        data.path().to_path_buf(),
        secret.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    let change = RootConfigSyncAdapter::upsert_change(
        "op-security",
        ROOT_CONFIG_RECORD_SECURITY,
        serde_json::json!({
            "file_guard": {"enabled": false, "paths": ["**/.env"]},
            "skill_scanner": {"mode": "block"},
            "allow_no_auth_hosts": ["203.0.113.10"]
        }),
        "device-a",
        hlc(280),
    );

    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let file: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    assert_eq!(file["security"]["file_guard"]["enabled"], false);
    assert_eq!(
        file["security"]["file_guard"]["paths"],
        serde_json::json!(["**/.env"])
    );
    assert_eq!(file["security"]["skill_scanner"]["mode"], "block");
    assert_eq!(
        file["security"]["allow_no_auth_hosts"],
        serde_json::json!(["127.0.0.1"])
    );
    assert_eq!(
        file["channels"]["desktop"]["enabled"],
        serde_json::json!(true)
    );
    assert_eq!(file["agents"]["profiles"]["local"]["id"], "local");
}

#[tokio::test]
async fn inbound_security_delete_preserves_other_config_sections() {
    let data = tempfile::tempdir().unwrap();
    let secret = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let config_path = data.path().join("config.json");
    rua_config::atomic_write_json(
        &config_path,
        &serde_json::json!({
            "agents": {
                "profiles": {"local": {"id": "local"}},
                "agent_order": ["local"]
            },
            "security": {
                "file_guard": {"enabled": true},
                "allow_no_auth_hosts": ["127.0.0.1"]
            },
            "channels": {"desktop": {"enabled": true}}
        }),
    )
    .unwrap();
    let adapter = Arc::new(RootConfigSyncAdapter::new(
        data.path().to_path_buf(),
        secret.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    let change = RootConfigSyncAdapter::delete_change(
        "op-security-delete",
        ROOT_CONFIG_RECORD_SECURITY,
        "device-a",
        hlc(290),
    );

    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let file: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(config_path).unwrap()).unwrap();
    assert!(file.get("security").is_none());
    assert_eq!(
        file["channels"]["desktop"]["enabled"],
        serde_json::json!(true)
    );
    assert_eq!(file["agents"]["profiles"]["local"]["id"], "local");
}
