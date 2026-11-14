use std::fs;
use std::sync::Arc;

use rua_tool::{AgentToolsCells, ToolConfigStore};
use serde_json::json;
use tempfile::TempDir;

const TEST_DEVICE_KEY: [u8; 32] = [9; 32];

#[test]
fn tool_snapshot_freezes_behavior_and_resolves_only_secret_references() {
    let original = json!({"web_search": {"max_results": 3, "api_key": "private-original", "headers": {"Authorization": "Bearer original"}}});
    let snapshot = rua_tool::tool_config_secret_io::ToolConfigSnapshot::capture(&original);
    let serialized = serde_json::to_string(&snapshot).unwrap();
    assert!(!serialized.contains("private-original"));
    assert!(!serialized.contains("Bearer original"));
    let runtime = json!({"web_search": {"max_results": 99, "api_key": "current-key", "headers": {"Authorization": "Bearer current"}}});
    let resolved = snapshot.resolve(&runtime).unwrap();
    assert_eq!(resolved["web_search"]["max_results"], 3);
    assert_eq!(resolved["web_search"]["api_key"], "current-key");
    assert!(snapshot.resolve(&json!({})).is_err());
}

#[test]
fn tool_snapshot_secret_references_support_escaped_keys_and_arrays() {
    let original = json!({"tool/name": {"sources": [{"api_key": "secret", "count": 2}]}});
    let snapshot = rua_tool::tool_config_secret_io::ToolConfigSnapshot::capture(&original);
    assert_eq!(
        snapshot.secret_pointers,
        vec!["/tool~1name/sources/0/api_key"]
    );
    assert_eq!(snapshot.resolve(&original).unwrap(), original);
}

#[test]
fn tool_snapshot_rejects_non_secret_reference_and_non_null_placeholder() {
    let snapshot = rua_tool::tool_config_secret_io::ToolConfigSnapshot {
        values: json!({"web_search": {"max_results": null}}),
        secret_pointers: vec!["/web_search/max_results".into()],
        extra: Default::default(),
    };
    assert!(snapshot
        .resolve(&json!({"web_search": {"max_results": 100}}))
        .is_err());
    let snapshot = rua_tool::tool_config_secret_io::ToolConfigSnapshot {
        values: json!({"web_search": {"api_key": "unexpected"}}),
        secret_pointers: vec!["/web_search/api_key".into()],
        extra: Default::default(),
    };
    assert!(snapshot
        .resolve(&json!({"web_search": {"api_key": "secret"}}))
        .is_err());
}

fn make_store(tmp: &TempDir) -> ToolConfigStore {
    let cell_store = Arc::new(cell_store::FileSystemCellStore::new());
    let cells = Arc::new(AgentToolsCells::new(
        cell_store,
        tmp.path().join("workspaces"),
    ));
    ToolConfigStore::new(cells)
}

fn make_encrypted_store(tmp: &TempDir) -> ToolConfigStore {
    let cell_store = Arc::new(cell_store::FileSystemCellStore::new());
    let cells = Arc::new(AgentToolsCells::new(
        cell_store,
        tmp.path().join("workspaces"),
    ));
    ToolConfigStore::new(cells).with_device_key(TEST_DEVICE_KEY)
}

fn write_agent(tmp: &TempDir, agent_id: &str, builtin_tools: serde_json::Value) {
    let allowed_tools = builtin_tools
        .as_object()
        .unwrap()
        .iter()
        .filter(|(_, entry)| {
            entry
                .get("enabled")
                .and_then(|enabled| enabled.as_bool())
                .unwrap_or(true)
        })
        .map(|(name, _)| rua_config::types::tools::ToolIdentity::builtin(name))
        .collect();
    let policy = rua_config::types::tools::ToolSelectionPolicy::restricted(allowed_tools);

    let agent_dir = tmp.path().join("workspaces").join(agent_id);
    fs::create_dir_all(&agent_dir).expect("agent dir");
    fs::write(
        agent_dir.join("agent.json"),
        serde_json::to_string_pretty(&json!({
            "id": agent_id,
            "name": agent_id,
            "description": "",
            "workspace_dir": "",
            "language": "",
            "system_prompt_files": [],
            "tools": {
                "selection_policy": policy,
                "builtin_tools": builtin_tools,
            },
        }))
        .expect("agent json"),
    )
    .expect("write agent json");
}

#[tokio::test]
async fn agent_scoped_tool_list_returns_python_compatible_dto_shape() {
    let tmp = TempDir::new().expect("tempdir");
    write_agent(
        &tmp,
        "agent-a",
        json!(rua_config::types::tools::default_builtin_tools()),
    );
    let store = make_store(&tmp);

    let tools = store.list_tools(Some("agent-a")).await.expect("list tools");
    let bash = tools
        .iter()
        .find(|tool| tool.name == "bash")
        .expect("bash tool must be backfilled");
    let serialized = serde_json::to_value(bash).expect("ToolInfo serializes");

    assert_eq!(serialized["name"], "bash");
    assert!(serialized["description"].is_string());
    assert!(serialized["enabled"].is_boolean());
    assert!(serialized["async_execution"].is_boolean());
}

#[tokio::test]
async fn agent_scoped_tool_list_uses_canonical_description_over_stale_config() {
    let tmp = TempDir::new().expect("tempdir");
    write_agent(
        &tmp,
        "agent-a",
        json!({
            "task_read": {
                "name": "task_read",
                "description": "Read everything without bounds",
                "enabled": false,
                "async_execution": true
            }
        }),
    );
    let store = make_store(&tmp);

    let tools = store.list_tools(Some("agent-a")).await.expect("list tools");
    let task_read = tools
        .iter()
        .find(|tool| tool.name == "task_read")
        .expect("task_read tool");

    assert_eq!(
        task_read.description,
        "Read bounded output and current state from a Task"
    );
    assert!(!task_read.enabled);
    assert!(task_read.async_execution);
}

#[tokio::test]
async fn agent_scoped_enabled_toggle_roundtrips() {
    let tmp = TempDir::new().expect("tempdir");
    write_agent(
        &tmp,
        "agent-a",
        json!(rua_config::types::tools::default_builtin_tools()),
    );
    let store = make_store(&tmp);

    let baseline = store
        .list_tools(Some("agent-a"))
        .await
        .expect("list tools")
        .into_iter()
        .find(|tool| tool.name == "bash")
        .expect("bash tool")
        .enabled;

    let toggled = store
        .set_enabled(Some("agent-a"), "bash", !baseline)
        .await
        .expect("toggle enabled");
    assert_eq!(toggled.name, "bash");
    assert_eq!(toggled.enabled, !baseline);

    let restored = store
        .set_enabled(Some("agent-a"), "bash", baseline)
        .await
        .expect("restore enabled");
    assert_eq!(restored.enabled, baseline);
}

#[tokio::test]
async fn agent_scoped_async_execution_roundtrips() {
    let tmp = TempDir::new().expect("tempdir");
    write_agent(
        &tmp,
        "agent-a",
        json!(rua_config::types::tools::default_builtin_tools()),
    );
    let store = make_store(&tmp);

    let updated = store
        .set_async_execution(Some("agent-a"), "bash", true)
        .await
        .expect("set async execution");
    assert_eq!(updated.name, "bash");
    assert!(updated.async_execution);

    let tools = store.list_tools(Some("agent-a")).await.expect("list tools");
    let bash = tools
        .iter()
        .find(|tool| tool.name == "bash")
        .expect("bash tool");
    assert!(bash.async_execution);

    let restored = store
        .set_async_execution(Some("agent-a"), "bash", false)
        .await
        .expect("restore async execution");
    assert!(!restored.async_execution);
}

#[tokio::test]
async fn tool_config_roundtrips_with_masked_secret_preservation() {
    let tmp = TempDir::new().expect("tempdir");
    write_agent(
        &tmp,
        "agent-a",
        json!(rua_config::types::tools::default_builtin_tools()),
    );
    let store = make_encrypted_store(&tmp);

    let saved = store
        .set_config(
            Some("agent-a"),
            "web_search",
            json!({
                "mode": "ordered_fallback",
                "providers": [
                    {"id": "tavily", "enabled": true},
                    {"id": "brave", "enabled": true, "api_key": "brave-secret"}
                ]
            })
            .as_object()
            .cloned()
            .expect("config object"),
        )
        .await
        .expect("set config");
    let masked_key = saved["providers"][1]["api_key"]
        .as_str()
        .expect("masked api key")
        .to_string();
    assert!(masked_key.contains("***") || masked_key.contains("******"));

    let raw = fs::read_to_string(
        tmp.path()
            .join("workspaces")
            .join("agent-a")
            .join("agent.json"),
    )
    .expect("read agent");
    assert!(
        !raw.contains("brave-secret"),
        "secret must be encrypted on disk"
    );
    assert!(raw.contains("rua_encrypted"));

    let unmasked = store
        .get_config_unmasked(Some("agent-a"), "web_search")
        .await
        .expect("unmasked config");
    assert_eq!(unmasked["providers"][1]["api_key"], "brave-secret");

    let preserved = store
        .set_config(
            Some("agent-a"),
            "web_search",
            json!({
                "mode": "ordered_fallback",
                "providers": [
                    {"id": "tavily", "enabled": false},
                    {"id": "brave", "enabled": true, "api_key": masked_key}
                ]
            })
            .as_object()
            .cloned()
            .expect("config object"),
        )
        .await
        .expect("preserve masked config");
    assert_eq!(preserved["providers"][1]["api_key"], masked_key);

    let unmasked = store
        .get_config_unmasked(Some("agent-a"), "web_search")
        .await
        .expect("unmasked preserved config");
    assert_eq!(unmasked["providers"][1]["api_key"], "brave-secret");
    assert_eq!(unmasked["providers"][0]["enabled"], false);
}
