use std::fs;
use std::path::Path;
use std::sync::Arc;

use rua_agent::cells::AgentCells;
use rua_agent::profile_store::AgentProfileStore;
use serde_json::json;
use tempfile::TempDir;

fn write_agent_json(data_dir: &Path, agent_id: &str, value: serde_json::Value) {
    let path = data_dir
        .join("workspaces")
        .join(agent_id)
        .join("agent.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
}

fn make_cells(data_dir: &Path) -> AgentCells {
    let store = Arc::new(cell_store::FileSystemCellStore::new());
    AgentCells::new(store, data_dir.join("workspaces"))
}

#[tokio::test]
async fn tools_belong_to_the_target_agent_without_default_overlay() {
    let tmp = TempDir::new().unwrap();
    write_agent_json(
        tmp.path(),
        "default",
        json!({"id":"default", "name":"default", "tools":{"builtin_tools":{"bash":{"name":"bash", "enabled":true}}}}),
    );
    for tools in [
        None,
        Some(json!({})),
        Some(json!({"builtin_tools":{"read":{"name":"read", "enabled":true}}})),
    ] {
        let mut value = json!({"id":"alice", "name":"Alice"});
        if let Some(ref tools) = tools {
            value["tools"] = tools.clone();
        }
        write_agent_json(tmp.path(), "alice", value);
        let store = AgentProfileStore::new(tmp.path().to_path_buf());
        let profile = store
            .load_agent_config_via_cells(&make_cells(tmp.path()), Some("alice"))
            .await
            .unwrap();
        assert_eq!(profile.tools, tools);
    }
}

#[test]
fn resolve_agent_id_defaults_to_default() {
    assert_eq!(AgentProfileStore::resolve_agent_id(None), "default");
    assert_eq!(AgentProfileStore::resolve_agent_id(Some("alice")), "alice");
}

#[test]
fn agent_json_path_uses_workspaces_subtree() {
    let store = AgentProfileStore::new("/tmp/data".into());
    assert_eq!(
        store.agent_json_path(None),
        Path::new("/tmp/data/workspaces/default/agent.json")
    );
    assert_eq!(
        store.agent_json_path(Some("alice")),
        Path::new("/tmp/data/workspaces/alice/agent.json")
    );
}

#[tokio::test]
async fn loads_only_the_requested_agent_without_reading_default() {
    let tmp = TempDir::new().unwrap();
    write_agent_json(
        tmp.path(),
        "alice",
        json!({"id":"alice","name":"Alice","language":"zh", "default_model":null}),
    );
    write_agent_json(tmp.path(), "default", json!("invalid profile"));
    let store = AgentProfileStore::new(tmp.path().to_path_buf());
    let config = store
        .load_agent_config_via_cells(&make_cells(tmp.path()), Some("alice"))
        .await
        .unwrap();
    assert_eq!(config.id, "alice");
    assert_eq!(config.language, "zh");
    assert!(config.default_model.is_none());
}

#[tokio::test]
async fn missing_malformed_and_mismatched_agent_configs_are_errors_without_writes() {
    let tmp = TempDir::new().unwrap();
    let store = AgentProfileStore::new(tmp.path().to_path_buf());
    assert!(store
        .load_agent_config_via_cells(&make_cells(tmp.path()), None)
        .await
        .is_err());
    assert!(store
        .load_agent_config_via_cells(&make_cells(tmp.path()), Some("alice"))
        .await
        .is_err());
    for value in [
        json!(null),
        json!({}),
        json!({"id":"other"}),
        json!({"id":"alice","running":{"max_iters": "invalid"}}),
    ] {
        write_agent_json(tmp.path(), "alice", value);
        let path = tmp.path().join("workspaces/alice/agent.json");
        let before = fs::read(&path).unwrap();
        assert!(store
            .load_agent_config_via_cells(&make_cells(tmp.path()), Some("alice"))
            .await
            .is_err());
        assert_eq!(fs::read(path).unwrap(), before);
    }
}
