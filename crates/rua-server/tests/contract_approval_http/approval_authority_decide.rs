use async_trait::async_trait;
use axum::http::{Method, StatusCode};
use serde_json::json;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

use rua_channel::core::console::{ConsoleChatFacade, ConsoleChatRequest, ConsoleSubmitOutcome};
use rua_security::{ApprovalRequestInput, ApprovalTrigger, ToolCallSnapshot};
use rua_server::middleware::agent_context_middleware;
use rua_server::{endpoints::approvals, AppState, ServerProfile};

use super::support::{build_app, build_sync_enabled_app, collect_v4_changes, read_json, request};

struct CapturingFacade {
    captured: Arc<Mutex<Vec<ConsoleChatRequest>>>,
}

#[async_trait]
impl ConsoleChatFacade for CapturingFacade {
    async fn submit_chat(
        &self,
        _agent_id: &str,
        request: ConsoleChatRequest,
    ) -> ConsoleSubmitOutcome {
        self.captured.lock().unwrap().push(request);
        ConsoleSubmitOutcome::local(true)
    }

    async fn stop_chat(&self, _agent_id: &str, _chat_id: &str) -> bool {
        false
    }
}

fn build_app_with_capturing_facade(
) -> (super::support::TestApp, Arc<Mutex<Vec<ConsoleChatRequest>>>) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed workspace");
    let mut state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let captured = Arc::new(Mutex::new(Vec::new()));
    state.console_facade = Arc::new(CapturingFacade {
        captured: Arc::clone(&captured),
    });
    let app = axum::Router::new()
        .nest("/api", approvals::router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state.clone());
    (
        super::support::TestApp {
            app,
            state,
            _tmp: tmp,
        },
        captured,
    )
}

const CHAT_ID: &str = "4edc47c3-1d8f-48bf-bd95-9a81a0f379d8";
const OTHER_CHAT_ID: &str = "389b1504-a145-4b32-8c82-48d47052666a";

async fn seed_chat(app: &super::support::TestApp, chat_id: &str) {
    use rua_server::services::conversation_authority_placement::{
        ConversationAuthorityEpoch, ConversationAuthorityPlacement,
    };
    let device =
        rua_server::services::device_context::current_sync_device_id(&app.state.sync_device)
            .unwrap();
    let address = rua_core::ChatAddress::new(chat_id);
    let workspace = app.state.chat_workspace();
    let store = rua_session::WorkspaceSessionStore::new(workspace.as_path());
    if store.load_chat_header(chat_id).await.unwrap().is_some() {
        return;
    }
    let placement = ConversationAuthorityPlacement {
        authority_epoch: ConversationAuthorityEpoch::next_for_device(&device, None),
        base_manifest_hash:
            rua_server::services::conversation_manifest::initial_conversation_manifest_hash(
                &address,
            )
            .unwrap(),
        device_id: device,
    };
    rua_server::services::console_chat::materialize_targeted_chat(
        workspace.as_path(),
        "default",
        chat_id,
        "Approval fixture",
        &placement,
    )
    .await
    .unwrap();
}

async fn create_authority_request(app: &super::support::TestApp) -> String {
    create_authority_request_for_command(app, "echo hello").await
}

async fn create_authority_request_for_command(
    app: &super::support::TestApp,
    command: &str,
) -> String {
    create_authority_request_for_command_in_chat(app, "default", CHAT_ID, "tool-call-1", command)
        .await
}

async fn create_authority_request_for_command_in_chat(
    app: &super::support::TestApp,
    agent_id: &str,
    chat_id: &str,
    tool_call_id: &str,
    command: &str,
) -> String {
    seed_chat(app, chat_id).await;
    let runtime = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("runtime");
    let security = runtime.security_module().await.expect("security module");
    let request = security
        .approval_authority()
        .create_request(ApprovalRequestInput {
            agent_id: agent_id.into(),
            owner: rua_security::ApprovalOwner::chat(rua_core::ChatAddress::new(chat_id)),
            turn_id: "turn-1".into(),
            tool_call_id: tool_call_id.into(),
            tool_name: "bash".into(),
            trigger: ApprovalTrigger::approval_mode("Balanced asks before bash"),
            tool_call: ToolCallSnapshot {
                display: format!("bash {command}"),
                args: json!({ "command": command }),
            },
        })
        .await;
    request.id
}

async fn create_denied_authority_request(app: &super::support::TestApp) -> String {
    let runtime = app
        .state
        .registry
        .get_or_create("default")
        .await
        .expect("runtime");
    let security = runtime.security_module().await.expect("security module");
    let request = security
        .approval_authority()
        .create_denied_request(ApprovalRequestInput {
            agent_id: "default".into(),
            owner: rua_security::ApprovalOwner::chat(rua_core::ChatAddress::new(CHAT_ID)),
            turn_id: "turn-1".into(),
            tool_call_id: "tool-call-denied".into(),
            tool_name: "bash".into(),
            trigger: ApprovalTrigger::approval_mode("Denied by test"),
            tool_call: ToolCallSnapshot {
                display: "bash rm -rf tmp".into(),
                args: json!({ "command": "rm -rf tmp" }),
            },
        })
        .await;
    request.id
}

async fn post_chat_decision(
    router: &axum::Router,
    request_id: &str,
    token: &str,
) -> axum::response::Response {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    router
        .clone()
        .oneshot(
            Request::post(format!(
                "/api/chats/{CHAT_ID}/approvals/{request_id}/decide"
            ))
            .header("authorization", format!("Bearer {token}"))
            .extension(axum::extract::ConnectInfo(
                "127.0.0.1:12345".parse::<std::net::SocketAddr>().unwrap(),
            ))
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "decision": "run_once", "decided_by": "webui" }).to_string(),
            ))
            .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_chat_approval_records_only_the_verified_login_identity() {
    let app = build_app();
    let router = rua_server::router::build_app(app.state.clone(), None);
    app.state
        .auth
        .register_user("desktop-user", "fixture-password", None)
        .unwrap();
    let login = request(
        &router,
        Method::POST,
        "/api/auth/login",
        Some(json!({ "username": "desktop-user", "password": "fixture-password" })),
        None,
    )
    .await;
    assert_eq!(login.status(), StatusCode::OK);
    let token = read_json(login).await["token"].as_str().unwrap().to_owned();
    let request_id = create_authority_request(&app).await;
    let response = post_chat_decision(&router, &request_id, &token).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = read_json(response).await;
    assert_eq!(body["decided_by"], "webui");
    assert_eq!(
        body["authenticated_user_decision"],
        json!({ "user_id": "desktop-user", "decision": "run_once" })
    );
    let security = app
        .state
        .registry
        .get_or_create("default")
        .await
        .unwrap()
        .security_module()
        .await
        .unwrap();
    let stored = security
        .approval_authority()
        .request(&request_id)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(stored).unwrap()["authenticated_user_decision"],
        body["authenticated_user_decision"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_chat_approval_via_exempt_host_with_revoked_login_has_no_user_fact() {
    let app = build_app();
    let router = rua_server::router::build_app(app.state.clone(), None);
    let token = app
        .state
        .auth
        .register_user("desktop-user", "fixture-password", None)
        .unwrap();
    assert!(app.state.auth.revoke_token(&token));
    rua_security::SecurityConfigStore::new(app.state.rua_root.clone())
        .update_allow_no_auth_hosts(vec!["127.0.0.1".into()])
        .unwrap();
    let request_id = create_authority_request(&app).await;
    let response = post_chat_decision(&router, &request_id, &token).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = read_json(response).await;
    assert_eq!(body["status"], "approved");
    assert!(body["authenticated_user_decision"].is_null());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_chat_approval_for_an_existing_blank_account_id_remains_an_ordinary_decision() {
    let app = build_app();
    let router = rua_server::router::build_app(app.state.clone(), None);
    let token = app
        .state
        .auth
        .register_user(" ", "fixture-password", None)
        .unwrap();
    let request_id = create_authority_request(&app).await;
    let response = post_chat_decision(&router, &request_id, &token).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = read_json(response).await;
    assert_eq!(body["status"], "approved");
    assert!(body["authenticated_user_decision"].is_null());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_chat_approval_with_static_api_credential_has_no_user_fact() {
    let mut app = build_app();
    app.state.auth = Arc::new(rua_server::auth::AuthService::new_with_token(
        "fixture-token".into(),
    ));
    let router = rua_server::router::build_app(app.state.clone(), None);
    let request_id = create_authority_request(&app).await;
    let response = post_chat_decision(&router, &request_id, "fixture-token").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = read_json(response).await;
    assert_eq!(body["status"], "approved");
    assert!(body["authenticated_user_decision"].is_null());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authority_decide_run_once_approves_one_pending_request() {
    let app = build_app();
    let request_id = create_authority_request(&app).await;

    let resp = request(
        &app.app,
        Method::POST,
        &format!("/api/chats/{CHAT_ID}/approvals/{request_id}/decide"),
        Some(json!({ "decision": "run_once", "decided_by": "webui" })),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["id"], request_id);
    assert_eq!(body["agent_id"], "default");
    assert_eq!(body["owner"]["kind"], "chat");
    assert_eq!(body["owner"]["address"]["chat_id"], CHAT_ID);
    assert_eq!(body["tool_name"], "bash");
    assert_eq!(body["status"], "approved");
    assert_eq!(body["decided_by"], "webui");
    assert!(body["authenticated_user_decision"].is_null());

    let duplicate = request(
        &app.app,
        Method::POST,
        &format!("/api/chats/{CHAT_ID}/approvals/{request_id}/decide"),
        Some(json!({ "decision": "run_once", "decided_by": "webui" })),
        None,
    )
    .await;
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);
    assert!(read_json(duplicate).await["detail"]
        .as_str()
        .expect("detail")
        .contains("cannot decide"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_session_snapshot_is_separate_from_chat_snapshot() {
    let app = build_app();
    let chat_request = create_authority_request(&app).await;
    let runtime = app
        .state
        .registry
        .get_or_create("default")
        .await
        .expect("runtime");
    let security = runtime.security_module().await.expect("security module");
    let created = security
        .approval_authority()
        .create_request(ApprovalRequestInput {
            agent_id: "default".into(),
            owner: rua_security::ApprovalOwner::runtime_session("default", CHAT_ID),
            turn_id: "turn-1".into(),
            tool_call_id: "tool-call-1".into(),
            tool_name: "bash".into(),
            trigger: ApprovalTrigger::approval_mode("ACP runtime approval"),
            tool_call: ToolCallSnapshot {
                display: "bash git status".into(),
                args: json!({"command": "git status"}),
            },
        })
        .await;

    let runtime_snapshot = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/default/approvals/runtime-sessions/{CHAT_ID}/snapshot"),
        None,
        Some("default"),
    )
    .await;
    assert_eq!(runtime_snapshot.status(), StatusCode::OK);
    let runtime_snapshot = read_json(runtime_snapshot).await;
    assert_eq!(runtime_snapshot["pending"][0]["id"], created.id);
    assert_eq!(
        runtime_snapshot["pending"][0]["owner"]["kind"],
        "runtime_session"
    );

    let chat_snapshot = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/{CHAT_ID}/approvals/snapshot"),
        None,
        None,
    )
    .await;
    assert_eq!(chat_snapshot.status(), StatusCode::OK);
    let chat_snapshot = read_json(chat_snapshot).await;
    assert_eq!(chat_snapshot["pending"].as_array().unwrap().len(), 1);
    assert_eq!(chat_snapshot["pending"][0]["id"], chat_request);
    assert_eq!(runtime_snapshot["pending"].as_array().unwrap().len(), 1);

    for path in [
        format!("/api/agents/default/approvals/{chat_request}/decide"),
        format!("/api/chats/{CHAT_ID}/approvals/{}/decide", created.id),
    ] {
        let response = request(
            &app.app,
            Method::POST,
            &path,
            Some(json!({"decision":"run_once","decided_by":"test"})),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }
    let session_decision = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/default/approvals/{}/decide", created.id),
        Some(json!({"decision":"run_once","decided_by":"runtime"})),
        None,
    )
    .await;
    assert_eq!(session_decision.status(), StatusCode::OK);
    assert_eq!(
        read_json(session_decision).await["owner"]["kind"],
        "runtime_session"
    );
    let chat_decision = request(
        &app.app,
        Method::POST,
        &format!("/api/chats/{CHAT_ID}/approvals/{chat_request}/decide"),
        Some(json!({"decision":"reject","decided_by":"console"})),
        None,
    )
    .await;
    assert_eq!(chat_decision.status(), StatusCode::OK);
    assert_eq!(read_json(chat_decision).await["status"], "rejected");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authority_decide_run_once_does_not_schedule_backend_continuation() {
    let (app, captured) = build_app_with_capturing_facade();
    let request_id = create_authority_request_for_command_in_chat(
        &app,
        "default",
        CHAT_ID,
        "tool-call-1",
        "du -sh /Users/example/projects/rua 2>/dev/null",
    )
    .await;

    let resp = request(
        &app.app,
        Method::POST,
        &format!("/api/chats/{CHAT_ID}/approvals/{request_id}/decide"),
        Some(json!({ "decision": "run_once", "decided_by": "webui" })),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(
        captured.lock().unwrap().is_empty(),
        "approval decisions must unblock the original run, not schedule a backend continuation"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authority_decide_allow_chat_persists_backend_matcher_rule() {
    let app = build_sync_enabled_app();
    let request_id = create_authority_request_for_command(&app, "git status --short").await;

    let resp = request(
        &app.app,
        Method::POST,
        &format!("/api/chats/{CHAT_ID}/approvals/{request_id}/decide"),
        Some(json!({
            "decision": "allow_chat",
            "decided_by": "webui",
            "matcher": {
                "type": "tool_call",
                "tool_name": "bash",
                "input_params": "git status *"
            }
        })),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["status"], "approved");

    let runtime = app
        .state
        .registry
        .get_or_create("default")
        .await
        .expect("runtime");
    let security = runtime.security_module().await.expect("security module");
    let rule = security
        .approval_authority()
        .find_reuse_rule_at(
            "default",
            &rua_core::ChatAddress::new(CHAT_ID),
            "bash",
            &json!({ "command": "git status" }),
        )
        .await
        .expect("chat-scope rule");
    assert_eq!(rule.label, "Allow bash(git status *) in this chat");
    assert_eq!(rule.source_approval_id, request_id);
    let changes = collect_v4_changes(&app.state).await;
    let change = changes
        .iter()
        .find(|change| {
            change.domain == "evergreen_chat"
                && change.record_id == format!("chats/{CHAT_ID}/permission_rules")
        })
        .expect("the Chat decision published its rules");
    assert_eq!(change.payload["chat_id"], CHAT_ID);
    assert_eq!(change.payload["rules"][0]["source_approval_id"], request_id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authority_agent_scope_rules_can_be_listed_and_deleted() {
    let app = build_app();
    let request_id = create_authority_request_for_command(&app, "git status --short").await;

    let resp = request(
        &app.app,
        Method::POST,
        &format!("/api/chats/{CHAT_ID}/approvals/{request_id}/decide"),
        Some(json!({
            "decision": "allow_agent",
            "decided_by": "webui",
            "matcher": {
                "type": "tool_call",
                "tool_name": "bash",
                "input_params": "git status *"
            }
        })),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let list = request(
        &app.app,
        Method::GET,
        "/api/agents/default/permission-rules",
        None,
        Some("default"),
    )
    .await;
    assert_eq!(list.status(), StatusCode::OK);
    let body = read_json(list).await;
    let rules = body.as_array().expect("rule list");
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0]["scope"], "agent");
    assert_eq!(rules[0]["status"], "active");
    assert_eq!(rules[0]["source_approval_id"], request_id);
    assert_eq!(rules[0]["hit_count"], 0);
    assert_eq!(rules[0]["matcher"]["type"], "tool_call");
    assert_eq!(rules[0]["matcher"]["tool_name"], "bash");
    assert_eq!(rules[0]["matcher"]["input_params"], "git status *");
    let rule_id = rules[0]["id"].as_str().expect("rule id");

    let delete = request(
        &app.app,
        Method::DELETE,
        &format!("/api/agents/default/permission-rules/{rule_id}"),
        None,
        Some("default"),
    )
    .await;
    assert_eq!(delete.status(), StatusCode::NO_CONTENT);

    let list = request(
        &app.app,
        Method::GET,
        "/api/agents/default/permission-rules",
        None,
        Some("default"),
    )
    .await;
    assert_eq!(list.status(), StatusCode::OK);
    assert_eq!(
        read_json(list).await.as_array().expect("rule list").len(),
        0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn permission_rule_crud_supports_manual_deny_enable_disable() {
    let app = build_app();

    let create = request(
        &app.app,
        Method::POST,
        "/api/agents/default/permission-rules",
        Some(json!({
            "effect": "deny",
            "enabled": true,
            "label": "Block WebSearch",
            "matcher": { "type": "tool_call", "tool_name": "WebSearch" }
        })),
        Some("default"),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);
    let created = read_json(create).await;
    assert_eq!(created["effect"], "deny");
    assert_eq!(created["source"], "manual");
    assert_eq!(created["enabled"], true);
    let rule_id = created["id"].as_str().expect("rule id");

    let runtime = app
        .state
        .registry
        .get_or_create("default")
        .await
        .expect("runtime");
    let security = runtime.security_module().await.expect("security module");
    let ctx = rua_security::ToolCallContext::new(
        "default",
        CHAT_ID,
        "WebSearch",
        serde_json::json!({"query": "x"}),
    );
    assert!(
        matches!(
            security.enforce(&ctx, &[]).await,
            rua_security::SecurityVerdict::Deny { .. }
        ),
        "enabled deny permission rule must reject matching calls"
    );

    let update = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/default/permission-rules/{rule_id}"),
        Some(json!({
            "effect": "deny",
            "enabled": false,
            "label": "Block WebSearch",
            "matcher": { "type": "tool_call", "tool_name": "WebSearch" }
        })),
        Some("default"),
    )
    .await;
    assert_eq!(update.status(), StatusCode::OK);
    assert_eq!(read_json(update).await["enabled"], false);
    assert!(
        !matches!(
            security.enforce(&ctx, &[]).await,
            rua_security::SecurityVerdict::Deny { .. }
        ),
        "disabled deny permission rule must stop rejecting matching calls"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_decisions_publish_rules_for_the_request_agent_after_the_chat_selects_another_agent() {
    let app = build_sync_enabled_app();
    app.state
        .agent_service
        .create_agent(
            serde_json::from_value(json!({"id":"original","name":"Original agent"})).unwrap(),
        )
        .await
        .unwrap();
    let request_id = create_authority_request_for_command_in_chat(
        &app,
        "original",
        CHAT_ID,
        "tool-call-1",
        "git status --short",
    )
    .await;

    let resp = request(
        &app.app,
        Method::POST,
        &format!("/api/chats/{CHAT_ID}/approvals/{request_id}/decide"),
        Some(json!({
            "decision": "allow_agent",
            "decided_by": "webui",
            "matcher": {
                "type": "tool_call",
                "tool_name": "bash",
                "input_params": "git status *"
            }
        })),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&app.state).await;
    let upsert = changes.iter().find(|change| {
        change.domain == "agent_security"
            && change.record_id == "original"
            && change.kind == "upsert"
            && change.payload["permission_rules"]
                .as_array()
                .is_some_and(|rules| {
                    rules.iter().any(|rule| {
                        rule["matcher"]["type"] == "tool_call"
                            && rule["matcher"]["tool_name"] == "bash"
                            && rule["matcher"]["input_params"] == "git status *"
                            && rule["source_approval_id"].as_str() == Some(request_id.as_str())
                    })
                })
    });
    assert!(
        upsert.is_some(),
        "allow_agent must append an agent_security v4 upsert carrying permission_rules; got {changes:#?}"
    );

    let list = request(
        &app.app,
        Method::GET,
        "/api/agents/original/permission-rules",
        None,
        None,
    )
    .await;
    assert_eq!(list.status(), StatusCode::OK);
    let body = read_json(list).await;
    let rule_id = body[0]["id"].as_str().expect("rule id");
    let selected_rules = request(
        &app.app,
        Method::GET,
        "/api/agents/default/permission-rules",
        None,
        None,
    )
    .await;
    assert_eq!(selected_rules.status(), StatusCode::OK);
    assert_eq!(read_json(selected_rules).await, json!([]));

    let delete = request(
        &app.app,
        Method::DELETE,
        &format!("/api/agents/original/permission-rules/{rule_id}"),
        None,
        None,
    )
    .await;
    assert_eq!(delete.status(), StatusCode::NO_CONTENT);

    let changes = collect_v4_changes(&app.state).await;
    let latest_security = changes
        .iter()
        .rev()
        .find(|change| change.domain == "agent_security" && change.record_id == "original")
        .expect("agent security change after delete");
    assert_eq!(latest_security.kind, "upsert");
    assert!(
        latest_security.payload["permission_rules"]
            .as_array()
            .map(|rules| rules.is_empty())
            .unwrap_or(true),
        "permission rule deletion must emit an empty agent_security permission_rules state; got {latest_security:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authority_snapshot_returns_pending_and_resolved_for_chat() {
    let app = build_app();
    let pending_a = create_authority_request_for_command_in_chat(
        &app,
        "default",
        CHAT_ID,
        "tool-call-a",
        "echo a",
    )
    .await;
    let pending_b = create_authority_request_for_command_in_chat(
        &app,
        "default",
        CHAT_ID,
        "tool-call-b",
        "echo b",
    )
    .await;
    let approved = create_authority_request_for_command_in_chat(
        &app,
        "default",
        CHAT_ID,
        "tool-call-c",
        "echo c",
    )
    .await;
    let denied = create_denied_authority_request(&app).await;
    let other_chat = create_authority_request_for_command_in_chat(
        &app,
        "default",
        OTHER_CHAT_ID,
        "tool-call-other",
        "echo x",
    )
    .await;

    let resp = request(
        &app.app,
        Method::POST,
        &format!("/api/chats/{CHAT_ID}/approvals/{approved}/decide"),
        Some(json!({ "decision": "run_once", "decided_by": "webui" })),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/{CHAT_ID}/approvals/snapshot"),
        None,
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let pending = body["pending"].as_array().expect("pending approvals");
    let resolved = body["resolved"].as_array().expect("resolved approvals");
    let pending_ids: Vec<_> = pending.iter().map(|item| item["id"].clone()).collect();
    assert_eq!(pending_ids, vec![json!(pending_a), json!(pending_b)]);
    assert!(!pending_ids.contains(&json!(other_chat)));

    let resolved_ids: Vec<_> = resolved.iter().map(|item| item["id"].clone()).collect();
    assert_eq!(resolved_ids, vec![json!(approved), json!(denied)]);
    assert_eq!(resolved[0]["status"], "approved");
    assert_eq!(resolved[1]["status"], "denied");
    assert_eq!(resolved[1]["tool_call_id"], "tool-call-denied");
}
