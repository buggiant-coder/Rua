use std::fs;
use std::sync::Arc;

use rua_tool::{AgentToolsCells, ToolConfigStore};
use serde_json::json;
use tempfile::TempDir;

fn make_store(tmp: &TempDir) -> ToolConfigStore {
    let cell_store = Arc::new(cell_store::FileSystemCellStore::new());
    let cells = Arc::new(AgentToolsCells::new(
        cell_store,
        tmp.path().join("workspaces"),
    ));
    ToolConfigStore::new(cells)
}

fn write_default_agent(tmp: &TempDir, body: serde_json::Value) {
    let agent_dir = tmp.path().join("workspaces").join("default");
    fs::create_dir_all(&agent_dir).unwrap();
    let agent_json = agent_dir.join("agent.json");
    fs::write(&agent_json, serde_json::to_string_pretty(&body).unwrap()).unwrap();
}

fn read_default_agent(tmp: &TempDir) -> serde_json::Value {
    let path = tmp.path().join("workspaces/default/agent.json");
    let raw = fs::read_to_string(&path).unwrap();
    serde_json::from_str(&raw).unwrap()
}

fn agent_skeleton(builtin_tools: serde_json::Value) -> serde_json::Value {
    json!({
        "id": "default",
        "name": "default",
        "description": "",
        "workspace_dir": "",
        "language": "",
        "system_prompt_files": [],
        "tools": {
            "builtin_tools": builtin_tools,
        },
    })
}

fn entry(name: &str, enabled: bool) -> serde_json::Value {
    json!({
        "name": name,
        "enabled": enabled,
        "description": "",
        "display_to_user": true,
        "async_execution": false,
    })
}

#[tokio::test]
async fn missing_tools_never_inherit_defaults_or_write_configuration() {
    let tmp = TempDir::new().unwrap();
    write_default_agent(&tmp, agent_skeleton(json!({"bash":entry("bash", true)})));
    fs::write(tmp.path().join("config.json"), "not read as tool defaults").unwrap();
    let target = tmp.path().join("workspaces/target");
    fs::create_dir_all(&target).unwrap();
    for tools in [None, Some(json!({})), Some(json!({"builtin_tools":{}}))] {
        let mut agent = json!({"id":"target"});
        if let Some(tools) = tools {
            agent["tools"] = tools;
        }
        let bytes = serde_json::to_vec(&agent).unwrap();
        fs::write(target.join("agent.json"), &bytes).unwrap();
        let store = make_store(&tmp);
        assert!(store.list_tools(Some("target")).await.unwrap().is_empty());
        assert!(store
            .list_configs_unmasked(Some("target"))
            .await
            .unwrap()
            .is_empty());
        assert!(store
            .set_enabled(Some("target"), "bash", true)
            .await
            .is_err());
        assert_eq!(fs::read(target.join("agent.json")).unwrap(), bytes);
    }
}

#[tokio::test]
async fn set_enabled_persists_change_and_returns_updated_dto() {
    let tmp = TempDir::new().unwrap();
    write_default_agent(
        &tmp,
        agent_skeleton(json!(rua_config::types::tools::default_builtin_tools())),
    );
    let store = make_store(&tmp);

    let updated = store.set_enabled(None, "bash", false).await.unwrap();
    assert_eq!(updated.name, "bash");
    assert!(!updated.enabled);

    let on_disk = read_default_agent(&tmp);
    let bash = &on_disk["tools"]["builtin_tools"]["bash"];
    assert_eq!(bash["enabled"], false);
}

#[tokio::test]
async fn set_async_execution_persists_change() {
    let tmp = TempDir::new().unwrap();
    write_default_agent(
        &tmp,
        agent_skeleton(json!(rua_config::types::tools::default_builtin_tools())),
    );
    let store = make_store(&tmp);

    let updated = store.set_async_execution(None, "bash", true).await.unwrap();
    assert!(updated.async_execution);

    let on_disk = read_default_agent(&tmp);
    assert_eq!(
        on_disk["tools"]["builtin_tools"]["bash"]["async_execution"],
        true
    );
}

#[tokio::test]
async fn set_enabled_returns_not_found_for_unknown_tool() {
    let tmp = TempDir::new().unwrap();
    write_default_agent(
        &tmp,
        agent_skeleton(json!(rua_config::types::tools::default_builtin_tools())),
    );
    let store = make_store(&tmp);

    let err = store
        .set_enabled(None, "definitely_not_a_tool", false)
        .await
        .unwrap_err();
    assert!(matches!(err, rua_tool::ToolConfigError::NotFound(_)));
}

#[tokio::test]
async fn list_tools_prunes_unknown_entries_from_effective_response() {
    let tmp = TempDir::new().unwrap();
    write_default_agent(
        &tmp,
        agent_skeleton(json!({
            "bash": entry("bash", true),
            "totally_made_up_tool": entry("totally_made_up_tool", true),
        })),
    );

    let store = make_store(&tmp);
    let tools = store.list_tools(None).await.unwrap();

    assert!(tools.iter().any(|tool| tool.name == "bash"));
    assert!(!tools.iter().any(|tool| tool.name == "totally_made_up_tool"));
}

#[tokio::test]
async fn tools_section_round_trip() {
    let tmp = TempDir::new().unwrap();
    write_default_agent(
        &tmp,
        agent_skeleton(json!(rua_config::types::tools::default_builtin_tools())),
    );
    let store = make_store(&tmp);

    store.set_enabled(None, "bash", false).await.unwrap();

    let store2 = make_store(&tmp);
    let tools = store2.list_tools(None).await.unwrap();
    let bash = tools.iter().find(|t| t.name == "bash").unwrap();
    assert!(!bash.enabled, "round-trip: disabled state must persist");
}

#[tokio::test]
async fn tools_section_external_edit_takes_effect() {
    let tmp = TempDir::new().unwrap();
    write_default_agent(
        &tmp,
        agent_skeleton(json!(rua_config::types::tools::default_builtin_tools())),
    );
    let store = make_store(&tmp);

    let tools = store.list_tools(None).await.unwrap();
    let bash = tools.iter().find(|t| t.name == "bash").unwrap();
    assert!(bash.enabled);

    let mut on_disk = read_default_agent(&tmp);
    on_disk["tools"]["builtin_tools"]["bash"]["enabled"] = json!(false);
    let path = tmp.path().join("workspaces/default/agent.json");
    fs::write(&path, serde_json::to_string_pretty(&on_disk).unwrap()).unwrap();

    let store2 = make_store(&tmp);
    let tools = store2.list_tools(None).await.unwrap();
    let bash = tools.iter().find(|t| t.name == "bash").unwrap();
    assert!(!bash.enabled, "external edit must be visible");
}

#[tokio::test]
async fn tools_section_does_not_affect_other_sections() {
    let tmp = TempDir::new().unwrap();
    let body = json!({
        "id": "default",
        "name": "default",
        "description": "test agent",
        "workspace_dir": "",
        "language": "en",
        "system_prompt_files": [],
        "channels": {"telegram": {}},
        "mcp": {"clients": {"foo": {}}},
        "tools": {"builtin_tools": {"bash":entry("bash", true)}},
    });
    write_default_agent(&tmp, body);
    let store = make_store(&tmp);

    store.set_enabled(None, "bash", false).await.unwrap();

    let on_disk = read_default_agent(&tmp);
    assert_eq!(on_disk["channels"]["telegram"], json!({}));
    assert_eq!(on_disk["mcp"]["clients"]["foo"], json!({}));
    assert_eq!(on_disk["language"], "en");
    assert_eq!(on_disk["description"], "test agent");
}

#[tokio::test]
async fn display_group_is_always_resolved_from_catalog_not_persisted_value() {
    let tmp = TempDir::new().unwrap();
    let body = agent_skeleton(json!({
        "bash": {
            "name": "bash",
            "enabled": true,
            "description": "shell",
            "display_to_user": true,
            "async_execution": false,
            // Attempted user override — must be ignored.
            "display_group": "hacked"
        }
    }));
    write_default_agent(&tmp, body);
    let store = make_store(&tmp);

    let listed = store.list_tools(None).await.unwrap();
    let bash = listed.iter().find(|tool| tool.name == "bash").unwrap();
    assert_eq!(
        bash.display_group, "shell",
        "display_group must be sourced from the canonical catalog, not user extras"
    );

    // set_enabled path also returns catalog-sourced display_group.
    let updated = store.set_enabled(None, "bash", false).await.unwrap();
    assert_eq!(updated.display_group, "shell");
}
