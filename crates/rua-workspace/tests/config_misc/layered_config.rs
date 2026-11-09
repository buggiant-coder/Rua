use std::fs;

use rua_workspace::{deep_merge, LayeredConfig, RuaRoot};
use serde_json::{json, Value};
use tempfile::TempDir;

const ENV_KEY: &str = "RUA_CONFIG_OVERRIDE";

fn env_guard() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex, OnceLock};
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn write_json(path: &std::path::Path, value: &Value) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn workspace_root(tmp: &TempDir) -> RuaRoot {
    RuaRoot::new(tmp.path())
}

fn layered_config(root: &RuaRoot, agent_id: &str) -> LayeredConfig {
    let workspace = root.agent_workspace(agent_id).expect("valid Agent ID");
    LayeredConfig::for_workspace(&root.rua_root, &workspace)
}

#[test]
fn four_layer_priority_order() {
    let _guard = env_guard();
    std::env::remove_var(ENV_KEY);
    let tmp = TempDir::new().unwrap();
    let root = workspace_root(&tmp);
    let agent_id = "agent-a";

    write_json(
        &root.rua_root.join("defaults/config.json"),
        &json!({ "source": "default", "default_only": 1 }),
    );
    write_json(
        &root.rua_root.join("config.json"),
        &json!({ "source": "global", "global_only": 1 }),
    );
    write_json(
        &root
            .agent_workspace(agent_id)
            .expect("valid Agent Workspace")
            .as_path()
            .join("agent.json"),
        &json!({ "source": "workspace", "workspace_only": 1 }),
    );
    std::env::set_var(ENV_KEY, r#"{"source":"env","env_only":1}"#);

    let cfg = layered_config(&root, agent_id);
    let merged = cfg.merged().expect("merge should succeed");

    assert_eq!(merged["source"], "env");
    assert_eq!(merged["default_only"], 1);
    assert_eq!(merged["global_only"], 1);
    assert_eq!(merged["workspace_only"], 1);
    assert_eq!(merged["env_only"], 1);

    std::env::remove_var(ENV_KEY);
}

#[test]
fn workspace_overrides_global_but_env_wins() {
    let _guard = env_guard();
    std::env::remove_var(ENV_KEY);
    let tmp = TempDir::new().unwrap();
    let root = workspace_root(&tmp);
    let agent_id = "agent-b";

    write_json(
        &root.rua_root.join("config.json"),
        &json!({ "model": "gpt-4" }),
    );
    write_json(
        &root
            .agent_workspace(agent_id)
            .expect("valid Agent Workspace")
            .as_path()
            .join("agent.json"),
        &json!({ "model": "claude-opus" }),
    );

    let cfg = layered_config(&root, agent_id);
    let merged = cfg.merged().unwrap();
    assert_eq!(merged["model"], "claude-opus");

    std::env::set_var(ENV_KEY, r#"{"model":"opus-4.7"}"#);
    cfg.invalidate();
    let merged = cfg.merged().unwrap();
    assert_eq!(merged["model"], "opus-4.7");
    std::env::remove_var(ENV_KEY);
}

#[test]
fn deep_merge_nests_objects_and_replaces_scalars() {
    let mut base = json!({
        "provider": { "name": "openai", "model": "gpt-4", "opts": { "temp": 0.1 } },
        "list": [1, 2, 3],
    });
    let overlay = json!({
        "provider": { "model": "gpt-5", "opts": { "top_p": 0.9 } },
        "list": [9],
        "new_key": true,
    });

    deep_merge(&mut base, &overlay);

    assert_eq!(base["provider"]["name"], "openai");
    assert_eq!(base["provider"]["model"], "gpt-5");
    assert_eq!(base["provider"]["opts"]["temp"], 0.1);
    assert_eq!(base["provider"]["opts"]["top_p"], 0.9);
    assert_eq!(base["list"], json!([9]));
    assert_eq!(base["new_key"], true);
}

#[test]
fn cache_invalidates_when_env_override_changes() {
    let _guard = env_guard();
    std::env::remove_var(ENV_KEY);
    let tmp = TempDir::new().unwrap();
    let root = workspace_root(&tmp);

    let cfg = layered_config(&root, "agent-c");

    let first = cfg.merged().unwrap();
    assert!(first.as_object().unwrap().is_empty());

    std::env::set_var(ENV_KEY, r#"{"feature":"on"}"#);
    let second = cfg.merged().unwrap();
    assert_eq!(second["feature"], "on");

    std::env::remove_var(ENV_KEY);
}

#[test]
fn cached_merged_value_is_not_mutated_by_callers() {
    let _guard = env_guard();
    std::env::remove_var(ENV_KEY);
    let tmp = TempDir::new().unwrap();
    let root = workspace_root(&tmp);

    write_json(
        &root.rua_root.join("config.json"),
        &json!({
            "running": {
                "max_iters": 7,
                "nested": {
                    "mode": "stored"
                }
            }
        }),
    );

    let cfg = layered_config(&root, "agent-cache");

    let mut first = cfg.merged().unwrap();
    first["running"]["max_iters"] = json!(50);
    first["running"]["nested"]["mode"] = json!("mutated");

    let second = cfg.merged().unwrap();
    assert_eq!(second["running"]["max_iters"], 7);
    assert_eq!(second["running"]["nested"]["mode"], "stored");

    std::env::remove_var(ENV_KEY);
}

#[test]
fn invalidate_forces_rebuild() {
    let _guard = env_guard();
    std::env::remove_var(ENV_KEY);
    let tmp = TempDir::new().unwrap();
    let root = workspace_root(&tmp);
    let agent_id = "agent-d";
    let global = root.rua_root.join("config.json");

    write_json(&global, &json!({ "v": 1 }));
    let cfg = layered_config(&root, agent_id);
    assert_eq!(cfg.merged().unwrap()["v"], 1);

    write_json(&global, &json!({ "v": 2 }));
    cfg.invalidate();
    assert_eq!(cfg.merged().unwrap()["v"], 2);
}
