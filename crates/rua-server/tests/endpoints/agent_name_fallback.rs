use serde_json::json;
use tempfile::TempDir;

use rua_server::services::agent_manager::AgentManager;

fn write_config_json(tmp: &TempDir, profiles: serde_json::Value) {
    let config = json!({
        "agents": {
            "active_agent": "default",
            "agent_order": [],
            "profiles": profiles,
        }
    });
    rua_config::atomic_write_json(&tmp.path().join("config.json"), &config).unwrap();
}

fn seed_agent_json(tmp: &TempDir, id: &str, agent_json: serde_json::Value) {
    let dir = tmp.path().join("workspaces").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    rua_config::atomic_write_json(&dir.join("agent.json"), &agent_json).unwrap();
}

fn build_manager(tmp: &TempDir) -> AgentManager {
    AgentManager::new(tmp.path().to_path_buf())
}

#[tokio::test]
async fn registered_agent_missing_name_is_not_synthesized_from_id() {
    let tmp = TempDir::new().unwrap();
    let ws_dir = format!("{}/workspaces/qa", tmp.path().to_string_lossy());
    write_config_json(
        &tmp,
        json!({
            "qa": { "id": "qa", "workspace_dir": ws_dir }
        }),
    );

    seed_agent_json(
        &tmp,
        "qa",
        json!({
            "id": "qa",
            "description": "test agent",
            "workspace_dir": ws_dir,
        }),
    );

    let mgr = build_manager(&tmp);
    let agents = mgr.list_agents().await.unwrap();
    let qa = agents
        .iter()
        .find(|a| a.id == "qa")
        .expect("qa agent should be loaded");
    assert_eq!(
        qa.name, "",
        "an absent name must not be synthesized from the Agent ID"
    );
}

#[tokio::test]
async fn registered_agent_empty_name_is_preserved() {
    let tmp = TempDir::new().unwrap();
    let ws_dir = format!("{}/workspaces/zz_test_agent", tmp.path().to_string_lossy());
    write_config_json(
        &tmp,
        json!({
            "zz_test_agent": { "id": "zz_test_agent", "workspace_dir": ws_dir }
        }),
    );
    seed_agent_json(
        &tmp,
        "zz_test_agent",
        json!({
            "id": "zz_test_agent",
            "name": "",
            "description": "",
            "workspace_dir": ws_dir,
        }),
    );

    let mgr = build_manager(&tmp);
    let agents = mgr.list_agents().await.unwrap();
    let agent = agents
        .iter()
        .find(|a| a.id == "zz_test_agent")
        .expect("agent with empty name should still be loaded");
    assert_eq!(agent.name, "", "an explicitly empty name must be preserved");
}

#[tokio::test]
async fn registered_agent_corrupted_configuration_is_reported() {
    let tmp = TempDir::new().unwrap();
    let ws_dir = format!("{}/workspaces/bad_agent", tmp.path().to_string_lossy());
    write_config_json(
        &tmp,
        json!({
            "bad_agent": { "id": "bad_agent", "workspace_dir": ws_dir }
        }),
    );

    seed_agent_json(
        &tmp,
        "bad_agent",
        json!({
            "id": 42,
            "name": "bad",
            "description": "this should fail",
        }),
    );

    let mgr = build_manager(&tmp);
    let error = mgr.list_agents().await.unwrap_err();
    assert!(matches!(error, rua_core::RuaError::Config { .. }));
    let detail = error.to_string();
    assert!(detail.contains("bad_agent/agent.json"), "{detail}");
    assert!(detail.contains("invalid type"), "{detail}");
}

#[tokio::test]
async fn unregistered_workspace_is_not_discovered() {
    let tmp = TempDir::new().unwrap();
    write_config_json(&tmp, json!({}));

    seed_agent_json(
        &tmp,
        "qa",
        json!({
            "id": "qa",
            "description": "scanned agent",
            "workspace_dir": "",
        }),
    );

    let mgr = build_manager(&tmp);
    let agents = mgr.list_agents().await.unwrap();
    assert!(
        agents.is_empty(),
        "only explicitly registered Agents may be listed"
    );
}

#[tokio::test]
async fn missing_root_configuration_is_not_reconstructed_from_workspace() {
    let tmp = TempDir::new().unwrap();
    seed_agent_json(
        &tmp,
        "my_workspace",
        json!({
            "id": "",
            "name": "",
            "description": "agent with empty id and name",
            "workspace_dir": "",
        }),
    );

    let mgr = build_manager(&tmp);
    let error = mgr.list_agents().await.unwrap_err();
    assert!(matches!(error, rua_core::RuaError::Config { .. }));
    assert!(error.to_string().contains("config.json"), "{error}");
    assert!(!tmp.path().join("config.json").exists());
}

#[tokio::test]
async fn unregistered_corrupted_workspace_is_not_read() {
    let tmp = TempDir::new().unwrap();
    write_config_json(&tmp, json!({}));

    seed_agent_json(
        &tmp,
        "broken",
        json!({
            "id": 99,
            "name": "broken",
        }),
    );

    let mgr = build_manager(&tmp);
    let agents = mgr.list_agents().await.unwrap();
    assert!(
        agents.is_empty(),
        "unregistered workspace files must not participate in Agent loading"
    );
}

#[tokio::test]
async fn registered_agent_missing_configuration_is_reported() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().join("workspaces/missing");
    write_config_json(
        &tmp,
        json!({"missing": {"id": "missing", "workspace_dir": workspace}}),
    );
    let error = build_manager(&tmp).list_agents().await.unwrap_err();
    assert!(matches!(error, rua_core::RuaError::Config { .. }));
    let detail = error.to_string();
    assert!(detail.contains("missing/agent.json"), "{detail}");
    assert!(
        !workspace.exists(),
        "a read must not create missing Agent resources"
    );
}

#[tokio::test]
async fn registered_agent_identity_mismatch_is_reported() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().join("workspaces/registered");
    write_config_json(
        &tmp,
        json!({"registered": {"id": "registered", "workspace_dir": workspace}}),
    );
    seed_agent_json(
        &tmp,
        "registered",
        json!({"id": "different", "name": "Different", "workspace_dir": workspace}),
    );
    let error = build_manager(&tmp).list_agents().await.unwrap_err();
    assert!(matches!(error, rua_core::RuaError::Config { .. }));
    assert!(
        error
            .to_string()
            .contains("does not match its registered identity or workspace"),
        "{error}"
    );
}
