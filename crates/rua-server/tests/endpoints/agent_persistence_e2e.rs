use std::time::{Duration, SystemTime};

use rua_config::AgentProfileConfig;
use serde_json::json;
use tempfile::TempDir;

use rua_server::AppState;

fn seed_default_agent(tmp: &TempDir) {
    let dir = tmp.path().join("workspaces").join("default");
    std::fs::create_dir_all(&dir).expect("seed default workspace");
    std::fs::write(
        dir.join("agent.json"),
        serde_json::to_string_pretty(&json!({
            "id": "default",
            "name": "default",
            "description": "",
            "workspace_dir": "",
            "language": "",
            "system_prompt_files": [],
        }))
        .unwrap(),
    )
    .expect("write agent.json");
}

fn agent_json_path(tmp: &TempDir, id: &str) -> std::path::PathBuf {
    tmp.path().join("workspaces").join(id).join("agent.json")
}

fn bump_mtime(path: &std::path::Path) {
    let later = SystemTime::now() + Duration::from_secs(2);
    let f = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open for mtime bump");
    f.set_modified(later).expect("set_modified");
}

fn build_state(tmp: &TempDir) -> AppState {
    AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_external_edit_takes_effect_on_next_read() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);

    let state = build_state(&tmp);

    let before = state
        .agent_cells
        .full("default")
        .read()
        .await
        .expect("read default")
        .expect("default agent present");
    assert_eq!(before.name, "default");
    assert_eq!(before.language, "");

    let path = agent_json_path(&tmp, "default");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&json!({
            "id": "default",
            "name": "default-from-disk",
            "description": "edited externally",
            "workspace_dir": "",
            "language": "ja",
            "system_prompt_files": ["PATCHED.md"],
        }))
        .unwrap(),
    )
    .expect("rewrite agent.json");
    bump_mtime(&path);

    let after = state
        .agent_cells
        .full("default")
        .read()
        .await
        .expect("read after external edit")
        .expect("default present");

    assert_eq!(
        after.name, "default-from-disk",
        "external edit must propagate to next read — ADR-0004 D3"
    );
    assert_eq!(after.language, "ja");
    assert_eq!(after.system_prompt_files, vec!["PATCHED.md"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upsert_then_simulated_restart_keeps_agent() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    let state = build_state(&tmp);

    let cfg = AgentProfileConfig {
        id: "survivor".to_string(),
        name: "Survivor".to_string(),
        description: "kept across restart".to_string(),
        workspace_dir: "".to_string(),
        template_id: None,
        language: "zh".to_string(),
        system_prompt_files: vec!["MAIN.md".to_string()],
        running: Default::default(),
        default_model: None,
        channels: None,
        mcp: Some(json!({"clients": {"weather": "stdio:weather"}})),
        heartbeat: None,
        last_dispatch: None,
        tools: None,
        extra: serde_json::Map::new(),
    };
    state
        .agent_cells
        .full("survivor")
        .write(cfg.clone())
        .await
        .expect("write survivor");

    let path = agent_json_path(&tmp, "survivor");
    assert!(path.exists(), "agent.json must be on disk before restart");

    drop(state);
    let state2 = build_state(&tmp);

    let restored = state2
        .agent_cells
        .full("survivor")
        .read()
        .await
        .expect("read survivor")
        .expect("survivor on disk");
    assert_eq!(restored.id, "survivor");
    assert_eq!(restored.name, "Survivor");
    assert_eq!(restored.language, "zh");
    assert_eq!(restored.system_prompt_files, vec!["MAIN.md"]);

    let mcp = restored.mcp.as_ref().expect("mcp present after restart");
    assert_eq!(mcp["clients"]["weather"], "stdio:weather");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn heartbeat_section_cells_write_read_through_state() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    let state = build_state(&tmp);

    let hb: Option<serde_json::Value> = state
        .agent_cells
        .section::<serde_json::Value>("default", "heartbeat")
        .read()
        .await
        .expect("read heartbeat");
    assert!(hb.is_none(), "fresh agent has no heartbeat");

    let heartbeat_val = json!({
        "enabled": true,
        "every": "15m",
        "target": "wechat",
        "active_hours": {"start": "08:00", "end": "22:00"}
    });
    state
        .agent_cells
        .section::<serde_json::Value>("default", "heartbeat")
        .write(heartbeat_val.clone())
        .await
        .expect("write heartbeat");

    let read_back: serde_json::Value = state
        .agent_cells
        .section::<serde_json::Value>("default", "heartbeat")
        .read()
        .await
        .expect("read heartbeat")
        .expect("heartbeat present after write");
    assert_eq!(read_back["enabled"], true);
    assert_eq!(read_back["every"], "15m");
    assert_eq!(read_back["target"], "wechat");

    drop(state);
    let state2 = build_state(&tmp);
    let persisted: serde_json::Value = state2
        .agent_cells
        .section::<serde_json::Value>("default", "heartbeat")
        .read()
        .await
        .expect("read after restart")
        .expect("heartbeat survives restart");
    assert_eq!(persisted, heartbeat_val);

    let full = state2
        .agent_cells
        .full("default")
        .read()
        .await
        .expect("full read")
        .expect("agent exists");
    assert_eq!(full.heartbeat, Some(heartbeat_val));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn section_write_preserves_other_sections_through_state() {
    let tmp = TempDir::new().expect("tempdir");

    let dir = tmp.path().join("workspaces").join("multi");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        dir.join("agent.json"),
        serde_json::to_string_pretty(&json!({
            "id": "multi",
            "name": "multi-section-agent",
            "description": "has many fields",
            "workspace_dir": "/ws",
            "language": "en",
            "system_prompt_files": ["A.md"],
            "template_id": "tpl-original",
            "heartbeat": {"enabled": true, "every": "30m", "target": "email"},
            "last_dispatch": {"ts": "2025-01-01"},
            "llm_routing": {"default_provider": "openai"}
        }))
        .unwrap(),
    )
    .expect("write multi agent.json");

    let state = build_state(&tmp);

    state
        .agent_cells
        .section::<serde_json::Value>("multi", "heartbeat")
        .write(json!({"enabled": false, "every": "1h", "target": "slack"}))
        .await
        .expect("write heartbeat");

    let raw = std::fs::read_to_string(dir.join("agent.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["template_id"], "tpl-original");
    assert_eq!(parsed["last_dispatch"]["ts"], "2025-01-01");
    assert_eq!(parsed["llm_routing"]["default_provider"], "openai");
    assert_eq!(parsed["name"], "multi-section-agent");
    assert_eq!(parsed["heartbeat"]["enabled"], false);
    assert_eq!(parsed["heartbeat"]["every"], "1h");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn running_config_section_write_persists_then_external_read_sees() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    let state = build_state(&tmp);

    let running_val = json!({
        "max_iters": 75,
        "llm_retry_enabled": true,
        "llm_max_retries": 5,
        "auto_continue_on_text_only": true
    });
    state
        .agent_cells
        .section::<serde_json::Value>("default", "running")
        .write(running_val.clone())
        .await
        .expect("write running via cells");

    let raw = std::fs::read_to_string(agent_json_path(&tmp, "default")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["running"]["max_iters"], 75);
    assert_eq!(parsed["running"]["llm_retry_enabled"], true);
    assert_eq!(parsed["running"]["llm_max_retries"], 5);

    drop(state);
    let state2 = build_state(&tmp);
    let read_back: serde_json::Value = state2
        .agent_cells
        .section::<serde_json::Value>("default", "running")
        .read()
        .await
        .expect("read running")
        .expect("running present after restart");
    assert_eq!(read_back["max_iters"], 75);
    assert_eq!(read_back["auto_continue_on_text_only"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_model_section_write_persists() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(&tmp);
    let state = build_state(&tmp);

    let initial: Option<serde_json::Value> = state
        .agent_cells
        .section::<serde_json::Value>("default", "default_model")
        .read()
        .await
        .expect("read default_model");
    assert!(initial.is_none(), "fresh agent has no default_model");

    let model_val = json!({
        "provider_id": "openai",
        "model": "gpt-4o"
    });
    state
        .agent_cells
        .section::<serde_json::Value>("default", "default_model")
        .write(model_val.clone())
        .await
        .expect("write default_model via cells");

    let read_back: serde_json::Value = state
        .agent_cells
        .section::<serde_json::Value>("default", "default_model")
        .read()
        .await
        .expect("read default_model")
        .expect("default_model present after write");
    assert_eq!(read_back["provider_id"], "openai");
    assert_eq!(read_back["model"], "gpt-4o");

    let raw = std::fs::read_to_string(agent_json_path(&tmp, "default")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["default_model"]["provider_id"], "openai");
    assert_eq!(parsed["default_model"]["model"], "gpt-4o");
    assert_eq!(parsed["name"], "default");
}
