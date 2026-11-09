use std::path::PathBuf;
use std::sync::Arc;

use cell_store::FileSystemCellStore;
use rua_agent::AgentCells;
use rua_config::AgentProfileConfig;
use serde::{Deserialize, Serialize};
use tempfile::TempDir;

fn build_cells(tmp: &TempDir) -> AgentCells {
    let store = Arc::new(FileSystemCellStore::new());
    let workspaces = tmp.path().join("workspaces");
    AgentCells::new(store, workspaces)
}

fn write_agent_json(tmp: &TempDir, id: &str, value: serde_json::Value) -> PathBuf {
    let dir = tmp.path().join("workspaces").join(id);
    std::fs::create_dir_all(&dir).expect("create workspace dir");
    let path = dir.join("agent.json");
    std::fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).expect("write agent.json");
    path
}

fn sample_config(id: &str) -> AgentProfileConfig {
    AgentProfileConfig {
        id: id.to_string(),
        name: format!("{id}-name"),
        description: format!("desc-of-{id}"),
        workspace_dir: format!("/tmp/{id}"),
        template_id: Some("tpl-001".to_string()),
        language: "zh".to_string(),
        system_prompt_files: vec!["AGENT.md".to_string()],
        running: Default::default(),
        default_model: None,
        channels: Some(serde_json::json!({"console": {"enabled": true}})),
        mcp: Some(serde_json::json!({"clients": {}})),
        heartbeat: None,
        last_dispatch: None,
        tools: Some(serde_json::json!({"builtin_tools": {}})),
        extra: {
            let mut m = serde_json::Map::new();
            m.insert("custom_field".into(), serde_json::json!(42));
            m
        },
    }
}

#[tokio::test]
async fn full_read_returns_none_for_missing_agent() {
    let tmp = TempDir::new().unwrap();
    let cells = build_cells(&tmp);
    let result = cells.full("does-not-exist").read().await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn full_write_persists_full_config() {
    let tmp = TempDir::new().unwrap();
    let cells = build_cells(&tmp);
    let cfg = sample_config("alice");
    cells.full("alice").write(cfg.clone()).await.unwrap();

    let path = tmp.path().join("workspaces/alice/agent.json");
    let raw = std::fs::read_to_string(&path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["id"], "alice");
    assert_eq!(parsed["name"], "alice-name");
    assert_eq!(parsed["custom_field"], 42);

    let read = cells.full("alice").read().await.unwrap().unwrap();
    assert_eq!(read.id, cfg.id);
    assert_eq!(read.name, cfg.name);
    assert_eq!(read.template_id, cfg.template_id);
    assert_eq!(
        read.extra.get("custom_field"),
        cfg.extra.get("custom_field")
    );
}

#[tokio::test]
async fn full_update_atomic_mutator() {
    let tmp = TempDir::new().unwrap();
    let cells = build_cells(&tmp);
    cells.full("bob").write(sample_config("bob")).await.unwrap();

    let updated = cells
        .full("bob")
        .update(|current| {
            let mut cfg = current.unwrap();
            cfg.name = "bob-renamed".into();
            cfg
        })
        .await
        .unwrap();
    assert_eq!(updated.name, "bob-renamed");

    let read = cells.full("bob").read().await.unwrap().unwrap();
    assert_eq!(read.name, "bob-renamed");
}

#[tokio::test]
async fn full_delete_removes_file() {
    let tmp = TempDir::new().unwrap();
    let cells = build_cells(&tmp);
    cells
        .full("doomed")
        .write(sample_config("doomed"))
        .await
        .unwrap();
    let path = tmp.path().join("workspaces/doomed/agent.json");
    assert!(path.exists());

    cells.full("doomed").delete().await.unwrap();
    assert!(!path.exists());
    assert!(cells.full("doomed").read().await.unwrap().is_none());
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Default)]
struct McpSection {
    #[serde(default)]
    clients: std::collections::BTreeMap<String, String>,
}

#[tokio::test]
async fn section_read_extracts_subobject() {
    let tmp = TempDir::new().unwrap();
    let cells = build_cells(&tmp);
    write_agent_json(
        &tmp,
        "carol",
        serde_json::json!({
            "id": "carol",
            "name": "carol",
            "mcp": {
                "clients": {
                    "weather-mcp": "stdio:weather",
                    "search-mcp": "http://localhost:9000"
                }
            },
            "channels": {"console": {"enabled": true}}
        }),
    );

    let mcp: McpSection = cells
        .section::<McpSection>("carol", "mcp")
        .read()
        .await
        .unwrap()
        .expect("mcp section present");
    assert_eq!(mcp.clients.len(), 2);
    assert_eq!(mcp.clients.get("weather-mcp").unwrap(), "stdio:weather");
}

#[tokio::test]
async fn section_write_modifies_only_target_section() {
    let tmp = TempDir::new().unwrap();
    let cells = build_cells(&tmp);
    cells
        .full("dave")
        .write(sample_config("dave"))
        .await
        .unwrap();

    let new_mcp = serde_json::json!({"clients": {"only-mcp": "stdio:only"}});
    cells
        .section::<serde_json::Value>("dave", "mcp")
        .write(new_mcp.clone())
        .await
        .unwrap();

    let path = tmp.path().join("workspaces/dave/agent.json");
    let raw = std::fs::read_to_string(&path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["mcp"], new_mcp);
    assert_eq!(parsed["name"], "dave-name");
    assert_eq!(parsed["channels"]["console"]["enabled"], true);
    assert_eq!(parsed["custom_field"], 42);
}

#[tokio::test]
async fn section_update_atomic_mutator() {
    let tmp = TempDir::new().unwrap();
    let cells = build_cells(&tmp);
    write_agent_json(
        &tmp,
        "eve",
        serde_json::json!({
            "id": "eve",
            "name": "eve",
            "mcp": {"clients": {"first": "stdio:a"}}
        }),
    );

    let updated: McpSection = cells
        .section::<McpSection>("eve", "mcp")
        .update(|current| {
            let mut m = current.unwrap_or_default();
            m.clients.insert("second".into(), "stdio:b".into());
            m
        })
        .await
        .unwrap();
    assert_eq!(updated.clients.len(), 2);

    let reread: McpSection = cells
        .section::<McpSection>("eve", "mcp")
        .read()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reread, updated);
}

#[tokio::test]
async fn section_name_with_dot_returns_typed_error() {
    let tmp = TempDir::new().unwrap();
    let cells = build_cells(&tmp);
    cells
        .full("frank")
        .write(sample_config("frank"))
        .await
        .unwrap();

    let err = cells
        .section::<serde_json::Value>("frank", "mcp.clients")
        .read()
        .await
        .unwrap_err();
    let msg = err.user_message();
    assert!(
        msg.contains("invalid character") && msg.contains('.'),
        "got: {msg}"
    );

    let err2 = cells
        .section::<serde_json::Value>("frank", "")
        .read()
        .await
        .unwrap_err();
    assert!(err2.user_message().contains("must not be empty"));

    let err3 = cells
        .section::<serde_json::Value>("frank", "mcp clients")
        .read()
        .await
        .unwrap_err();
    assert!(err3.user_message().contains("invalid character"));
}

#[tokio::test]
async fn list_returns_all_agent_ids_in_workspaces() {
    let tmp = TempDir::new().unwrap();
    let cells = build_cells(&tmp);

    assert!(cells.list().await.unwrap().is_empty());

    write_agent_json(
        &tmp,
        "default",
        serde_json::json!({"id": "default", "name": "default"}),
    );
    write_agent_json(
        &tmp,
        "alpha",
        serde_json::json!({"id": "alpha", "name": "alpha"}),
    );
    write_agent_json(
        &tmp,
        "beta",
        serde_json::json!({"id": "beta", "name": "beta"}),
    );

    std::fs::create_dir_all(tmp.path().join("workspaces/no-agent-json")).unwrap();

    let ids = cells.list().await.unwrap();
    assert_eq!(ids, vec!["alpha", "beta", "default"]);
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
struct HeartbeatSection {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    every: String,
    #[serde(default)]
    target: String,
    #[serde(default)]
    active_hours: Option<serde_json::Value>,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

#[tokio::test]
async fn section_heartbeat_round_trip() {
    let tmp = TempDir::new().unwrap();
    let cells = build_cells(&tmp);
    write_agent_json(
        &tmp,
        "hb-agent",
        serde_json::json!({
            "id": "hb-agent",
            "name": "hb-agent",
            "heartbeat": {
                "enabled": true,
                "every": "30m",
                "target": "wechat",
                "active_hours": {"start": "09:00", "end": "18:00"}
            },
            "channels": {"console": {"enabled": true}}
        }),
    );

    let hb: HeartbeatSection = cells
        .section::<HeartbeatSection>("hb-agent", "heartbeat")
        .read()
        .await
        .unwrap()
        .expect("heartbeat section present");
    assert!(hb.enabled);
    assert_eq!(hb.every, "30m");
    assert_eq!(hb.target, "wechat");

    let new_hb = HeartbeatSection {
        enabled: false,
        every: "1h".into(),
        target: "email".into(),
        active_hours: None,
        extra: Default::default(),
    };
    cells
        .section::<HeartbeatSection>("hb-agent", "heartbeat")
        .write(new_hb.clone())
        .await
        .unwrap();

    let read_back: HeartbeatSection = cells
        .section::<HeartbeatSection>("hb-agent", "heartbeat")
        .read()
        .await
        .unwrap()
        .unwrap();
    assert!(!read_back.enabled);
    assert_eq!(read_back.every, "1h");
    assert_eq!(read_back.target, "email");

    let raw = std::fs::read_to_string(tmp.path().join("workspaces/hb-agent/agent.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["channels"]["console"]["enabled"], true);
}

#[tokio::test]
async fn section_template_id_round_trip() {
    let tmp = TempDir::new().unwrap();
    let cells = build_cells(&tmp);
    write_agent_json(
        &tmp,
        "tpl-agent",
        serde_json::json!({
            "id": "tpl-agent",
            "name": "tpl-agent",
            "template_id": "original-template",
            "heartbeat": {"enabled": true}
        }),
    );

    let tpl: String = cells
        .section::<String>("tpl-agent", "template_id")
        .read()
        .await
        .unwrap()
        .expect("template_id present");
    assert_eq!(tpl, "original-template");

    cells
        .section::<String>("tpl-agent", "template_id")
        .write("new-template".to_string())
        .await
        .unwrap();

    let read_back: String = cells
        .section::<String>("tpl-agent", "template_id")
        .read()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read_back, "new-template");

    let raw = std::fs::read_to_string(tmp.path().join("workspaces/tpl-agent/agent.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["heartbeat"]["enabled"], true);
    assert_eq!(parsed["template_id"], "new-template");
}

#[tokio::test]
async fn section_last_dispatch_round_trip() {
    let tmp = TempDir::new().unwrap();
    let cells = build_cells(&tmp);
    let dispatch_val = serde_json::json!({
        "timestamp": "2025-05-27T10:00:00Z",
        "task_id": "task-42"
    });
    write_agent_json(
        &tmp,
        "ld-agent",
        serde_json::json!({
            "id": "ld-agent",
            "name": "ld-agent",
            "last_dispatch": dispatch_val,
            "template_id": "tpl-keep"
        }),
    );

    let ld: serde_json::Value = cells
        .section::<serde_json::Value>("ld-agent", "last_dispatch")
        .read()
        .await
        .unwrap()
        .expect("last_dispatch present");
    assert_eq!(ld["timestamp"], "2025-05-27T10:00:00Z");
    assert_eq!(ld["task_id"], "task-42");

    let new_dispatch = serde_json::json!({
        "timestamp": "2025-05-27T12:00:00Z",
        "task_id": "task-99"
    });
    cells
        .section::<serde_json::Value>("ld-agent", "last_dispatch")
        .write(new_dispatch.clone())
        .await
        .unwrap();

    let read_back: serde_json::Value = cells
        .section::<serde_json::Value>("ld-agent", "last_dispatch")
        .read()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read_back, new_dispatch);

    let raw = std::fs::read_to_string(tmp.path().join("workspaces/ld-agent/agent.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["template_id"], "tpl-keep");
}

#[tokio::test]
async fn section_llm_routing_round_trip() {
    let tmp = TempDir::new().unwrap();
    let cells = build_cells(&tmp);
    let routing = serde_json::json!({
        "default_provider": "openai",
        "fallback": ["azure", "ollama"]
    });
    write_agent_json(
        &tmp,
        "lr-agent",
        serde_json::json!({
            "id": "lr-agent",
            "name": "lr-agent",
            "llm_routing": routing,
            "heartbeat": {"enabled": false}
        }),
    );

    let lr: serde_json::Value = cells
        .section::<serde_json::Value>("lr-agent", "llm_routing")
        .read()
        .await
        .unwrap()
        .expect("llm_routing present");
    assert_eq!(lr["default_provider"], "openai");
    assert_eq!(lr["fallback"][0], "azure");

    let new_routing = serde_json::json!({
        "default_provider": "azure",
        "fallback": ["openai"]
    });
    cells
        .section::<serde_json::Value>("lr-agent", "llm_routing")
        .write(new_routing.clone())
        .await
        .unwrap();

    let read_back: serde_json::Value = cells
        .section::<serde_json::Value>("lr-agent", "llm_routing")
        .read()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read_back, new_routing);

    let raw = std::fs::read_to_string(tmp.path().join("workspaces/lr-agent/agent.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["heartbeat"]["enabled"], false);
    assert_eq!(parsed["llm_routing"]["default_provider"], "azure");
}

#[tokio::test]
async fn agent_profile_config_serde_round_trip_preserves_extra_flatten() {
    let tmp = TempDir::new().unwrap();
    let cells = build_cells(&tmp);
    let mut cfg = sample_config("rich");
    cfg.extra.insert(
        "nested_extra".into(),
        serde_json::json!({"a": 1, "b": [true, false]}),
    );
    cfg.extra
        .insert("scalar_extra".into(), serde_json::json!("hello"));

    cells.full("rich").write(cfg.clone()).await.unwrap();
    let read = cells.full("rich").read().await.unwrap().unwrap();

    assert_eq!(
        read.extra.get("custom_field"),
        cfg.extra.get("custom_field")
    );
    assert_eq!(
        read.extra.get("nested_extra"),
        cfg.extra.get("nested_extra")
    );
    assert_eq!(
        read.extra.get("scalar_extra"),
        cfg.extra.get("scalar_extra")
    );

    let raw_path = tmp.path().join("workspaces/rich/agent.json");
    let raw = std::fs::read_to_string(&raw_path).unwrap();
    let raw_value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(raw_value["nested_extra"]["a"], 1);
    assert_eq!(raw_value["scalar_extra"], "hello");
}

#[tokio::test]
async fn running_section_round_trip() {
    let tmp = TempDir::new().unwrap();
    let cells = build_cells(&tmp);
    write_agent_json(
        &tmp,
        "run-agent",
        serde_json::json!({
            "id": "run-agent",
            "name": "run-agent",
            "running": {
                "max_iters": 50,
                "llm_retry_enabled": true,
                "llm_max_retries": 3
            },
            "channels": {"console": {"enabled": true}}
        }),
    );

    let running: rua_config::AgentsRunningConfig = cells
        .section::<rua_config::AgentsRunningConfig>("run-agent", "running")
        .read()
        .await
        .unwrap()
        .expect("running section present");
    assert_eq!(running.max_iters, 50);
    assert!(running.llm_retry_enabled);
    assert_eq!(running.llm_max_retries, 3);

    let new_running = rua_config::AgentsRunningConfig {
        max_iters: 200,
        auto_continue_on_text_only: true,
        ..Default::default()
    };
    cells
        .section::<rua_config::AgentsRunningConfig>("run-agent", "running")
        .write(new_running)
        .await
        .unwrap();

    let read_back: rua_config::AgentsRunningConfig = cells
        .section::<rua_config::AgentsRunningConfig>("run-agent", "running")
        .read()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read_back.max_iters, 200);
    assert!(read_back.auto_continue_on_text_only);

    let raw = std::fs::read_to_string(tmp.path().join("workspaces/run-agent/agent.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["channels"]["console"]["enabled"], true);
}

#[tokio::test]
async fn running_external_edit_takes_effect_on_next_read() {
    let tmp = TempDir::new().unwrap();
    let cells = build_cells(&tmp);
    write_agent_json(
        &tmp,
        "ext-run",
        serde_json::json!({
            "id": "ext-run",
            "name": "ext-run",
            "running": {"max_iters": 10}
        }),
    );

    let r1: rua_config::AgentsRunningConfig = cells
        .section::<rua_config::AgentsRunningConfig>("ext-run", "running")
        .read()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r1.max_iters, 10);

    let path = tmp.path().join("workspaces/ext-run/agent.json");
    let mut doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    doc["running"]["max_iters"] = serde_json::json!(999);
    std::fs::write(&path, serde_json::to_string_pretty(&doc).unwrap()).unwrap();

    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(2);
    let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    f.set_modified(later).unwrap();

    let r2: rua_config::AgentsRunningConfig = cells
        .section::<rua_config::AgentsRunningConfig>("ext-run", "running")
        .read()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r2.max_iters, 999);
}

#[tokio::test]
async fn default_model_section_round_trip() {
    let tmp = TempDir::new().unwrap();
    let cells = build_cells(&tmp);
    write_agent_json(
        &tmp,
        "am-agent",
        serde_json::json!({
            "id": "am-agent",
            "name": "am-agent",
            "default_model": {
                "provider_id": "openai",
                "model": "gpt-4o"
            },
            "running": {"max_iters": 100}
        }),
    );

    let am: rua_config::ModelSlotConfig = cells
        .section::<rua_config::ModelSlotConfig>("am-agent", "default_model")
        .read()
        .await
        .unwrap()
        .expect("default_model section present");
    assert_eq!(am.provider_id, "openai");
    assert_eq!(am.model, "gpt-4o");

    let new_am = rua_config::ModelSlotConfig {
        provider_id: "azure".into(),
        model: "gpt-4-turbo".into(),
        extra: Default::default(),
    };
    cells
        .section::<rua_config::ModelSlotConfig>("am-agent", "default_model")
        .write(new_am)
        .await
        .unwrap();

    let read_back: rua_config::ModelSlotConfig = cells
        .section::<rua_config::ModelSlotConfig>("am-agent", "default_model")
        .read()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read_back.provider_id, "azure");
    assert_eq!(read_back.model, "gpt-4-turbo");

    let raw = std::fs::read_to_string(tmp.path().join("workspaces/am-agent/agent.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["running"]["max_iters"], 100);
}

#[tokio::test]
async fn default_model_set_then_simulated_restart_keeps_value() {
    let tmp = TempDir::new().unwrap();
    let cells = build_cells(&tmp);
    write_agent_json(
        &tmp,
        "restart-am",
        serde_json::json!({
            "id": "restart-am",
            "name": "restart-am"
        }),
    );

    let none: Option<rua_config::ModelSlotConfig> = cells
        .section::<rua_config::ModelSlotConfig>("restart-am", "default_model")
        .read()
        .await
        .unwrap();
    assert!(none.is_none());

    let am = rua_config::ModelSlotConfig {
        provider_id: "ollama".into(),
        model: "llama3".into(),
        extra: Default::default(),
    };
    cells
        .section::<rua_config::ModelSlotConfig>("restart-am", "default_model")
        .write(am.clone())
        .await
        .unwrap();

    let cells2 = build_cells(&tmp);
    let restored: rua_config::ModelSlotConfig = cells2
        .section::<rua_config::ModelSlotConfig>("restart-am", "default_model")
        .read()
        .await
        .unwrap()
        .expect("default_model survives restart");
    assert_eq!(restored.provider_id, "ollama");
    assert_eq!(restored.model, "llama3");
}
