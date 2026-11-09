use rua_contract::config::AppConfig;
use rua_server::profiles::ServerProfile;
use rua_server::AppState;
use tempfile::TempDir;

#[test]
fn appstate_load_returns_configuration_errors_before_initializing_services() {
    let temp = TempDir::new().unwrap();
    for content in [None, Some("invalid JSON")] {
        if let Some(content) = content {
            std::fs::write(temp.path().join("config.json"), content).unwrap();
        }
        let result = AppState::load(
            AppConfig::default(),
            temp.path().to_path_buf(),
            temp.path().join("secret"),
            ServerProfile::Test,
        );
        assert!(result.is_err());
        assert!(!temp.path().join("workspaces").exists());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn from_tempdir_creates_appstate_and_secret_dir() {
    let tmp = TempDir::new().expect("tempdir");

    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);

    assert_eq!(state.profile, ServerProfile::Test);
    assert!(
        tmp.path().join("secret").is_dir(),
        "from_tempdir must create the `secret` subdirectory"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn from_tempdir_preheats_workspace_registry_with_default_agent() {
    let tmp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    state.wait_platform_initialization().await.unwrap();

    let summaries = state.registry.list().await.expect("registry.list ok");
    assert_eq!(
        summaries.len(),
        1,
        "from_tempdir should preheat exactly the default agent"
    );
    assert_eq!(summaries[0].agent_id, "default");

    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("get_or_create default ok");
    assert_eq!(runtime.agent_id, "default");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn from_tempdir_wires_all_service_fields() {
    let tmp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);

    let _ = state.agent_service.list_agents().await;

    let _ = state.config.clone();
    let _ = state.auth.clone();
    let _ = state.streams.clone();
    let _ = state.push_message_store.clone();
    let _ = state.local_model.clone();
    let _ = state.lifecycle.clone();
    let _ = state.shutdown_tx.subscribe();

    let _ = state.chat_service.clone();
    let _ = state.console_facade.clone();
    let _ = state.provider_manager.clone();
    let _ = state.providers_loader.clone();
    let command_handler = state
        .device_command_handler()
        .expect("Device Command handler should be available");
    assert!(
        format!("{command_handler:?}").contains("has_conversation_authority: true"),
        "rebound Device Command handlers must retain Chat Authority controls"
    );

    let _ = state.skill_service.clone();

    let _ = state.plugins.clone();

    let _ = state.voice.clone();

    if let Some(manager) = state.mcp_manager_for("default").await {
        let servers = manager
            .list_servers()
            .await
            .expect("list bundled MCP servers");
        assert!(
            !servers.is_empty(),
            "an unconfigured manager must serve product defaults"
        );
        assert!(
            servers.iter().all(|server| matches!(
                server.key.as_str(),
                rua_mcp::bundled_browser::SERVER_ID | rua_mcp::bundled_computer::SERVER_ID
            )),
            "unconfigured Agent must not acquire external MCP servers: {servers:?}"
        );
    }

    assert!(
        state.memory_manager_for("default").await.is_some(),
        "memory_manager should be wired in by load"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn load_still_runs_against_handwritten_fixture() {
    let tmp = TempDir::new().expect("tempdir");
    let data_dir = tmp.path().to_path_buf();
    let secret_dir = tmp.path().join("secret");
    std::fs::create_dir_all(&secret_dir).expect("mkdir secret");

    std::fs::write(
        data_dir.join("config.json"),
        serde_json::json!({
            "agents": { "active_agent": "default" }
        })
        .to_string(),
    )
    .expect("write config.json");

    let workspace = data_dir.join("workspaces").join("default");
    std::fs::create_dir_all(&workspace).expect("mkdir workspaces/default");
    std::fs::write(
        workspace.join("agent.json"),
        serde_json::json!({
            "id": "default",
            "name": "Default Agent"
        })
        .to_string(),
    )
    .expect("write agent.json");

    let state = AppState::load(
        AppConfig::default(),
        data_dir,
        secret_dir,
        ServerProfile::Test,
    )
    .expect("valid test configuration");

    state.wait_platform_initialization().await.unwrap();
    let summaries = state.registry.list().await.expect("registry.list ok");
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].agent_id, "default");
}
