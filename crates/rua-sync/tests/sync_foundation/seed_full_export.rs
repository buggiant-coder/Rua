use std::sync::Arc;

use rua_sync::agent_adapter::AGENT_CONFIG_DOMAIN;
use rua_sync::agent_prompt_file_adapter::AGENT_PROMPT_FILE_DOMAIN;
use rua_sync::agent_template_adapter::AGENT_TEMPLATE_DOMAIN;
use rua_sync::crypto::SyncKey;
use rua_sync::prompt_adapter::PROMPT_DOMAIN;
use rua_sync::root_config_adapter::{ROOT_CONFIG_DOMAIN, ROOT_CONFIG_RECORD_AGENTS};
use rua_sync::skill_adapter::{workspace_record_id, SKILL_BLOB_DOMAIN};
use rua_sync::sync_registry::SyncRegistry;

#[tokio::test]
async fn full_export_filters_fresh_seed_and_exports_after_user_modification() {
    let data = tempfile::tempdir().unwrap();
    let secret = tempfile::tempdir().unwrap();
    write_seeded_device_state(data.path());

    let registry = build_registry(data.path(), secret.path()).unwrap();
    let exported = registry.export_all().await.unwrap();

    assert!(!has_change(&exported, PROMPT_DOMAIN, "seed.md"));
    assert!(!has_change(
        &exported,
        AGENT_PROMPT_FILE_DOMAIN,
        "default/root/AGENT.md"
    ));
    assert!(!has_change(&exported, AGENT_CONFIG_DOMAIN, "default"));
    assert!(!has_change(&exported, SKILL_BLOB_DOMAIN, "guidance"));
    assert!(!has_change(
        &exported,
        SKILL_BLOB_DOMAIN,
        &workspace_record_id("default", "guidance")
    ));
    assert!(!has_change(&exported, AGENT_TEMPLATE_DOMAIN, "default"));
    assert!(
        has_change(&exported, PROMPT_DOMAIN, "untracked.md"),
        "absence from Seed Manifest must not classify ordinary content as seed"
    );

    let root_agents = exported
        .iter()
        .find(|change| {
            change.domain == ROOT_CONFIG_DOMAIN && change.record_id == ROOT_CONFIG_RECORD_AGENTS
        })
        .expect("root agents projection snapshot");
    assert!(root_agents.payload["profiles"].get("default").is_none());
    assert_eq!(root_agents.payload["agent_order"], serde_json::json!([]));
    assert!(root_agents.payload.get("active_agent").is_none());

    write_file(data.path(), "prompt_library/seed.md", "custom prompt");
    write_json(
        data.path(),
        "workspaces/default/agent.json",
        &serde_json::json!({"id": "default", "name": "Customized"}),
    );
    write_file(
        data.path(),
        "workspaces/default/prompts/AGENT.md",
        "custom agent prompt",
    );
    write_file(
        data.path(),
        "skill_pool/guidance/SKILL.md",
        "custom pool skill",
    );
    write_file(
        data.path(),
        "workspaces/default/skills/guidance/SKILL.md",
        "custom workspace skill",
    );
    write_json(
        data.path(),
        "agent_templates.json",
        &serde_json::json!({
            "version": 1,
            "templates": {
                "default": {"name": "Customized"}
            }
        }),
    );

    let exported = registry.export_all().await.unwrap();
    let prompt = exported
        .iter()
        .find(|change| change.domain == PROMPT_DOMAIN && change.record_id == "seed.md")
        .expect("modified seeded prompt exports");
    assert_eq!(prompt.payload["content"], "custom prompt");
    assert!(has_change(&exported, AGENT_CONFIG_DOMAIN, "default"));
    assert!(has_change(
        &exported,
        AGENT_PROMPT_FILE_DOMAIN,
        "default/root/AGENT.md"
    ));
    assert!(has_change(&exported, SKILL_BLOB_DOMAIN, "guidance"));
    assert!(has_change(
        &exported,
        SKILL_BLOB_DOMAIN,
        &workspace_record_id("default", "guidance")
    ));
    assert!(has_change(&exported, AGENT_TEMPLATE_DOMAIN, "default"));
}

fn build_registry(
    data_dir: &std::path::Path,
    secret_dir: &std::path::Path,
) -> rua_sync::SyncResult<SyncRegistry> {
    let mut registry = SyncRegistry::new();
    registry.register(Arc::new(
        rua_sync::prompt_adapter::PromptLibrarySyncAdapter::new(data_dir.to_path_buf(), "device-a"),
    ))?;
    registry.register(Arc::new(
        rua_sync::agent_prompt_file_adapter::AgentPromptFileSyncAdapter::new(
            data_dir.to_path_buf(),
            "device-a",
        ),
    ))?;
    registry.register(Arc::new(
        rua_sync::agent_adapter::AgentConfigSyncAdapter::new(data_dir.to_path_buf(), "device-a"),
    ))?;
    registry.register(Arc::new(
        rua_sync::root_config_adapter::RootConfigSyncAdapter::new(
            data_dir.to_path_buf(),
            secret_dir.to_path_buf(),
            "device-a",
        ),
    ))?;
    registry.register(Arc::new(
        rua_sync::agent_template_adapter::AgentTemplateSyncAdapter::new(
            data_dir.to_path_buf(),
            "device-a",
        ),
    ))?;
    registry.register(Arc::new(
        rua_sync::skill_adapter::SkillBlobManifestSyncAdapter::new(
            data_dir.to_path_buf(),
            "device-a",
        )
        .with_blob_store(
            iroh_blobs::store::mem::MemStore::default().into(),
            SyncKey::from_bytes([7; 32]),
        ),
    ))?;
    Ok(registry)
}

fn write_seeded_device_state(data_dir: &std::path::Path) {
    write_file(data_dir, "prompt_library/seed.md", "seed prompt");
    write_file(data_dir, "prompt_library/untracked.md", "ordinary prompt");
    write_json(
        data_dir,
        "workspaces/default/agent.json",
        &serde_json::json!({"id": "default", "name": "Default"}),
    );
    write_file(
        data_dir,
        "workspaces/default/prompts/AGENT.md",
        "agent prompt",
    );
    write_file(data_dir, "skill_pool/guidance/SKILL.md", "pool skill");
    write_file(
        data_dir,
        "workspaces/default/skills/guidance/SKILL.md",
        "workspace skill",
    );
    write_json(
        data_dir,
        "agent_templates.json",
        &serde_json::json!({
            "version": 1,
            "templates": {
                "default": {"name": "Default"}
            }
        }),
    );
    write_json(
        data_dir,
        "config.json",
        &serde_json::json!({
            "agents": {
                "active_agent": "default",
                "profiles": {
                    "default": {"id": "default", "workspace_dir": "/tmp/default"}
                },
                "agent_order": ["default"]
            }
        }),
    );

    rua_core::seed_manifest::record_seed_entry(
        data_dir,
        "prompt_library:builtin",
        "builtin_prompt_library",
        &["prompt_library/seed.md"],
    )
    .unwrap();
    rua_core::seed_manifest::record_seed_entry(
        data_dir,
        "agent:default",
        "agent_seed_catalog",
        &[
            "workspaces/default/agent.json",
            "workspaces/default/prompts/AGENT.md",
        ],
    )
    .unwrap();
    rua_core::seed_manifest::record_seed_entry(
        data_dir,
        "skill_pool:guidance",
        "builtin_skill_pool",
        &["skill_pool/guidance/SKILL.md"],
    )
    .unwrap();
    rua_core::seed_manifest::record_seed_entry(
        data_dir,
        "workspace_skill:default:guidance",
        "agent_seed_catalog",
        &["workspaces/default/skills/guidance/SKILL.md"],
    )
    .unwrap();
    rua_core::seed_manifest::record_seed_entry(
        data_dir,
        "agent_templates:builtin",
        "agent_template_seed",
        &["agent_templates.json"],
    )
    .unwrap();
}

fn has_change(
    changes: &[rua_sync::opaque_replica_log::SyncChange],
    domain: &str,
    record_id: &str,
) -> bool {
    changes
        .iter()
        .any(|change| change.domain == domain && change.record_id == record_id)
}

fn write_file(data_dir: &std::path::Path, rel: &str, content: &str) {
    let path = data_dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn write_json(data_dir: &std::path::Path, rel: &str, value: &serde_json::Value) {
    let path = data_dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    rua_config::atomic_write_json(&path, value).unwrap();
}
