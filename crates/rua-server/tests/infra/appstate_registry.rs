use rua_contract::config::AppConfig;
use rua_server::profiles::ServerProfile;
use rua_server::AppState;
use tempfile::TempDir;

fn write_min_real_fixture(data_dir: &std::path::Path) {
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
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn load_preheats_active_agent() {
    let tmp = TempDir::new().expect("tempdir");
    let data_dir = tmp.path().to_path_buf();
    let secret_dir = tmp.path().join("secret");
    std::fs::create_dir_all(&secret_dir).expect("mkdir secret");
    write_min_real_fixture(&data_dir);

    let state = AppState::load(
        AppConfig::default(),
        data_dir,
        secret_dir,
        ServerProfile::Test,
    )
    .expect("valid test configuration");

    state.wait_platform_initialization().await.unwrap();
    let summaries = state.registry.list().await.expect("registry.list ok");
    assert_eq!(
        summaries.len(),
        1,
        "registry should contain exactly the preheated active agent"
    );
    assert_eq!(summaries[0].agent_id, "default");

    assert!(
        state.registry.get("default").await.is_some(),
        "registry.get should return the preheated runtime"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn load_keeps_startup_mcp_tasks_alive_after_service_extraction() {
    use mcp_manager::types::ToolFilter;
    use std::time::Duration;

    let tmp = TempDir::new().expect("tempdir");
    let data_dir = tmp.path().to_path_buf();
    let secret_dir = tmp.path().join("secret");
    std::fs::create_dir_all(&secret_dir).unwrap();
    write_min_real_fixture(&data_dir);
    let agent_file = data_dir.join("workspaces/default/agent.json");
    let mut config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&agent_file).unwrap()).unwrap();
    let driver = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../rua-mcp/tests/fixtures/mock_mcp_server.py");
    config["mcp"] = serde_json::json!({"clients":{"startup-fixture":{
        "name":"Startup fixture","enabled":true,"transport":"stdio","command":"python3","args":[driver],
        "auto_connect":"on_startup"
    }}});
    rua_config::atomic_write_json(&agent_file, &config).unwrap();

    let state = AppState::load(
        AppConfig::default(),
        data_dir,
        secret_dir,
        ServerProfile::Test,
    )
    .expect("valid test configuration");
    let manager = state.mcp_manager_for("default").await.unwrap();
    let ready = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let statuses = manager.list_servers().await.unwrap();
            if statuses
                .iter()
                .any(|server| server.key == "startup-fixture" && server.state.is_available())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    let statuses = manager.list_servers().await.unwrap();
    let tools = if ready.is_ok() {
        Some(
            manager
                .list_tools(ToolFilter {
                    server_id: Some("startup-fixture".into()),
                    ..Default::default()
                })
                .await,
        )
    } else {
        None
    };
    manager.shutdown_all().await.unwrap();
    state.signal_shutdown();
    assert!(
        ready.is_ok(),
        "MCP startup must survive AppState::load's helper thread: {statuses:?}"
    );
    assert_eq!(tools.unwrap().unwrap()[0].name, "echo");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn from_tempdir_creates_registry() {
    let tmp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);

    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("get_or_create default ok");
    assert_eq!(runtime.agent_id, "default");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mobile_apk_keeps_the_agent_runtime_available() {
    let tmp = TempDir::new().expect("tempdir");
    let data_dir = tmp.path().to_path_buf();
    let secret_dir = tmp.path().join("secret");
    std::fs::create_dir_all(&secret_dir).expect("mkdir secret");
    write_min_real_fixture(&data_dir);

    let state = AppState::load(
        AppConfig::default(),
        data_dir,
        secret_dir,
        ServerProfile::MobileApk,
    )
    .expect("valid test configuration");

    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("get_or_create default ok");
    assert!(
        runtime.react_runtime().await.is_some(),
        "mobile-apk must expose the same agent runtime used by desktop chat"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn appstate_keeps_runtime_cache_outside_the_working_dir() {
    let tmp = TempDir::new().expect("tempdir");
    let data_dir = tmp.path().join(".rua-test");
    let expected_cache_dir = tmp.path().join(".rua-test.cache");

    let state = AppState::from_tempdir(data_dir.clone(), ServerProfile::Test);

    assert_eq!(state.cache_dir, expected_cache_dir);
    assert!(!data_dir.join("data").exists());
    assert!(!data_dir.join("cache").exists());
}
