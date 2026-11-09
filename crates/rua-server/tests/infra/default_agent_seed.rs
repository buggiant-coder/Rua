use rua_server::seed::agents::ensure_builtin_agents_seeded;
use tempfile::TempDir;

#[test]
fn seeds_agent_json_and_config_when_missing() {
    let tmp = TempDir::new().unwrap();
    let data_dir = tmp.path();

    ensure_builtin_agents_seeded(data_dir);

    let agent_json = data_dir
        .join("workspaces")
        .join("default")
        .join("agent.json");
    assert!(
        agent_json.is_file(),
        "workspaces/default/agent.json missing"
    );

    let agent: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&agent_json).unwrap()).unwrap();
    assert_eq!(agent["id"], "default");
    assert_eq!(agent["name"], "小Rua");
    assert!(agent["workspace_dir"].is_string());

    let config_json = data_dir.join("config.json");
    let cfg: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&config_json).unwrap()).unwrap();
    assert!(
        cfg["agents"]["profiles"]["default"].is_object(),
        "config.json missing agents.profiles.default"
    );
    assert_eq!(cfg["agents"]["profiles"]["default"]["name"], "小Rua");
    assert_eq!(cfg["agents"]["active_agent"], "default");
    assert!(
        cfg["agents"]["agent_order"]
            .as_array()
            .map(|a| a.iter().any(|v| v == "default"))
            .unwrap_or(false),
        "agent_order must contain 'default'"
    );

    for sub in &["sessions", "skills", "prompts"] {
        assert!(
            data_dir.join("workspaces/default").join(sub).is_dir(),
            "workspaces/default/{sub} not created"
        );
    }
    assert!(
        !data_dir.join("workspaces/default/MEMORY").exists(),
        "Removed Memory Markdown directory must not be seeded"
    );
    assert!(
        !data_dir.join("workspaces/default/memory").exists(),
        "Removed lowercase Memory Markdown directory must not be seeded"
    );
    assert!(
        !data_dir.join("workspaces/default/MEMORY.md").exists(),
        "Removed Memory Markdown file must not be seeded"
    );
}

#[test]
fn idempotent_no_overwrite_of_existing_profile() {
    let tmp = TempDir::new().unwrap();
    let data_dir = tmp.path();

    let workspace = data_dir.join("workspaces").join("default");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        workspace.join("agent.json"),
        serde_json::json!({
            "id": "default",
            "name": "Custom Default",
            "description": "user-edited"
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        data_dir.join("config.json"),
        serde_json::json!({
            "agents": {
                "active_agent": "default",
                "agent_order": ["default"],
                "profiles": {
                    "default": {
                        "id": "default",
                        "workspace_dir": format!(
                            "{}/workspaces/default",
                            data_dir.display()
                        ),
                        "enabled": true
                    }
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    ensure_builtin_agents_seeded(data_dir);

    let agent: serde_json::Value =
        serde_json::from_slice(&std::fs::read(workspace.join("agent.json")).unwrap()).unwrap();
    assert_eq!(agent["name"], "Custom Default");
    assert_eq!(agent["description"], "user-edited");
}

#[test]
fn does_not_claim_existing_agent_json_when_profile_missing() {
    let tmp = TempDir::new().unwrap();
    let data_dir = tmp.path();
    let workspace = data_dir.join("workspaces").join("default");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        workspace.join("agent.json"),
        serde_json::json!({"id":"default","name":"existing"}).to_string(),
    )
    .unwrap();

    ensure_builtin_agents_seeded(data_dir);

    let agent: serde_json::Value =
        serde_json::from_slice(&std::fs::read(workspace.join("agent.json")).unwrap()).unwrap();
    assert_eq!(agent["name"], "existing");

    let cfg: serde_json::Value =
        serde_json::from_slice(&std::fs::read(data_dir.join("config.json")).unwrap()).unwrap();
    assert!(
        !cfg["agents"]["profiles"]["default"].is_object(),
        "occupied agent.json without manifest entry must not be claimed as seed"
    );
}
