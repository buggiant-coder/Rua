use axum::http::{Method, StatusCode};
use http_body_util::BodyExt;

use serde_json::json;

use rua_acp::permissions::{AcpPermissionRequestContext, RequestPermissionRequest};
use rua_acp::remote::{AcpRuntimeStreamItem, AcpRuntimeTurnRequest};
use rua_contract::agent::{AgentInputMessage, AgentRequest};
use rua_core::id::SessionId;
use rua_core::message::ContentPart;
use rua_server::endpoints::acp::builtin_acp_runner_npm_package;
use rua_server::services::conversation_authority::{
    ConversationActiveTurnPhase, ConversationActiveTurnStart, ConversationDriverKind,
};
use rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch;

use super::support::{
    build_app, create_agent, mock_acp_agent_path, mock_acp_command, read_json, request, runner_body,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_input_acp_start_failure_keeps_accepted_chat_and_failed_turn() {
    let app = build_app();
    app.state.wait_platform_initialization().await.unwrap();
    let runner = "first-input-failure";
    let created = request(&app.app, Method::POST, "/api/agents/default/acp", Some(json!({
        "key": runner, "enabled": true, "command": "definitely-missing-first-input-acp", "args": [], "env": {}, "trusted": false
    })), Some("default")).await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let chat_id = "00000000-0000-4000-8000-000000000901";
    let submitted = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/default/chats/{chat_id}/console/submit"),
        Some(json!({
            "chat_id": chat_id, "message": "accepted ACP input", "initial_acp": {"runner": runner}
        })),
        Some("default"),
    )
    .await;
    assert_eq!(submitted.status(), StatusCode::OK);
    let submitted = read_json(submitted).await;
    assert_eq!(submitted["accepted"], true, "{submitted}");
    let runtime = app.state.registry.get("default").await.unwrap();
    let store = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir());
    let turn = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let Some(turn) = store
                .read_turn_range(chat_id, 1, 1)
                .await
                .unwrap()
                .into_iter()
                .next()
            {
                if turn.lifecycle.status != "in_progress" {
                    break turn;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("ACP startup failure commits terminal Turn");
    assert_eq!(turn.lifecycle.status, "failed");
    let header = store.load_chat_header(chat_id).await.unwrap().unwrap();
    assert!(!header.extra.contains_key("acp_binding"));
    assert!(!header.extra.contains_key("initial_acp"));
    let messages = store
        .read_turn_messages(chat_id, &turn.turn_id)
        .await
        .unwrap();
    assert!(messages.iter().any(|message| message.role == "user"));
}

#[tokio::test]
async fn rejected_http_first_input_does_not_materialize_chat() {
    let app = build_app();
    app.state.wait_platform_initialization().await.unwrap();
    app.state
        .conversation_authority
        .begin_draining("default")
        .await;
    let chat_id = "00000000-0000-4000-8000-000000000903";
    let response = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/default/chats/{chat_id}/console/submit"),
        Some(json!({"message": "rejected first input"})),
        Some("default"),
    )
    .await;
    let body = read_json(response).await;
    assert_eq!(body["accepted"], false, "{body}");
    let runtime = app.state.registry.get("default").await.unwrap();
    let store = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir());
    assert!(store.load_chat_header(chat_id).await.unwrap().is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_input_starts_selected_acp_only_after_acceptance() {
    let app = build_app();
    app.state.wait_platform_initialization().await.unwrap();
    let runner = "first-input-selected";
    let created = request(&app.app, Method::POST, "/api/agents/default/acp", Some(json!({
        "key": runner, "enabled": true, "command": mock_acp_command(), "args": [mock_acp_agent_path()], "env": {}, "trusted": true
    })), Some("default")).await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let runtime = app.state.registry.get("default").await.unwrap();
    let store = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir());
    for (chat_id, resume) in [
        ("00000000-0000-4000-8000-000000000902", None),
        (
            "00000000-0000-4000-8000-000000000905",
            Some("resume-first-input"),
        ),
    ] {
        assert!(store.load_chat_header(chat_id).await.unwrap().is_none());
        let submitted = request(&app.app, Method::POST, &format!("/api/agents/default/chats/{chat_id}/console/submit"), Some(json!({
        "chat_id": chat_id, "message": "selected ACP first input", "initial_acp": {"runner": runner, "resume_session_id": resume}
    })), Some("default")).await;
        assert_eq!(submitted.status(), StatusCode::OK);
        assert_eq!(read_json(submitted).await["accepted"], true);
        let header = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if let Some(header) = store.load_chat_header(chat_id).await.unwrap() {
                    if header.extra.contains_key("acp_binding") {
                        break header;
                    }
                    let turns = store.read_turn_range(chat_id, 1, 1).await.unwrap();
                    if let Some(turn) = turns
                        .first()
                        .filter(|turn| turn.lifecycle.status == "failed")
                    {
                        panic!(
                            "ACP first input failed: {turn:?}; messages={:?}",
                            store
                                .read_turn_messages(chat_id, &turn.turn_id)
                                .await
                                .unwrap()
                        );
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("accepted first input installs selected ACP binding");
        assert_eq!(header.extra["acp_binding"]["runner"], runner);
        if let Some(resume) = resume {
            assert_eq!(header.extra["acp_binding"]["runner_session_id"], resume);
        }
        assert!(!header.extra.contains_key("initial_acp"));
        let exited = request(
            &app.app,
            Method::POST,
            &format!("/api/agents/default/chats/{chat_id}/acp/exit"),
            None,
            Some("default"),
        )
        .await;
        assert_eq!(exited.status(), StatusCode::OK);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_during_first_acp_startup_closes_session_and_commits_interrupted_turn() {
    let app = build_app();
    app.state.wait_platform_initialization().await.unwrap();
    let runtime = app.state.registry.get("default").await.unwrap();
    let marker = runtime.workspace_dir().join("startup-marker");
    let runner = "first-input-stop";
    let created = request(&app.app, Method::POST, "/api/agents/default/acp", Some(json!({
        "key": runner, "enabled": true, "command": mock_acp_command(), "args": [mock_acp_agent_path()],
        "env": {"ACP_MOCK_STARTUP_MARKER": marker}, "trusted": true
    })), Some("default")).await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let chat_id = "00000000-0000-4000-8000-000000000904";
    let submitted = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/default/chats/{chat_id}/console/submit"),
        Some(json!({
            "message": "stop startup", "initial_acp": {"runner": runner}
        })),
        Some("default"),
    )
    .await;
    assert_eq!(read_json(submitted).await["accepted"], true);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !marker.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("ACP reached controlled initialization boundary");
    assert!(
        app.state
            .conversation_input_intake
            .request_local_stop("default", chat_id)
            .await
    );
    let store = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir());
    let turn = store
        .read_turn_range(chat_id, 1, 1)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(turn.lifecycle.status, "interrupted");
    let header = store.load_chat_header(chat_id).await.unwrap().unwrap();
    assert!(!header.extra.contains_key("acp_binding"));
    assert!(!header.extra.contains_key("initial_acp"));
}

fn runner_keys(body: &serde_json::Value) -> Vec<String> {
    body.as_array()
        .expect("runner list")
        .iter()
        .filter_map(|runner| runner["key"].as_str().map(str::to_owned))
        .collect()
}

async fn wait_for_history_containing(
    app: &axum::Router,
    agent_id: &str,
    chat_id: &str,
    needle: &str,
) -> serde_json::Value {
    for _ in 0..30 {
        let history = request(
            app,
            Method::GET,
            &format!("/api/agents/{agent_id}/chats/{chat_id}"),
            None,
            Some("default"),
        )
        .await;
        if history.status() == StatusCode::OK {
            let body = read_json(history).await;
            let compact = serde_json::to_string(&body["messages"]).expect("history messages json");
            if compact.contains(needle) {
                return body;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("history did not contain `{needle}`");
}

async fn wait_for_no_active_turn(app: &super::support::TestApp, agent_id: &str, chat_id: &str) {
    for _ in 0..60 {
        if app
            .state
            .conversation_authority
            .active_turn(&rua_core::ChatAddress::agent(agent_id, chat_id))
            .await
            .is_none()
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("active turn stayed present for {agent_id}/{chat_id}");
}

async fn seed_chat_authority(app: &super::support::TestApp, agent_id: &str, chat_id: &str) {
    let workspace_dir = app.state.rua_root.join("workspaces").join(agent_id);
    let device_id = app
        .state
        .sync_device
        .read()
        .expect("Device identity lock")
        .as_ref()
        .map(|device| hex::encode(device.device_id_short()))
        .expect("Device identity");
    let now = chrono::Utc::now();
    rua_server::services::chat_index::upsert(
        &workspace_dir,
        rua_server::services::chat_index::ChatIndexEntry {
            id: chat_id.to_string(),
            name: "ACP Chat".to_string(),
            user_id: "default".to_string(),
            channel: "console".to_string(),
            created_at: now,
            updated_at: now,
            meta: json!({
                "authority_placement": {
                    "device_id": device_id,
                    "authority_epoch": "0000000000000001000000000000000000000000",
                    "base_manifest_hash":
                        "sha256:1111111111111111111111111111111111111111111111111111111111111111",
                }
            }),
            status: "idle".to_string(),
            pinned: false,
            pin_order: 0,
            tag: rua_session::ChatTag::None,
            archived: false,
        },
    )
    .await
    .expect("seed Chat authority");

    let mut header = rua_session::ChatSessionHeader::new(rua_session::ChatMetadata {
        chat_id: chat_id.to_string(),
        agent_id: agent_id.to_string(),
        title: "ACP Chat".to_string(),
        created_at: now.to_rfc3339(),
        last_activity_at: now.to_rfc3339(),
        pinned: false,
        archived: false,
        preview: None,
    });
    header.extra.insert(
        "authority_placement".to_string(),
        json!({
            "device_id": device_id,
            "authority_epoch": "0000000000000001000000000000000000000000",
            "base_manifest_hash":
                "sha256:1111111111111111111111111111111111111111111111111111111111111111",
        }),
    );
    rua_chat::apply_agent_scope_defaults(&mut header, agent_id);
    let configuration = app
        .state
        .product_hooks
        .resolve_complete_chat_configuration(
            &rua_core::ChatAddress::agent(agent_id, chat_id),
            rua_server::services::product_hooks::ChatConfigurationMaterialization {
                selected_agent_id: agent_id.into(),
                running_config: Default::default(),
                security_config: Default::default(),
                agent_assignment: None,
                prompt_file_content: None,
                tool_config_snapshot: Default::default(),
                capability_configuration: None,
                model: None,
                thinking: None,
                collaboration_mode: json!("default"),
                harness_configuration: None,
                authority_device_id: device_id,
            },
        )
        .await
        .unwrap();
    header.extra.insert(
        "hook_configuration".into(),
        serde_json::to_value(configuration).unwrap(),
    );
    rua_session::WorkspaceSessionStore::new(&workspace_dir)
        .save_chat_header(&header)
        .await
        .expect("seed Chat Session authority");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_remote_chat_has_no_acp_permissions_without_contacting_its_device() {
    let app = build_app();
    app.state.wait_platform_initialization().await.unwrap();
    let chat_id = "ordinary-unpaired-device";
    seed_chat_authority(&app, "default", chat_id).await;
    let runtime = app.state.registry.get_or_create("default").await.unwrap();
    let store = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir());
    let mut header = store.load_chat_header(chat_id).await.unwrap().unwrap();
    header.extra.get_mut("authority_placement").unwrap()["device_id"] = json!("51e8e7811446d40d");
    store.save_chat_header(&header).await.unwrap();
    for active in [false, true] {
        if active {
            header
                .extra
                .insert("acp_binding".into(), json!({"runner": "codex"}));
            store.save_chat_header(&header).await.unwrap();
        }
        let response = request(
            &app.app,
            Method::GET,
            &format!("/api/agents/default/chats/{chat_id}/acp/permissions"),
            None,
            Some("default"),
        )
        .await;
        let status = response.status();
        let body = read_json(response).await;
        if active {
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
            assert!(body["detail"].is_string(), "{body}");
        } else {
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body, json!({"pending": []}));
        }
    }
}

#[test]
fn builtin_acp_install_recipes_match_builtin_runner_keys() {
    assert_eq!(
        builtin_acp_runner_npm_package("opencode"),
        Some("opencode-ai@1.18.3")
    );
    assert_eq!(
        builtin_acp_runner_npm_package("qwen"),
        Some("@qwen-code/qwen-code@0.19.11")
    );
    assert_eq!(
        builtin_acp_runner_npm_package("claude"),
        Some("@agentclientprotocol/claude-agent-acp@0.59.0")
    );
    assert_eq!(
        builtin_acp_runner_npm_package("codex"),
        Some("@agentclientprotocol/codex-acp@1.1.4")
    );
    assert_eq!(builtin_acp_runner_npm_package("custom"), None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn root_acp_list_backfills_builtin_runner_contract() {
    let app = build_app();

    let resp = request(&app.app, Method::GET, "/api/acp", None, Some("default")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let keys = runner_keys(&body);

    for builtin in ["opencode", "qwen", "claude", "codex"] {
        assert!(
            keys.iter().any(|key| key == builtin),
            "missing builtin {builtin} in {keys:?}"
        );
    }

    let opencode = body
        .as_array()
        .expect("runner list")
        .iter()
        .find(|runner| runner["key"] == "opencode")
        .expect("opencode runner");
    assert_eq!(opencode["is_builtin"], true);
    assert!(opencode["enabled"].is_boolean());
    assert!(opencode["command"].is_string());
    assert!(opencode["args"].is_array());
    assert!(opencode["trusted"].is_boolean());
    assert!(opencode["tool_parse_mode"].is_string());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_scoped_acp_runners_use_chat_lifecycle_route() {
    let app = build_app();
    let agent_id = "chat_acp_runner_agent";
    let runner_key = "chat_scoped_runner";
    let chat_id = "chat-scoped-runner-discovery";
    create_agent(&app.app, agent_id, "Chat ACP runner agent").await;
    seed_chat_authority(&app, agent_id, chat_id).await;
    app.state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("preheat ACP runtime before runner config changes");

    let create = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/acp"),
        Some(runner_body(runner_key, "chat-scoped-cmd")),
        Some("default"),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);

    let resp = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/chats/{chat_id}/acp/runners"),
        None,
        Some("default"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let runner = body
        .as_array()
        .expect("runner list")
        .iter()
        .find(|runner| runner["key"] == runner_key)
        .expect("chat-scoped runner");
    assert_eq!(runner["command"], "chat-scoped-cmd");
    assert_eq!(runner["enabled"], false);
    assert_eq!(runner["is_builtin"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn root_acp_crud_uses_header_selected_agent_and_rust_runner_shape() {
    let app = build_app();

    let create = request(
        &app.app,
        Method::POST,
        "/api/acp",
        Some(runner_body("contract_runner", "contract-cmd")),
        Some("default"),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);
    let created = read_json(create).await;
    assert_eq!(created["key"], "contract_runner");
    assert_eq!(created["command"], "contract-cmd");
    assert_eq!(created["args"], json!(["--version"]));
    assert_eq!(created["enabled"], false);
    assert_eq!(created["is_builtin"], false);
    assert!(created.get("check_result").is_none());

    let get = request(
        &app.app,
        Method::GET,
        "/api/acp/contract_runner",
        None,
        Some("default"),
    )
    .await;
    assert_eq!(get.status(), StatusCode::OK);
    let got = read_json(get).await;
    assert_eq!(got["key"], "contract_runner");
    assert_eq!(got["tool_parse_mode"], "call_detail");

    let duplicate = request(
        &app.app,
        Method::POST,
        "/api/acp",
        Some(runner_body("contract_runner", "contract-cmd")),
        Some("default"),
    )
    .await;
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);
    assert!(read_json(duplicate).await["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains("already exists")));

    let delete = request(
        &app.app,
        Method::DELETE,
        "/api/acp/contract_runner",
        None,
        Some("default"),
    )
    .await;
    assert_eq!(delete.status(), StatusCode::OK);
    assert_eq!(
        read_json(delete).await,
        json!({"message": "ACP runner 'contract_runner' deleted successfully"})
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acp_runner_update_merges_partial_body_without_clearing_command() {
    let app = build_app();

    let create = request(
        &app.app,
        Method::POST,
        "/api/acp",
        Some(runner_body("partial_update_runner", "partial-cmd")),
        Some("default"),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);

    let update = request(
        &app.app,
        Method::PUT,
        "/api/acp/partial_update_runner",
        Some(json!({"trusted": true, "stdio_buffer_limit_bytes": null})),
        Some("default"),
    )
    .await;
    assert_eq!(update.status(), StatusCode::OK);
    let updated = read_json(update).await;
    assert_eq!(updated["command"], "partial-cmd");
    assert_eq!(updated["args"], json!(["--version"]));
    assert_eq!(updated["enabled"], false);
    assert_eq!(updated["trusted"], true);
    assert_eq!(updated["tool_parse_mode"], "call_detail");
    assert_eq!(updated["stdio_buffer_limit_bytes"], serde_json::Value::Null);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acp_install_route_rejects_custom_runner_without_running_npm() {
    let app = build_app();

    let create = request(
        &app.app,
        Method::POST,
        "/api/acp",
        Some(runner_body("custom_install_runner", "contract-cmd")),
        Some("default"),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);

    let install = request(
        &app.app,
        Method::POST,
        "/api/acp/custom_install_runner/install",
        None,
        Some("default"),
    )
    .await;
    assert_eq!(install.status(), StatusCode::BAD_REQUEST);
    assert!(read_json(install).await["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains("builtin")));

    let start = request(
        &app.app,
        Method::POST,
        "/api/acp/custom_install_runner/install/start",
        None,
        Some("default"),
    )
    .await;
    assert_eq!(start.status(), StatusCode::BAD_REQUEST);
    assert!(read_json(start).await["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains("builtin")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acp_install_task_status_is_agent_scoped() {
    let app = build_app();
    let missing = request(
        &app.app,
        Method::GET,
        "/api/acp/install-tasks/missing-task",
        None,
        Some("default"),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert!(read_json(missing).await["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains("install task")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acp_check_route_is_advisory_and_is_not_cached_on_runner_detail() {
    let app = build_app();
    let runner_key = "contract_check_runner";

    let create = request(
        &app.app,
        Method::POST,
        "/api/acp",
        Some(json!({
            "key": runner_key,
            "enabled": false,
            "command": mock_acp_command(),
            "args": [mock_acp_agent_path()],
            "env": {},
            "trusted": true,
            "tool_parse_mode": "call_detail",
            "stdio_buffer_limit_bytes": 1048576
        })),
        Some("default"),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);
    assert!(
        read_json(create).await.get("check_result").is_none(),
        "create response stays cheap and does not probe"
    );

    let check = request(
        &app.app,
        Method::GET,
        &format!("/api/acp/{runner_key}/check"),
        None,
        Some("default"),
    )
    .await;
    assert_eq!(check.status(), StatusCode::OK);
    let checked = read_json(check).await;
    assert_eq!(checked["ok"], true);
    assert_eq!(checked["status"], "ok");
    assert!(checked["message"]
        .as_str()
        .is_some_and(|message| message.contains("available")));

    let detail = request(
        &app.app,
        Method::GET,
        &format!("/api/acp/{runner_key}"),
        None,
        Some("default"),
    )
    .await;
    assert_eq!(detail.status(), StatusCode::OK);
    let detail_body = read_json(detail).await;
    assert!(
        detail_body.get("check_result").is_none(),
        "explicit diagnostics must not become runner card authority"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_acp_uses_existing_authority_and_exit_preserves_it() {
    let app = build_app();
    let agent_id = "acp_binding_agent";
    let runner_key = "contract_chat_acp_binding_runner";
    let chat_id = "contract-chat-acp-binding";
    create_agent(&app.app, agent_id, "ACP binding agent").await;
    seed_chat_authority(&app, agent_id, chat_id).await;

    let create = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/acp"),
        Some(json!({
            "key": runner_key,
            "enabled": true,
            "command": mock_acp_command(),
            "args": [mock_acp_agent_path()],
            "env": {"ACP_MOCK_ECHO_SESSION_ID": "1"},
            "trusted": true,
            "tool_parse_mode": "call_detail",
            "stdio_buffer_limit_bytes": 1048576
        })),
        Some("default"),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);

    let enter = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/chats/{chat_id}/acp/enter"),
        Some(json!({
            "runner": runner_key,
            "cwd": "/tmp"
        })),
        Some("default"),
    )
    .await;
    assert_eq!(enter.status(), StatusCode::OK);
    let enter_body = read_json(enter).await;
    assert_eq!(enter_body["ok"], true);
    let target_device_id = enter_body["status"]["target_device_id"]
        .as_str()
        .expect("target device id")
        .to_string();
    let runner_session_id = enter_body["status"]["session_id"]
        .as_str()
        .expect("runner session id")
        .to_string();

    let history = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/chats/{chat_id}"),
        None,
        Some("default"),
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    let history_body = read_json(history).await;
    let meta = &history_body["meta"];
    assert_eq!(meta["authority_placement"]["device_id"], target_device_id);
    assert_eq!(
        meta["authority_placement"]["authority_epoch"]
            .as_str()
            .map(str::len),
        Some(40)
    );
    assert_eq!(meta["acp_binding"]["kind"], "acp_passthrough");
    assert_eq!(meta["acp_binding"]["device_id"], target_device_id);
    assert_eq!(meta["acp_binding"]["runner"], runner_key);
    assert_eq!(meta["acp_binding"]["cwd"], "/tmp");
    assert_eq!(meta["acp_binding"]["runner_session_id"], runner_session_id);
    assert!(meta["acp_binding"]
        .get("auto_assigned_authority_placement")
        .is_none());
    assert!(history_body.get("status").is_none());
    assert!(history_body.get("active_turn").is_none());
    assert!(history_body.get("control").is_none());

    let duplicate = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/chats/{chat_id}/acp/enter"),
        Some(json!({
            "runner": runner_key,
            "cwd": "/tmp"
        })),
        Some("default"),
    )
    .await;
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);

    let exit = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/chats/{chat_id}/acp/exit"),
        None,
        Some("default"),
    )
    .await;
    assert_eq!(exit.status(), StatusCode::OK);

    let after = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/chats/{chat_id}"),
        None,
        Some("default"),
    )
    .await;
    assert_eq!(after.status(), StatusCode::OK);
    let after_body = read_json(after).await;
    assert!(after_body["meta"].get("acp_binding").is_none());
    assert_eq!(
        after_body["meta"]["authority_placement"]["device_id"],
        target_device_id
    );
    assert_eq!(
        after_body["meta"]["authority_placement"]["authority_epoch"],
        "0000000000000001000000000000000000000000"
    );
    assert!(after_body.get("control").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_acp_submit_resumes_persisted_binding_after_workspace_reload() {
    let app = build_app();
    let agent_id = "acp_resume_after_reload_agent";
    let runner_key = "contract_chat_acp_resume_runner";
    let chat_id = "123e4567-e89b-12d3-a456-426614174000";
    create_agent(&app.app, agent_id, "ACP resume after reload agent").await;
    seed_chat_authority(&app, agent_id, chat_id).await;

    let create = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/acp"),
        Some(json!({
            "key": runner_key,
            "enabled": true,
            "command": mock_acp_command(),
            "args": [mock_acp_agent_path()],
            "env": {"ACP_MOCK_ECHO_SESSION_ID": "1"},
            "trusted": true,
            "tool_parse_mode": "call_detail",
            "stdio_buffer_limit_bytes": 1048576
        })),
        Some("default"),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);

    let enter = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/chats/{chat_id}/acp/enter"),
        Some(json!({
            "runner": runner_key,
            "cwd": "/tmp"
        })),
        Some("default"),
    )
    .await;
    assert_eq!(enter.status(), StatusCode::OK);
    let enter_body = read_json(enter).await;
    let runner_session_id = enter_body["status"]["session_id"]
        .as_str()
        .expect("runner session id")
        .to_string();

    let reload = request(
        &app.app,
        Method::POST,
        &format!("/api/workspace/{agent_id}/reload"),
        None,
        Some("default"),
    )
    .await;
    assert_eq!(reload.status(), StatusCode::OK);

    let submit = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/chats/{chat_id}/console/submit"),
        Some(json!({
            "chat_id": chat_id,
            "message": "continue in acp"
        })),
        Some("default"),
    )
    .await;
    assert_eq!(submit.status(), StatusCode::OK);
    let submit_body = read_json(submit).await;
    assert_eq!(submit_body["accepted"], true);

    let history = wait_for_history_containing(
        &app.app,
        agent_id,
        chat_id,
        &format!("sid={runner_session_id}"),
    )
    .await;
    let compact = serde_json::to_string(&history).expect("history json");
    assert!(
        compact.contains(r#""origin":{"kind":"acp""#),
        "resumed ACP turn should persist ACP origin metadata: {history:#?}"
    );
    assert!(
        compact.contains(runner_key),
        "resumed ACP origin should name the runner: {history:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acp_enter_does_not_materialize_an_unsubmitted_chat() {
    let app = build_app();
    let agent_id = "acp_draft_local_authority_agent";
    let runner_key = "contract_acp_draft_local_runner";
    let chat_id = uuid::Uuid::new_v4().to_string();
    create_agent(&app.app, agent_id, "ACP draft local authority agent").await;

    let create = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/acp"),
        Some(json!({
            "key": runner_key,
            "enabled": true,
            "command": mock_acp_command(),
            "args": [mock_acp_agent_path()],
            "env": {"ACP_MOCK_ECHO_SESSION_ID": "1"},
            "trusted": true,
            "tool_parse_mode": "call_detail",
            "stdio_buffer_limit_bytes": 1048576
        })),
        Some("default"),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);

    let runners = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/chats/{chat_id}/acp/runners"),
        None,
        Some("default"),
    )
    .await;
    assert_eq!(runners.status(), StatusCode::OK);
    let workspace_dir = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("runtime")
        .workspace_dir();
    assert!(
        rua_server::services::chat_index::get(&workspace_dir, &chat_id)
            .await
            .expect("read chat index")
            .is_none(),
        "loading ACP runners must not materialize an empty draft Chat"
    );
    assert!(
        rua_session::WorkspaceSessionStore::new(&workspace_dir)
            .load_chat_header(&chat_id)
            .await
            .expect("read draft header")
            .is_none(),
        "loading ACP runners must not create a draft Chat Header"
    );

    let enter = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/chats/{chat_id}/acp/enter"),
        Some(json!({
            "runner": runner_key,
            "cwd": "/tmp"
        })),
        Some("default"),
    )
    .await;
    assert_eq!(enter.status(), StatusCode::NOT_FOUND);
    assert!(rua_session::WorkspaceSessionStore::new(&workspace_dir)
        .load_chat_header(&chat_id)
        .await
        .unwrap()
        .is_none());
    assert!(
        rua_server::services::chat_index::get(&workspace_dir, &chat_id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_acp_submit_persists_completed_turn_terminal_fact() {
    let app = build_app();
    let agent_id = "acp_terminal_fact_agent";
    let runner_key = "contract_chat_acp_terminal_runner";
    let chat_id = "123e4567-e89b-12d3-a456-426614174010";
    create_agent(&app.app, agent_id, "ACP terminal fact agent").await;
    seed_chat_authority(&app, agent_id, chat_id).await;

    let create = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/acp"),
        Some(json!({
            "key": runner_key,
            "enabled": true,
            "command": mock_acp_command(),
            "args": [mock_acp_agent_path()],
            "env": {"ACP_MOCK_ECHO_SESSION_ID": "1"},
            "trusted": true,
            "tool_parse_mode": "call_detail",
            "stdio_buffer_limit_bytes": 1048576
        })),
        Some("default"),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);

    let enter = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/chats/{chat_id}/acp/enter"),
        Some(json!({
            "runner": runner_key,
            "cwd": "/tmp"
        })),
        Some("default"),
    )
    .await;
    assert_eq!(enter.status(), StatusCode::OK);

    let submit = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/chats/{chat_id}/console/submit"),
        Some(json!({
            "chat_id": chat_id,
            "message": "complete in acp"
        })),
        Some("default"),
    )
    .await;
    assert_eq!(submit.status(), StatusCode::OK);
    assert_eq!(read_json(submit).await["accepted"], true);

    let history = wait_for_history_containing(&app.app, agent_id, chat_id, "complete in acp").await;
    let compact = serde_json::to_string(&history).expect("history json");
    assert!(
        compact.contains(r#""origin":{"kind":"acp""#),
        "ACP turn should persist ACP origin metadata: {history:#?}"
    );

    let runtime = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("runtime");
    let store = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir());
    let mut turns = Vec::new();
    for _ in 0..60 {
        turns = store
            .read_turn_range(chat_id, 1, usize::MAX)
            .await
            .expect("read turns");
        if turns
            .first()
            .is_some_and(|turn| turn.lifecycle.status != "in_progress")
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(
        turns.len(),
        1,
        "ACP submit should create one turn: {turns:#?}"
    );
    assert_eq!(
        turns[0].lifecycle.status, "completed",
        "ACP completion must use the same completed turn terminal fact as non-ACP Chat"
    );
    assert!(
        turns[0].lifecycle.assistant_completed_at.is_some(),
        "ACP completed terminal fact should carry assistant_completed_at"
    );
    wait_for_no_active_turn(&app, agent_id, chat_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_acp_submit_persists_failed_turn_terminal_fact() {
    let app = build_app();
    let agent_id = "acp_failed_terminal_fact_agent";
    let runner_key = "contract_chat_acp_failed_terminal_runner";
    let chat_id = "123e4567-e89b-12d3-a456-426614174011";
    create_agent(&app.app, agent_id, "ACP failed terminal fact agent").await;
    seed_chat_authority(&app, agent_id, chat_id).await;

    let create = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/acp"),
        Some(json!({
            "key": runner_key,
            "enabled": true,
            "command": mock_acp_command(),
            "args": [mock_acp_agent_path()],
            "env": {
                "ACP_MOCK_ECHO_SESSION_ID": "1",
                "ACP_MOCK_FAIL_ON_PROMPT": "1"
            },
            "trusted": true,
            "tool_parse_mode": "call_detail",
            "stdio_buffer_limit_bytes": 1048576
        })),
        Some("default"),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);

    let enter = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/chats/{chat_id}/acp/enter"),
        Some(json!({
            "runner": runner_key,
            "cwd": "/tmp"
        })),
        Some("default"),
    )
    .await;
    assert_eq!(enter.status(), StatusCode::OK);

    let submit = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/chats/{chat_id}/console/submit"),
        Some(json!({
            "chat_id": chat_id,
            "message": "fail in acp"
        })),
        Some("default"),
    )
    .await;
    assert_eq!(submit.status(), StatusCode::OK);
    assert_eq!(read_json(submit).await["accepted"], true);

    let runtime = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("runtime");
    let store = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir());
    let mut turns = Vec::new();
    for _ in 0..30 {
        turns = store
            .read_turn_range(chat_id, 1, usize::MAX)
            .await
            .expect("read turns");
        if turns
            .first()
            .is_some_and(|turn| turn.lifecycle.status != "in_progress")
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(turns.len(), 1, "ACP failed submit should create one turn");
    assert_eq!(
        turns[0].lifecycle.status, "failed",
        "ACP failure must use the same failed turn terminal fact as non-ACP Chat"
    );
    assert!(
        turns[0].lifecycle.assistant_completed_at.is_some(),
        "ACP failed terminal fact should carry assistant_completed_at"
    );
    wait_for_no_active_turn(&app, agent_id, chat_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_acp_stop_persists_interrupted_turn_terminal_fact() {
    let app = build_app();
    let agent_id = "acp_stop_terminal_fact_agent";
    let runner_key = "contract_chat_acp_stop_terminal_runner";
    let chat_id = "123e4567-e89b-12d3-a456-426614174012";
    create_agent(&app.app, agent_id, "ACP stop terminal fact agent").await;
    seed_chat_authority(&app, agent_id, chat_id).await;

    let create = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/acp"),
        Some(json!({
            "key": runner_key,
            "enabled": true,
            "command": mock_acp_command(),
            "args": [mock_acp_agent_path()],
            "env": {
                "ACP_MOCK_ECHO_SESSION_ID": "1",
                "ACP_MOCK_WAIT_FOR_CANCEL": "1"
            },
            "trusted": true,
            "tool_parse_mode": "call_detail",
            "stdio_buffer_limit_bytes": 1048576
        })),
        Some("default"),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);

    let enter = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/chats/{chat_id}/acp/enter"),
        Some(json!({
            "runner": runner_key,
            "cwd": "/tmp"
        })),
        Some("default"),
    )
    .await;
    assert_eq!(enter.status(), StatusCode::OK);

    let submit = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/chats/{chat_id}/console/submit"),
        Some(json!({
            "chat_id": chat_id,
            "message": "wait in acp"
        })),
        Some("default"),
    )
    .await;
    assert_eq!(submit.status(), StatusCode::OK);
    assert_eq!(read_json(submit).await["accepted"], true);

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let observed = app
        .state
        .conversation_authority
        .active_turn(&rua_core::ChatAddress::agent(agent_id, chat_id))
        .await
        .unwrap();
    let stop = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/chats/{chat_id}/console/stop"),
        Some(json!({"turn_id": observed.turn_id, "authority_epoch": observed.authority_epoch})),
        Some("default"),
    )
    .await;
    assert_eq!(stop.status(), StatusCode::OK);
    assert_eq!(read_json(stop).await["stopped"], true);
    if let Some(snapshot) = app
        .state
        .conversation_authority
        .active_turn(&rua_core::ChatAddress::agent(agent_id, chat_id))
        .await
    {
        assert_eq!(snapshot.phase, ConversationActiveTurnPhase::Stopping);
    }

    let runtime = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("runtime");
    let store = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir());
    let mut turns = Vec::new();
    for _ in 0..100 {
        turns = store
            .read_turn_range(chat_id, 1, usize::MAX)
            .await
            .expect("read turns");
        if turns
            .first()
            .is_some_and(|turn| turn.lifecycle.status != "in_progress")
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(turns.len(), 1, "ACP stopped submit should create one turn");
    assert_eq!(
        turns[0].lifecycle.status, "interrupted",
        "ACP Stop must use the same interrupted turn terminal fact as non-ACP Chat"
    );
    assert!(
        turns[0].lifecycle.assistant_completed_at.is_some(),
        "ACP interrupted terminal fact should carry assistant_completed_at"
    );
    wait_for_no_active_turn(&app, agent_id, chat_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_acp_runner_env_reaches_runner_prompt_process() {
    let app = build_app();
    let agent_id = "acp_env_agent";
    let runner_key = "contract_chat_acp_env_runner";
    let chat_id = "123e4567-e89b-12d3-a456-426614174001";
    create_agent(&app.app, agent_id, "ACP env agent").await;
    seed_chat_authority(&app, agent_id, chat_id).await;

    let create = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/acp"),
        Some(json!({
            "key": runner_key,
            "enabled": true,
            "command": mock_acp_command(),
            "args": [mock_acp_agent_path()],
            "env": {
                "ACP_MOCK_ECHO_ENV_KEY": "RUA_ACP_ENV_CONTRACT",
                "RUA_ACP_ENV_CONTRACT": "runner-env-ok"
            },
            "trusted": true,
            "tool_parse_mode": "call_detail",
            "stdio_buffer_limit_bytes": 1048576
        })),
        Some("default"),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);

    let enter = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/chats/{chat_id}/acp/enter"),
        Some(json!({
            "runner": runner_key,
            "cwd": "/tmp"
        })),
        Some("default"),
    )
    .await;
    assert_eq!(enter.status(), StatusCode::OK);

    let submit = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/chats/{chat_id}/console/submit"),
        Some(json!({
            "chat_id": chat_id,
            "message": "echo runner env"
        })),
        Some("default"),
    )
    .await;
    assert_eq!(submit.status(), StatusCode::OK);
    let submit_body = read_json(submit).await;
    assert_eq!(submit_body["accepted"], true, "{submit_body:#?}");

    wait_for_history_containing(
        &app.app,
        agent_id,
        chat_id,
        "RUA_ACP_ENV_CONTRACT=runner-env-ok",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unavailable_runner_configuration_can_be_enabled_updated_and_disabled_without_probe() {
    let app = build_app();
    let runner_key = "contract_bad_runner";

    let create = request(
        &app.app,
        Method::POST,
        "/api/acp",
        Some(json!({
            "key": runner_key,
            "enabled": false,
            "command": "definitely-missing-rua-acp-agent",
            "args": [],
            "env": {},
            "trusted": true,
            "tool_parse_mode": "call_detail",
            "stdio_buffer_limit_bytes": 1048576
        })),
        Some("default"),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);

    let enable = request(
        &app.app,
        Method::PATCH,
        &format!("/api/acp/{runner_key}/toggle"),
        None,
        Some("default"),
    )
    .await;
    assert_eq!(enable.status(), StatusCode::OK);
    let enabled = read_json(enable).await;
    assert_eq!(enabled["enabled"], true);
    assert_eq!(enabled["command"], "definitely-missing-rua-acp-agent");
    assert!(enabled.get("check_result").is_none());

    let update = request(
        &app.app,
        Method::PUT,
        &format!("/api/acp/{runner_key}"),
        Some(json!({
            "enabled": true,
            "command": "another-missing-rua-acp-agent",
            "args": ["--configured-only"],
            "env": {},
            "trusted": true,
            "tool_parse_mode": "call_detail",
            "stdio_buffer_limit_bytes": 1048576
        })),
        Some("default"),
    )
    .await;
    assert_eq!(update.status(), StatusCode::OK);
    let updated = read_json(update).await;
    assert_eq!(updated["enabled"], true);
    assert_eq!(updated["command"], "another-missing-rua-acp-agent");
    assert!(updated.get("check_result").is_none());

    let disable = request(
        &app.app,
        Method::PATCH,
        &format!("/api/acp/{runner_key}/toggle"),
        None,
        Some("default"),
    )
    .await;
    assert_eq!(disable.status(), StatusCode::OK);
    assert_eq!(read_json(disable).await["enabled"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn path_scoped_acp_routes_are_isolated_from_header_and_default_agent() {
    let app = build_app();
    create_agent(&app.app, "acpagent1", "ACP scoped agent").await;

    let create = request(
        &app.app,
        Method::POST,
        "/api/agents/acpagent1/acp",
        Some(runner_body("scoped_runner", "scoped-cmd")),
        Some("default"),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);

    let path_scoped = request(
        &app.app,
        Method::GET,
        "/api/agents/acpagent1/acp/scoped_runner",
        None,
        Some("default"),
    )
    .await;
    assert_eq!(path_scoped.status(), StatusCode::OK);
    assert_eq!(read_json(path_scoped).await["command"], "scoped-cmd");

    let default_root = request(
        &app.app,
        Method::GET,
        "/api/acp/scoped_runner",
        None,
        Some("default"),
    )
    .await;
    assert_eq!(default_root.status(), StatusCode::NOT_FOUND);

    let header_scoped_root = request(
        &app.app,
        Method::GET,
        "/api/acp/scoped_runner",
        None,
        Some("acpagent1"),
    )
    .await;
    assert_eq!(header_scoped_root.status(), StatusCode::OK);
    assert_eq!(read_json(header_scoped_root).await["key"], "scoped_runner");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_acp_runner_key_can_diverge_across_agents() {
    let app = build_app();
    let agent_a = "acpagent_iso_a";
    let agent_b = "acpagent_iso_b";
    create_agent(&app.app, agent_a, "ACP isolation A").await;
    create_agent(&app.app, agent_b, "ACP isolation B").await;

    let runner_key = "shared_runner";

    let create_a = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_a}/acp"),
        Some(runner_body(runner_key, "cmd-a")),
        Some("default"),
    )
    .await;
    assert_eq!(create_a.status(), StatusCode::CREATED);

    let create_b = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_b}/acp"),
        Some(runner_body(runner_key, "cmd-b")),
        Some("default"),
    )
    .await;
    assert_eq!(create_b.status(), StatusCode::CREATED);

    let path_a = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_a}/acp/{runner_key}"),
        None,
        Some("default"),
    )
    .await;
    assert_eq!(path_a.status(), StatusCode::OK);
    let path_a_body = read_json(path_a).await;
    assert_eq!(path_a_body["key"], runner_key);
    assert_eq!(path_a_body["command"], "cmd-a");

    let path_b = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_b}/acp/{runner_key}"),
        None,
        Some("default"),
    )
    .await;
    assert_eq!(path_b.status(), StatusCode::OK);
    let path_b_body = read_json(path_b).await;
    assert_eq!(path_b_body["key"], runner_key);
    assert_eq!(path_b_body["command"], "cmd-b");

    let header_a = request(
        &app.app,
        Method::GET,
        &format!("/api/acp/{runner_key}"),
        None,
        Some(agent_a),
    )
    .await;
    assert_eq!(header_a.status(), StatusCode::OK);
    assert_eq!(read_json(header_a).await["command"], "cmd-a");

    let header_b = request(
        &app.app,
        Method::GET,
        &format!("/api/acp/{runner_key}"),
        None,
        Some(agent_b),
    )
    .await;
    assert_eq!(header_b.status(), StatusCode::OK);
    assert_eq!(read_json(header_b).await["command"], "cmd-b");

    let default_agent = request(
        &app.app,
        Method::GET,
        &format!("/api/acp/{runner_key}"),
        None,
        Some("default"),
    )
    .await;
    assert_eq!(default_agent.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn path_scoped_acp_update_and_builtin_toggle_use_path_agent_contract() {
    let app = build_app();
    let header_agent = "acpagent_update_header";
    let path_agent = "acpagent_update_path";
    create_agent(&app.app, header_agent, "ACP update header").await;
    create_agent(&app.app, path_agent, "ACP update path").await;

    let runner_key = "mutable_runner";

    let create_header = request(
        &app.app,
        Method::POST,
        "/api/acp",
        Some(runner_body(runner_key, "header-cmd")),
        Some(header_agent),
    )
    .await;
    assert_eq!(create_header.status(), StatusCode::CREATED);

    let create_path = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{path_agent}/acp"),
        Some(runner_body(runner_key, "path-cmd-v1")),
        Some(header_agent),
    )
    .await;
    assert_eq!(create_path.status(), StatusCode::CREATED);

    let update_body = json!({
        "enabled": false,
        "command": "path-cmd-v2",
        "args": ["--dry-run", "--json"],
        "env": {"RUST_ACP_CONTRACT": "1"},
        "trusted": true,
        "tool_parse_mode": "update_detail",
        "stdio_buffer_limit_bytes": 2048,
        "qwenpaw_reference": "single-runner update"
    });
    let update = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/{path_agent}/acp/{runner_key}"),
        Some(update_body.clone()),
        Some(header_agent),
    )
    .await;
    assert_eq!(update.status(), StatusCode::OK);
    let updated = read_json(update).await;
    assert_eq!(updated["key"], runner_key);
    assert_eq!(updated["command"], "path-cmd-v2");
    assert_eq!(updated["args"], json!(["--dry-run", "--json"]));
    assert_eq!(updated["env"]["RUST_ACP_CONTRACT"], "1");
    assert_eq!(updated["trusted"], true);
    assert_eq!(updated["tool_parse_mode"], "update_detail");
    assert_eq!(updated["stdio_buffer_limit_bytes"], 2048);
    assert_eq!(updated["qwenpaw_reference"], "single-runner update");

    let header_runner = request(
        &app.app,
        Method::GET,
        &format!("/api/acp/{runner_key}"),
        None,
        Some(header_agent),
    )
    .await;
    assert_eq!(header_runner.status(), StatusCode::OK);
    assert_eq!(
        read_json(header_runner).await["command"],
        "header-cmd",
        "path-scoped update must not mutate the conflicting header agent"
    );

    let toggle = request(
        &app.app,
        Method::PATCH,
        &format!("/api/agents/{path_agent}/acp/opencode/toggle"),
        None,
        Some(header_agent),
    )
    .await;
    assert_eq!(toggle.status(), StatusCode::OK);
    let toggled = read_json(toggle).await;
    assert_eq!(toggled["key"], "opencode");
    assert_eq!(toggled["enabled"], false);

    let path_after = request(
        &app.app,
        Method::GET,
        "/api/acp/opencode",
        None,
        Some(path_agent),
    )
    .await;
    assert_eq!(path_after.status(), StatusCode::OK);
    assert_eq!(read_json(path_after).await["enabled"], false);

    let header_builtin = request(
        &app.app,
        Method::GET,
        "/api/acp/opencode",
        None,
        Some(header_agent),
    )
    .await;
    assert_eq!(header_builtin.status(), StatusCode::OK);
    assert_eq!(
        read_json(header_builtin).await["enabled"],
        true,
        "path-scoped builtin toggle must not mutate the conflicting header agent"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn qwenpaw_config_acp_routes_are_not_rust_public_contract() {
    let app = build_app();
    create_agent(&app.app, "acpagent2", "ACP route gap agent").await;

    for path in [
        "/api/agents/acpagent2/config/acp",
        "/api/agents/acpagent2/config/acp/opencode",
    ] {
        let resp = request(&app.app, Method::GET, path, None, Some("default")).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "path: {path}");
        assert_eq!(read_json(resp).await, json!({"detail": "Not found"}));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acp_permission_relay_routes_are_chat_scoped_and_validate_option_ids() {
    let app = build_app();
    let agent_id = "acp_permission_agent";
    let chat_id = "acp-permission-chat";
    create_agent(&app.app, agent_id, "ACP permission agent").await;
    seed_chat_authority(&app, agent_id, chat_id).await;
    let runtime = app.state.registry.get_or_create(agent_id).await.unwrap();
    let store = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir());
    let mut header = store.load_chat_header(chat_id).await.unwrap().unwrap();
    header
        .extra
        .insert("acp_binding".into(), json!({"runner": "codex"}));
    store.save_chat_header(&header).await.unwrap();
    let active_turn = app
        .state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent(agent_id.to_string(), chat_id.to_string()),
            driver: ConversationDriverKind::Acp,
            authority_device_id: "device-local".to_string(),
            authority_epoch: ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: None,
        })
        .await;
    app.state
        .conversation_authority
        .transition_active_turn_phase(
            &rua_core::ChatAddress::agent(agent_id, chat_id),
            Some(&active_turn.turn_id),
            ConversationActiveTurnPhase::WaitingApproval,
        )
        .await
        .expect("ACP waiting approval");

    let snapshot = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/chats/{chat_id}/acp/permissions"),
        None,
        Some("default"),
    )
    .await;
    assert_eq!(snapshot.status(), StatusCode::OK);
    assert_eq!(read_json(snapshot).await, json!({"pending": []}));

    let protocol_request: RequestPermissionRequest = serde_json::from_value(json!({
        "sessionId": chat_id,
        "toolCall": {"toolCallId": "permission-tool"},
        "options": [{
            "optionId": "allow_once",
            "name": "Allow once",
            "kind": "allow_once"
        }]
    }))
    .expect("ACP permission request");
    let (pending, _resolution_rx) = app
        .state
        .acp_permission_relay
        .create_request(
            AcpPermissionRequestContext {
                agent_id: agent_id.to_string(),
                chat_id: chat_id.to_string(),
                chat_address: None,
                runner: "codex".to_string(),
                task: None,
            },
            &protocol_request,
            None,
        )
        .await;
    let decided = request(
        &app.app,
        Method::POST,
        &format!(
            "/api/agents/{agent_id}/chats/{chat_id}/acp/permissions/{}/decide",
            pending.id
        ),
        Some(json!({"option_id": "allow_once"})),
        Some("default"),
    )
    .await;
    assert_eq!(decided.status(), StatusCode::OK);
    assert_eq!(
        app.state
            .conversation_authority
            .active_turn(&rua_core::ChatAddress::agent(agent_id, chat_id))
            .await
            .expect("ACP active turn")
            .phase,
        ConversationActiveTurnPhase::RunningTool
    );

    let empty_option = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/chats/{chat_id}/acp/permissions/missing/decide"),
        Some(json!({"option_id": ""})),
        Some("default"),
    )
    .await;
    assert_eq!(empty_option.status(), StatusCode::BAD_REQUEST);

    let missing = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/chats/{chat_id}/acp/permissions/missing/decide"),
        Some(json!({"option_id": "allow_once"})),
        Some("default"),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acp_runtime_bridge_reuses_the_resident_agent_runtime() {
    let app = build_app();
    let agent_id = "default";

    let health = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/acp/runtime/health"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(health.status(), StatusCode::OK);
    assert_eq!(read_json(health).await, json!({"ok": true}));

    let turn = AcpRuntimeTurnRequest {
        request: AgentRequest {
            session_id: SessionId::new("acp-runtime-bridge-test"),
            user_id: "acp:acp-runtime-bridge-test".to_string(),
            agent_id: agent_id.to_string().into(),
            messages: vec![AgentInputMessage {
                role: "user".to_string(),
                content: vec![ContentPart::Text {
                    text: "/status".to_string(),
                }],
            }],
            model_override: None,
            tool_mode: "auto".to_string(),
            attachments: vec![],
            display_preferences: None,
            disabled_tools: vec![],
            metadata: std::collections::HashMap::from([("channel".to_string(), json!("acp"))]),
            cancel: None,
            run_control: None,
            workspace_dir: None,
            conversation_store_dir: None,
            working_dir: None,
        },
        workspace_dir: app.state.rua_root.join("workspaces").join(agent_id),
        mcp_config: None,
    };
    let response = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/acp/runtime/turn"),
        Some(serde_json::to_value(turn).expect("turn request json")),
        Some(agent_id),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect ACP runtime stream")
        .to_bytes();
    let items = body
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| {
            serde_json::from_slice::<AcpRuntimeStreamItem>(line).expect("ACP runtime stream item")
        })
        .collect::<Vec<_>>();

    assert!(items.iter().any(|item| {
        matches!(
            item,
            AcpRuntimeStreamItem::Event { event }
                if matches!(event.as_ref(), rua_core::RuntimeEvent::TextDelta { .. })
        )
    }));
    assert!(items.iter().any(|item| {
        matches!(
            item,
            AcpRuntimeStreamItem::Event { event }
                if matches!(event.as_ref(), rua_core::RuntimeEvent::End { .. })
        )
    }));
}
