use axum::http::Method;
use axum::Router;
use rua_core::ChatAddress;
use serde_json::{json, Value};

use super::support::{build_app, json as response_json, request, wait_for_chat_idle, TestApp};

async fn api(app: &Router, method: Method, path: &str, body: Option<Value>) -> Value {
    let response = request(app, method, path, body).await;
    let status = response.status();
    let body = response_json(response).await;
    assert!(status.is_success(), "{path}: {status} {body}");
    body
}

async fn configure_photo_agent(test: &TestApp) {
    let agent = "photo-editor";
    api(
        &test.app,
        Method::POST,
        "/api/agents",
        Some(json!({"id":agent,"name":"Photo editor"})),
    )
    .await;
    let workspace = rua_core::RuaRoot::new(&test.state.rua_root)
        .agent_workspace(agent)
        .unwrap();
    let prompts = workspace.as_path().join("prompts");
    std::fs::create_dir_all(&prompts).unwrap();
    std::fs::write(prompts.join("photo.md"), "Preserve source photo metadata.").unwrap();
    api(
        &test.app,
        Method::PUT,
        "/api/agents/photo-editor",
        Some(json!({
            "system_prompt_files":["photo.md"],
            "mcp":{"clients":{"offline-album":{
                "name":"Optional album index", "enabled":true, "transport":"stdio",
                "command":test._temp.path().join("uninstalled-album-index"), "auto_connect":"manual"
            }}}
        })),
    )
    .await;
    let mut defaults = api(
        &test.app,
        Method::GET,
        "/api/agents/photo-editor/running-config",
        None,
    )
    .await;
    defaults["max_iters"] = json!(19);
    api(
        &test.app,
        Method::PUT,
        "/api/agents/photo-editor/running-config",
        Some(defaults),
    )
    .await;
    api(
        &test.app,
        Method::POST,
        "/api/agents/photo-editor/skills",
        Some(json!({
            "name":"album-selection","description":"Sort the album","enable":true
        })),
    )
    .await;
    test.state
        .tool_config_store
        .set_enabled(Some(agent), "chat_send", false)
        .await
        .unwrap();
    let security = test.state.security.config_store();
    let mut config = security.load(agent).unwrap();
    config.tool_guard.approval_mode = "strict".into();
    security.save(agent, &config).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn standalone_switch_replaces_agent_contribution_and_preserves_chat_settings_history_and_directory(
) {
    let test = build_app();
    configure_photo_agent(&test).await;
    let chat_id = "00000000-0000-4000-8000-000000000505";
    let address = ChatAddress::new(chat_id);
    let path = format!("/api/chats/{chat_id}");
    let selection_url = format!("{path}/agent-selection");
    let chat_running = format!("{path}/running-config");
    api(
        &test.app,
        Method::POST,
        &format!("{path}/console/submit"),
        Some(json!({"chat_id":chat_id,"agent_id":"default","message":"Preserve the photo album."})),
    )
    .await;
    wait_for_chat_idle(&test.state, chat_id).await;

    let mut running = api(&test.app, Method::GET, &chat_running, None).await;
    running["max_iters"] = json!(27);
    api(&test.app, Method::PUT, &chat_running, Some(running)).await;
    let service = rua_chat::ChatService::new(&test.state.rua_root);
    service
        .update_setting(
            &address,
            rua_chat::ChatSetting::Model(
                json!({"provider_id":"photo-camera","model":"album","thinking_strength":"high"}),
            ),
        )
        .await
        .unwrap();
    let before = api(&test.app, Method::GET, &path, None).await;
    let scope = test.state.chat_workspace();
    let store = rua_session::WorkspaceSessionStore::new(scope.as_path());
    let original = store.load_chat_header(chat_id).await.unwrap().unwrap();
    assert_eq!(
        original.extra["hook_configuration"]["running_config"]["max_iters"],
        27
    );
    let original_cwd = original.extra["execution_directory"].clone();
    assert_eq!(
        original_cwd["path"],
        json!(rua_core::RuaRoot::new(&test.state.rua_root)
            .managed_chat_directory(chat_id, original_cwd["device_id"].as_str().unwrap())
            .unwrap())
    );

    // A malformed required security rule must leave the old Agent usable.
    let security = test.state.security.config_store();
    let valid = security.load("photo-editor").unwrap();
    let mut invalid = valid.clone();
    invalid
        .extra
        .insert("permission_rules".into(), json!("invalid"));
    security.save("photo-editor", &invalid).unwrap();
    let switch = json!({"expected_agent_id":"default","agent_id":"photo-editor"});
    let response = request(
        &test.app,
        Method::PATCH,
        &selection_url,
        Some(switch.clone()),
    )
    .await;
    assert!(!response.status().is_success());
    assert_eq!(
        store.load_chat_header(chat_id).await.unwrap().unwrap(),
        original
    );
    assert_eq!(
        rua_session::chat_index::get(scope.as_path(), chat_id)
            .await
            .unwrap()
            .unwrap()
            .meta["agent_id"],
        "default"
    );
    security.save("photo-editor", &valid).unwrap();

    let result = api(&test.app, Method::PATCH, &selection_url, Some(switch)).await;
    assert_eq!(result["selection"]["agent_id"], "photo-editor");
    let after = api(&test.app, Method::GET, &path, None).await;
    assert_eq!(after["agent_id"], "photo-editor");
    assert_eq!(after["messages"], before["messages"]);
    let saved = store.load_chat_header(chat_id).await.unwrap().unwrap();
    assert_eq!(saved.project_id, None);
    assert_eq!(saved.extra["execution_directory"], original_cwd);
    assert_eq!(
        saved.extra["authority_placement"],
        original.extra["authority_placement"]
    );
    let current = &saved.extra["hook_configuration"];
    assert_eq!(
        current["model"],
        original.extra["hook_configuration"]["model"]
    );
    assert_eq!(
        current["thinking"],
        original.extra["hook_configuration"]["thinking"]
    );
    assert_eq!(current["running_config"]["max_iters"], 19);
    assert_eq!(
        current["security_config"]["tool_guard"]["approval_mode"],
        "strict"
    );
    assert_eq!(current["agent_assignment"]["agent_name"], "Photo editor");
    assert!(current["prompt_file_content"]
        .as_str()
        .unwrap()
        .contains("Preserve source photo metadata."));
    let captured: rua_server::services::product_hooks::ChatHookConfiguration =
        serde_json::from_value(current.clone()).unwrap();
    let capability = captured.capability_configuration.unwrap();
    assert!(capability
        .skills
        .iter()
        .any(|skill| skill.name == "album-selection"));
    assert!(!capability.selection_policy.selects(
        &rua_config::types::tools::ToolIdentity::builtin("chat_send"),
        None
    ));
    assert!(capability.mcp_servers.contains_key("offline-album"));
    let manager = test.state.mcp_manager_for("photo-editor").await.unwrap();
    let server = mcp_manager::types::McpServerId::new("offline-album");
    // An unavailable optional server may be registered as failed, or its
    // transport may be absent altogether. Selection must succeed in both cases.
    let servers = manager.list_servers().await.unwrap();
    assert!(
        !servers
            .iter()
            .any(|status| status.key == server.as_str() && status.state.is_available()),
        "{servers:?}"
    );
    let error = manager
        .connect_server(&server)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains(server.as_str()) || error.contains("uninstalled-album-index"),
        "{error}"
    );
    assert!(!test._temp.path().join("uninstalled-album-index").exists());
    let first_turn = before["messages"][0]["turn_id"].as_str().unwrap();
    assert_eq!(
        service
            .load_turn_agent_assignment(&address, first_turn)
            .await
            .unwrap()
            .agent_id,
        "default"
    );
}
