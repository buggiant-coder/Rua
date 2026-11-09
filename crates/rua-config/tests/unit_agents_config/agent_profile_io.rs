use std::path::{Path, PathBuf};

use rua_config::loader::save_agent_config;
use rua_config::{
    load_agent_config, save_config, AgentProfileConfig, AgentProfileRef, AgentsConfig,
    AgentsRunningConfig, ModelSlotConfig, RootConfig,
};
use serde_json::json;
use tempfile::TempDir;

fn profile(agent_id: &str, workspace_dir: &Path) -> AgentProfileConfig {
    AgentProfileConfig {
        id: agent_id.to_string(),
        name: format!("{agent_id} agent"),
        description: String::new(),
        workspace_dir: workspace_dir.to_string_lossy().into_owned(),
        template_id: None,
        language: String::new(),
        system_prompt_files: Vec::new(),
        running: AgentsRunningConfig::default(),
        default_model: None,
        channels: None,
        mcp: None,
        heartbeat: None,
        last_dispatch: None,
        tools: None,
        extra: serde_json::Map::new(),
    }
}

fn seed_agent(tmp: &TempDir, agent_id: &str) -> PathBuf {
    let workspace_dir = tmp.path().join("workspaces").join(agent_id);
    std::fs::create_dir_all(&workspace_dir).expect("create agent workspace");
    let config = profile(agent_id, &workspace_dir);
    save_agent_config(&config, &workspace_dir).expect("seed agent profile");
    workspace_dir
}

fn save_root(tmp: &TempDir, agents: &[(&str, &Path)]) {
    let mut profiles = serde_json::Map::new();
    for (agent_id, workspace_dir) in agents {
        profiles.insert(
            (*agent_id).to_string(),
            serde_json::to_value(AgentProfileRef {
                id: (*agent_id).to_string(),
                workspace_dir: workspace_dir.to_string_lossy().into_owned(),
                enabled: true,
                extra: serde_json::Map::new(),
            })
            .expect("profile ref"),
        );
    }

    let root = RootConfig {
        agents: AgentsConfig {
            active_agent: agents
                .first()
                .map(|(agent_id, _)| (*agent_id).to_string())
                .unwrap_or_else(|| "default".to_string()),
            agent_order: agents
                .iter()
                .map(|(agent_id, _)| (*agent_id).to_string())
                .collect(),
            profiles,
            ..Default::default()
        },
        ..Default::default()
    };

    save_config(&root, &tmp.path().join("config.json")).expect("save root config");
}

#[test]
fn load_save_agent_config_persists_default_model_and_running_retry_settings() {
    let tmp = TempDir::new().expect("tempdir");
    let workspace_dir = seed_agent(&tmp, "agent-model");
    save_root(&tmp, &[("agent-model", &workspace_dir)]);

    let mut config = load_agent_config("agent-model", tmp.path()).expect("load agent profile");
    assert!(config.default_model.is_none());

    config.default_model = Some(ModelSlotConfig {
        provider_id: "openai".to_string(),
        model: "gpt-4".to_string(),
        extra: serde_json::Map::new(),
    });
    config.running = AgentsRunningConfig {
        llm_retry_enabled: true,
        llm_max_retries: 5,
        llm_backoff_base: 0.5,
        llm_backoff_cap: 8.0,
        ..Default::default()
    };
    save_agent_config(&config, &workspace_dir).expect("save updated profile");

    let reloaded = load_agent_config("agent-model", tmp.path()).expect("reload agent profile");
    let default_model = reloaded
        .default_model
        .expect("default_model should persist");
    assert_eq!(default_model.provider_id, "openai");
    assert_eq!(default_model.model, "gpt-4");
    assert!(reloaded.running.llm_retry_enabled);
    assert_eq!(reloaded.running.llm_max_retries, 5);
    assert_eq!(reloaded.running.llm_backoff_base, 0.5);
    assert_eq!(reloaded.running.llm_backoff_cap, 8.0);
}

#[test]
fn default_model_can_be_cleared_without_losing_agent_profile_fields() {
    let tmp = TempDir::new().expect("tempdir");
    let workspace_dir = seed_agent(&tmp, "agent-clear");
    save_root(&tmp, &[("agent-clear", &workspace_dir)]);

    let mut config = load_agent_config("agent-clear", tmp.path()).expect("load agent profile");
    config.description = "kept description".to_string();
    config
        .extra
        .insert("future_field".to_string(), json!("preserved"));
    config.default_model = Some(ModelSlotConfig {
        provider_id: "dashscope".to_string(),
        model: "qwen-max".to_string(),
        extra: serde_json::Map::new(),
    });
    save_agent_config(&config, &workspace_dir).expect("save active model");

    let mut reloaded = load_agent_config("agent-clear", tmp.path()).expect("reload active model");
    reloaded.default_model = None;
    save_agent_config(&reloaded, &workspace_dir).expect("clear active model");

    let cleared = load_agent_config("agent-clear", tmp.path()).expect("reload cleared profile");
    assert!(cleared.default_model.is_none());
    assert_eq!(cleared.description, "kept description");
    assert_eq!(cleared.extra["future_field"], "preserved");

    let raw: serde_json::Value = serde_json::from_slice(
        &std::fs::read(workspace_dir.join("agent.json")).expect("read agent json"),
    )
    .expect("parse agent json");
    assert!(
        raw.get("default_model")
            .is_some_and(|value| value.is_null()),
        "Rust persists cleared default_model as explicit null at the config boundary"
    );
}

#[test]
fn default_model_load_save_is_isolated_per_agent_workspace() {
    let tmp = TempDir::new().expect("tempdir");
    let alpha_dir = seed_agent(&tmp, "agent-alpha");
    let beta_dir = seed_agent(&tmp, "agent-beta");
    save_root(
        &tmp,
        &[("agent-alpha", &alpha_dir), ("agent-beta", &beta_dir)],
    );

    let mut alpha = load_agent_config("agent-alpha", tmp.path()).expect("load alpha");
    alpha.default_model = Some(ModelSlotConfig {
        provider_id: "openai".to_string(),
        model: "gpt-4".to_string(),
        extra: serde_json::Map::new(),
    });
    save_agent_config(&alpha, &alpha_dir).expect("save alpha");

    let mut beta = load_agent_config("agent-beta", tmp.path()).expect("load beta");
    beta.default_model = Some(ModelSlotConfig {
        provider_id: "anthropic".to_string(),
        model: "claude-3-5-sonnet-20241022".to_string(),
        extra: serde_json::Map::new(),
    });
    save_agent_config(&beta, &beta_dir).expect("save beta");

    let alpha = load_agent_config("agent-alpha", tmp.path())
        .expect("load alpha configuration")
        .default_model
        .expect("alpha active model");
    let beta = load_agent_config("agent-beta", tmp.path())
        .expect("load beta configuration")
        .default_model
        .expect("beta active model");

    assert_eq!(alpha.provider_id, "openai");
    assert_eq!(alpha.model, "gpt-4");
    assert_eq!(beta.provider_id, "anthropic");
    assert_eq!(beta.model, "claude-3-5-sonnet-20241022");
}
