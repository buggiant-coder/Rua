use std::sync::Arc;

use cell_store::FileSystemCellStore;
use rua_agent::AgentCells;
use serde_json::{json, Value};
use tempfile::TempDir;

fn build_cells(tmp: &TempDir) -> AgentCells {
    let store = Arc::new(FileSystemCellStore::new());
    let workspaces = tmp.path().join("workspaces");
    AgentCells::new(store, workspaces)
}

fn seed_agent_json(tmp: &TempDir, agent_id: &str, value: Value) {
    let dir = tmp.path().join("workspaces").join(agent_id);
    std::fs::create_dir_all(&dir).expect("create workspace dir");
    let path = dir.join("agent.json");
    std::fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).expect("write agent.json");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn channels_section_round_trip() {
    let tmp = TempDir::new().unwrap();
    let cells = build_cells(&tmp);

    let initial = json!({
        "id": "test-agent",
        "name": "Test",
        "channels": {
            "dingtalk": {"client_id": "abc", "client_secret": "xyz"},
            "telegram": {"bot_token": "tok", "chat_id": "123"}
        }
    });
    seed_agent_json(&tmp, "test-agent", initial);

    let cell = cells.section::<Value>("test-agent", "channels");

    let read = cell.read().await.unwrap().unwrap();
    let map = read.as_object().unwrap();
    assert!(map.contains_key("dingtalk"));
    assert!(map.contains_key("telegram"));
    assert_eq!(map["dingtalk"]["client_id"], "abc");

    let updated = json!({
        "dingtalk": {"client_id": "new_id", "client_secret": "new_secret"},
        "feishu": {"webhook_url": "https://open.feishu.cn/hook/xxx"}
    });
    cell.write(updated.clone()).await.unwrap();

    let re_read = cell.read().await.unwrap().unwrap();
    let map2 = re_read.as_object().unwrap();
    assert_eq!(map2["dingtalk"]["client_id"], "new_id");
    assert!(map2.contains_key("feishu"));
    assert!(!map2.contains_key("telegram"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn channels_external_edit_takes_effect() {
    let tmp = TempDir::new().unwrap();
    let cells = build_cells(&tmp);

    let initial = json!({
        "id": "ext-agent",
        "name": "Ext",
        "channels": {"console": {"enabled": true}}
    });
    seed_agent_json(&tmp, "ext-agent", initial);

    let cell = cells.section::<Value>("ext-agent", "channels");
    let v1 = cell.read().await.unwrap().unwrap();
    assert_eq!(v1["console"]["enabled"], true);

    let path = tmp.path().join("workspaces/ext-agent/agent.json");
    let raw = std::fs::read_to_string(&path).unwrap();
    let mut doc: Value = serde_json::from_str(&raw).unwrap();
    doc["channels"] = json!({"discord": {"webhook_url": "https://disc.gg/wh"}});
    std::fs::write(&path, serde_json::to_string_pretty(&doc).unwrap()).unwrap();

    let v2 = cell.read().await.unwrap().unwrap();
    assert!(v2.get("discord").is_some());
    assert!(v2.get("console").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn channels_does_not_affect_other_sections() {
    let tmp = TempDir::new().unwrap();
    let cells = build_cells(&tmp);

    let initial = json!({
        "id": "multi",
        "name": "Multi",
        "channels": {"dingtalk": {"client_id": "old"}},
        "mcp": {"clients": {"server1": {"command": "npx"}}},
        "tools": {"builtin_tools": {"web_search": true}}
    });
    seed_agent_json(&tmp, "multi", initial);

    let cell = cells.section::<Value>("multi", "channels");
    cell.write(json!({"telegram": {"bot_token": "t123", "chat_id": "c456"}}))
        .await
        .unwrap();

    let path = tmp.path().join("workspaces/multi/agent.json");
    let raw = std::fs::read_to_string(&path).unwrap();
    let doc: Value = serde_json::from_str(&raw).unwrap();

    assert_eq!(doc["mcp"]["clients"]["server1"]["command"], "npx");
    assert_eq!(doc["tools"]["builtin_tools"]["web_search"], true);
    assert_eq!(doc["name"], "Multi");

    assert_eq!(doc["channels"]["telegram"]["bot_token"], "t123");
    assert!(doc["channels"].get("dingtalk").is_none());
}
