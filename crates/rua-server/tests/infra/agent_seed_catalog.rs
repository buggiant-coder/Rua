use rua_config::types::tools::{ToolIdentity, ToolSelectionPolicy};
use rua_server::seed::agents::{
    builtin_agent_seed_catalog, ensure_builtin_agents_seeded, AgentSeedToolPreset,
};
use tempfile::TempDir;

#[test]
fn builtin_agent_catalog_contains_default_qa_and_research_entries() {
    let catalog = builtin_agent_seed_catalog();
    let ids: Vec<_> = catalog.iter().map(|spec| spec.id).collect();

    assert!(ids.contains(&"default"));
    assert!(ids.contains(&"rua_qa"));
    assert!(ids.contains(&"research"));
    assert!(ids.contains(&"implementation"));
    assert!(catalog.iter().all(|spec| !spec.id.contains('.')));
    assert!(catalog.iter().any(
        |spec| spec.id == "rua_qa" && matches!(spec.tool_preset, AgentSeedToolPreset::Named(_))
    ));
    assert!(catalog.iter().any(|spec| spec.id == "research"
        && matches!(spec.tool_preset, AgentSeedToolPreset::ReadOnlyResearch)));
}

#[test]
fn builtin_agent_seed_materializes_catalog_entries_and_records_manifest() {
    let tmp = TempDir::new().unwrap();

    let report = ensure_builtin_agents_seeded(tmp.path());

    assert!(report
        .outcomes
        .iter()
        .any(|outcome| outcome.entry_id == "agent:default"));
    assert!(report
        .outcomes
        .iter()
        .any(|outcome| outcome.entry_id == "agent:rua_qa"));
    assert!(report
        .outcomes
        .iter()
        .any(|outcome| outcome.entry_id == "agent:research"));

    let default_agent = read_agent(tmp.path(), "default");
    assert_eq!(default_agent["id"], "default");
    assert_eq!(default_agent["name"], "小Rua");
    assert_eq!(
        default_agent["system_prompt_files"],
        serde_json::json!([
            "10_AGENT_SAFETY.md",
            "20_AGENT_WORKFLOW.md",
            "30_EXECUTION_REPORTING.md",
            "40_STRUCTURED_MEMORY.md",
            "50_AGENT_RESPONSE.md",
            "60_MARKDOWN_PLUS.md",
            "90_DEFAULT_PROFILE.md"
        ])
    );

    let research = read_agent(tmp.path(), "research");
    let implementation = read_agent(tmp.path(), "implementation");
    let implementation_policy: ToolSelectionPolicy =
        serde_json::from_value(implementation["tools"]["selection_policy"].clone()).unwrap();
    assert!(matches!(
        implementation_policy,
        ToolSelectionPolicy::Restricted { .. }
    ));
    for tool in ["read", "edit", "bash", "task_create", "task_read"] {
        assert!(
            implementation_policy.selects(&ToolIdentity::builtin(tool), None),
            "{tool}"
        );
    }
    for tool in ["chat_send", "sync_send", "memory_write"] {
        assert!(
            !implementation_policy.selects(&ToolIdentity::builtin(tool), None),
            "{tool}"
        );
    }
    assert!(implementation["systemPrompt"]
        .as_str()
        .unwrap()
        .contains("user-authorized"));
    assert_eq!(research["id"], "research");
    assert_eq!(research["name"], "Research Agent");
    assert_eq!(
        research["system_prompt_files"],
        serde_json::json!([
            "10_AGENT_SAFETY.md",
            "20_AGENT_WORKFLOW.md",
            "30_EXECUTION_REPORTING.md",
            "50_AGENT_RESPONSE.md",
            "60_MARKDOWN_PLUS.md",
            "93_RESEARCH_PROFILE.md"
        ])
    );
    let policy: ToolSelectionPolicy =
        serde_json::from_value(research["tools"]["selection_policy"].clone()).unwrap();
    assert!(matches!(policy, ToolSelectionPolicy::Restricted { .. }));
    for allowed in [
        "read",
        "grep",
        "glob",
        "web_search",
        "web_fetch",
        "conversation_search",
        "conversation_read_span",
    ] {
        assert!(
            policy.selects(&ToolIdentity::builtin(allowed), None),
            "{allowed} must be enabled"
        );
    }
    for forbidden in [
        "bash",
        "edit",
        "python",
        "memory_write",
        "memory_update",
        "memory_delete",
        "task_create",
        "chat_send",
        "chat_read",
        "chat_stop",
        "sync_ls",
        "sync_send",
    ] {
        assert!(
            !policy.selects(&ToolIdentity::builtin(forbidden), None),
            "{forbidden} must be disabled"
        );
    }
    assert!(!policy.selects(&ToolIdentity::builtin("future_write_tool"), None));
    assert!(research["tools"]["builtin_tools"]
        .as_object()
        .unwrap()
        .is_empty());
    let default_policy: ToolSelectionPolicy =
        serde_json::from_value(default_agent["tools"]["selection_policy"].clone()).unwrap();
    assert!(default_policy.selects(&ToolIdentity::builtin("future_tool"), None));

    let qa_agent = read_agent(tmp.path(), "rua_qa");
    assert_eq!(qa_agent["id"], "rua_qa");
    assert!(qa_agent["name"].as_str().unwrap_or("").contains("QA"));
    assert_eq!(
        qa_agent["system_prompt_files"],
        serde_json::json!([
            "10_AGENT_SAFETY.md",
            "20_AGENT_WORKFLOW.md",
            "30_EXECUTION_REPORTING.md",
            "40_STRUCTURED_MEMORY.md",
            "50_AGENT_RESPONSE.md",
            "60_MARKDOWN_PLUS.md",
            "91_QA_PROFILE.md",
            "92_QA_WORKFLOW.md"
        ])
    );

    assert!(rua_core::seed_manifest::is_entry_materialized(tmp.path(), "agent:default").unwrap());
    assert!(rua_core::seed_manifest::is_entry_materialized(tmp.path(), "agent:rua_qa").unwrap());
    assert!(rua_core::seed_manifest::is_file_untouched_seed(
        tmp.path(),
        "workspaces/default/agent.json"
    )
    .unwrap());
    assert!(rua_core::seed_manifest::is_file_untouched_seed(
        tmp.path(),
        "workspaces/rua_qa/agent.json"
    )
    .unwrap());

    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(tmp.path().join("config.json")).unwrap()).unwrap();
    assert!(config["agents"]["profiles"]["default"].is_object());
    assert!(config["agents"]["profiles"]["rua_qa"].is_object());
    assert!(config["agents"]["profiles"]["research"].is_object());
    assert!(config["agents"]["agent_order"]
        .as_array()
        .unwrap()
        .iter()
        .any(|id| id == "default"));
    assert!(config["agents"]["agent_order"]
        .as_array()
        .unwrap()
        .iter()
        .any(|id| id == "rua_qa"));
}

#[test]
fn builtin_agent_seed_copies_workspace_prompts_and_records_hashes() {
    let tmp = TempDir::new().unwrap();

    ensure_builtin_agents_seeded(tmp.path());

    let qa_prompt = tmp
        .path()
        .join("workspaces/rua_qa/prompts/92_QA_WORKFLOW.md");
    assert!(qa_prompt.is_file());
    assert!(rua_core::seed_manifest::is_file_untouched_seed(
        tmp.path(),
        "workspaces/rua_qa/prompts/92_QA_WORKFLOW.md"
    )
    .unwrap());
    assert!(rua_core::seed_manifest::is_file_untouched_seed(
        tmp.path(),
        "workspaces/default/prompts/10_AGENT_SAFETY.md"
    )
    .unwrap());
}

#[test]
fn builtin_agent_seed_installs_initial_workspace_skills_and_records_entries() {
    let tmp = TempDir::new().unwrap();

    ensure_builtin_agents_seeded(tmp.path());

    assert!(tmp
        .path()
        .join("workspaces/rua_qa/skills/guidance/SKILL.md")
        .is_file());
    assert!(rua_core::seed_manifest::is_entry_materialized(
        tmp.path(),
        "workspace_skill:rua_qa:guidance"
    )
    .unwrap());
    assert!(rua_core::seed_manifest::is_entity_untouched_seed(
        tmp.path(),
        "workspaces/rua_qa/skills/guidance"
    )
    .unwrap());
}

#[test]
fn builtin_agent_seed_does_not_claim_existing_agent_without_manifest_entry() {
    let tmp = TempDir::new().unwrap();
    let qa_dir = tmp.path().join("workspaces/rua_qa");
    std::fs::create_dir_all(&qa_dir).unwrap();
    std::fs::write(
        qa_dir.join("agent.json"),
        serde_json::json!({
            "id": "rua_qa",
            "name": "User QA"
        })
        .to_string(),
    )
    .unwrap();

    ensure_builtin_agents_seeded(tmp.path());

    let qa_agent = read_agent(tmp.path(), "rua_qa");
    assert_eq!(qa_agent["name"], "User QA");
    assert!(!rua_core::seed_manifest::is_entry_materialized(tmp.path(), "agent:rua_qa").unwrap());
}

#[test]
fn builtin_agent_seed_does_not_recreate_deleted_materialized_agent() {
    let tmp = TempDir::new().unwrap();
    ensure_builtin_agents_seeded(tmp.path());
    std::fs::remove_file(tmp.path().join("workspaces/default/agent.json")).unwrap();

    ensure_builtin_agents_seeded(tmp.path());

    assert!(!tmp.path().join("workspaces/default/agent.json").exists());
    assert!(rua_core::seed_manifest::is_entry_materialized(tmp.path(), "agent:default").unwrap());
}

#[test]
fn startup_preserves_existing_research_policy_even_when_seed_hash_matches() {
    let tmp = TempDir::new().unwrap();
    ensure_builtin_agents_seeded(tmp.path());
    let relative = "workspaces/research/agent.json";
    let path = tmp.path().join(relative);
    let mut previous = read_agent(tmp.path(), "research");
    previous["tools"]
        .as_object_mut()
        .unwrap()
        .remove("selection_policy");
    rua_config::atomic_write_json(&path, &previous).unwrap();
    rua_core::seed_manifest::record_seed_entry(
        tmp.path(),
        "agent:research",
        "agent_seed_catalog",
        &[relative],
    )
    .unwrap();
    let before = std::fs::read(&path).unwrap();
    ensure_builtin_agents_seeded(tmp.path());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    previous["name"] = serde_json::json!("User customized research");
    rua_config::atomic_write_json(&path, &previous).unwrap();
    ensure_builtin_agents_seeded(tmp.path());
    assert_eq!(read_agent(tmp.path(), "research"), previous);
}

#[test]
fn builtin_agent_seed_does_not_backfill_deleted_workspace_prompt_or_skill() {
    let tmp = TempDir::new().unwrap();
    ensure_builtin_agents_seeded(tmp.path());
    std::fs::remove_file(
        tmp.path()
            .join("workspaces/rua_qa/prompts/92_QA_WORKFLOW.md"),
    )
    .unwrap();
    std::fs::remove_dir_all(tmp.path().join("workspaces/rua_qa/skills/guidance")).unwrap();

    ensure_builtin_agents_seeded(tmp.path());

    assert!(!tmp
        .path()
        .join("workspaces/rua_qa/prompts/92_QA_WORKFLOW.md")
        .exists());
    assert!(!tmp
        .path()
        .join("workspaces/rua_qa/skills/guidance")
        .exists());
}

#[test]
fn builtin_agent_seed_preserves_old_prompt_selection_and_files_without_migration() {
    let tmp = TempDir::new().unwrap();
    ensure_builtin_agents_seeded(tmp.path());

    let default_path = tmp.path().join("workspaces/default/agent.json");
    let mut default_agent = read_agent(tmp.path(), "default");
    default_agent["system_prompt_files"] =
        serde_json::json!(["AGENT_SAFETY.md", "AGENT_zOther.md"]);
    std::fs::write(
        &default_path,
        serde_json::to_vec_pretty(&default_agent).unwrap(),
    )
    .unwrap();
    let default_prompt_dir = tmp.path().join("workspaces/default/prompts");
    std::fs::write(default_prompt_dir.join("AGENT_SAFETY.md"), "seeded safety").unwrap();
    std::fs::write(default_prompt_dir.join("AGENT_zOther.md"), "seeded other").unwrap();
    rua_core::seed_manifest::record_seed_entry(
        tmp.path(),
        "agent:default:legacy-prompts",
        "legacy_agent_seed",
        &[
            "workspaces/default/prompts/AGENT_SAFETY.md",
            "workspaces/default/prompts/AGENT_zOther.md",
        ],
    )
    .unwrap();
    std::fs::write(
        default_prompt_dir.join("AGENT_zOther.md"),
        "user edited other",
    )
    .unwrap();

    let qa_path = tmp.path().join("workspaces/rua_qa/agent.json");
    let mut qa_agent = read_agent(tmp.path(), "rua_qa");
    qa_agent["system_prompt_files"] = serde_json::json!(["CUSTOM.md"]);
    std::fs::write(&qa_path, serde_json::to_vec_pretty(&qa_agent).unwrap()).unwrap();

    let before = std::fs::read(&default_path).unwrap();
    let deleted = default_prompt_dir.join("90_DEFAULT_PROFILE.md");
    std::fs::remove_file(&deleted).unwrap();
    ensure_builtin_agents_seeded(tmp.path());

    assert_eq!(std::fs::read(&default_path).unwrap(), before);
    assert!(!deleted.exists());
    assert_eq!(
        std::fs::read_to_string(default_prompt_dir.join("AGENT_SAFETY.md")).unwrap(),
        "seeded safety"
    );
    assert_eq!(
        std::fs::read_to_string(default_prompt_dir.join("AGENT_zOther.md")).unwrap(),
        "user edited other"
    );
    assert_eq!(
        read_agent(tmp.path(), "rua_qa")["system_prompt_files"],
        serde_json::json!(["CUSTOM.md"])
    );
}

#[test]
fn builtin_agent_seed_does_not_rewrite_custom_agents_with_old_virtual_prompt_names() {
    let tmp = TempDir::new().unwrap();
    ensure_builtin_agents_seeded(tmp.path());

    let custom_dir = tmp.path().join("workspaces/custom");
    std::fs::create_dir_all(custom_dir.join("prompts")).unwrap();
    std::fs::write(
        custom_dir.join("agent.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "id": "custom",
            "name": "Custom",
            "workspace_dir": custom_dir,
            "system_prompt_files": [
                "__RUA_BUILTIN_SYSTEM_PROMPT__.md",
                "CUSTOM.md"
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    ensure_builtin_agents_seeded(tmp.path());

    assert_eq!(
        read_agent(tmp.path(), "custom")["system_prompt_files"],
        serde_json::json!(["__RUA_BUILTIN_SYSTEM_PROMPT__.md", "CUSTOM.md"])
    );
}

fn read_agent(data_dir: &std::path::Path, agent_id: &str) -> serde_json::Value {
    serde_json::from_slice(
        &std::fs::read(
            data_dir
                .join("workspaces")
                .join(agent_id)
                .join("agent.json"),
        )
        .unwrap(),
    )
    .unwrap()
}
