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

async fn create_authority_request(app: &super::support::TestApp) -> String {
    create_authority_request_for_command(app, "echo hello").await
}

async fn create_authority_request_for_command(
    app: &super::support::TestApp,
    command: &str,
) -> String {
    create_authority_request_for_command_in_chat(app, "chat-1", "tool-call-1", command).await
}

async fn create_authority_request_for_command_in_chat(
    app: &super::support::TestApp,
    chat_id: &str,
    tool_call_id: &str,
    command: &str,
) -> String {
    let runtime = app
        .state
        .registry
        .get_or_create("default")
        .await
        .expect("runtime");
    let security = runtime.security_module().await.expect("security module");
    let request = security
        .approval_authority()
        .create_request(ApprovalRequestInput {
            agent_id: "default".into(),
            owner: rua_security::ApprovalOwner::chat(rua_core::ChatAddress::agent(
                "default", chat_id,
            )),
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
            owner: rua_security::ApprovalOwner::chat(rua_core::ChatAddress::agent(
                "default", "chat-1",
            )),
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authority_decide_run_once_approves_one_pending_request() {
    let app = build_app();
    let request_id = create_authority_request(&app).await;

    let resp = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/default/approvals/{request_id}/decide"),
        Some(json!({ "decision": "run_once", "decided_by": "webui" })),
        Some("default"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["id"], request_id);
    assert_eq!(body["agent_id"], "default");
    assert_eq!(body["owner"]["kind"], "chat");
    assert_eq!(body["owner"]["address"]["chat_id"], "chat-1");
    assert_eq!(body["tool_name"], "bash");
    assert_eq!(body["status"], "approved");
    assert_eq!(body["decided_by"], "webui");

    let duplicate = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/default/approvals/{request_id}/decide"),
        Some(json!({ "decision": "run_once", "decided_by": "webui" })),
        Some("default"),
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
            owner: rua_security::ApprovalOwner::runtime_session("default", "acp-session-1"),
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
        "/api/agents/default/approvals/runtime-sessions/acp-session-1/snapshot",
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
        "/api/agents/default/approvals/snapshot?chat_id=acp-session-1",
        None,
        Some("default"),
    )
    .await;
    assert_eq!(chat_snapshot.status(), StatusCode::OK);
    assert_eq!(read_json(chat_snapshot).await["pending"], json!([]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authority_decide_run_once_does_not_schedule_backend_continuation() {
    let (app, captured) = build_app_with_capturing_facade();
    let request_id = create_authority_request_for_command_in_chat(
        &app,
        "4edc47c3-1d8f-48bf-bd95-9a81a0f379d8",
        "tool-call-1",
        "du -sh /Users/example/projects/rua 2>/dev/null",
    )
    .await;

    let resp = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/default/approvals/{request_id}/decide"),
        Some(json!({ "decision": "run_once", "decided_by": "webui" })),
        Some("default"),
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
    let app = build_app();
    let request_id = create_authority_request_for_command(&app, "git status --short").await;

    let resp = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/default/approvals/{request_id}/decide"),
        Some(json!({
            "decision": "allow_chat",
            "decided_by": "webui",
            "matcher": {
                "type": "tool_call",
                "tool_name": "bash",
                "input_params": "git status *"
            }
        })),
        Some("default"),
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
            &rua_core::ChatAddress::agent("default", "chat-1"),
            "bash",
            &json!({ "command": "git status" }),
        )
        .await
        .expect("chat-scope rule");
    assert_eq!(rule.label, "Allow bash(git status *) in this chat");
    assert_eq!(rule.source_approval_id, request_id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authority_agent_scope_rules_can_be_listed_and_deleted() {
    let app = build_app();
    let request_id = create_authority_request_for_command(&app, "git status --short").await;

    let resp = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/default/approvals/{request_id}/decide"),
        Some(json!({
            "decision": "allow_agent",
            "decided_by": "webui",
            "matcher": {
                "type": "tool_call",
                "tool_name": "bash",
                "input_params": "git status *"
            }
        })),
        Some("default"),
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
        "chat-1",
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
async fn authority_agent_scope_rule_writes_emit_agent_security_v4() {
    let app = build_sync_enabled_app();
    let request_id = create_authority_request_for_command(&app, "git status --short").await;

    let resp = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/default/approvals/{request_id}/decide"),
        Some(json!({
            "decision": "allow_agent",
            "decided_by": "webui",
            "matcher": {
                "type": "tool_call",
                "tool_name": "bash",
                "input_params": "git status *"
            }
        })),
        Some("default"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&app.state).await;
    let upsert = changes.iter().find(|change| {
        change.domain == "agent_security"
            && change.record_id == "default"
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
        "/api/agents/default/permission-rules",
        None,
        Some("default"),
    )
    .await;
    assert_eq!(list.status(), StatusCode::OK);
    let body = read_json(list).await;
    let rule_id = body[0]["id"].as_str().expect("rule id");

    let delete = request(
        &app.app,
        Method::DELETE,
        &format!("/api/agents/default/permission-rules/{rule_id}"),
        None,
        Some("default"),
    )
    .await;
    assert_eq!(delete.status(), StatusCode::NO_CONTENT);

    let changes = collect_v4_changes(&app.state).await;
    let latest_security = changes
        .iter()
        .rev()
        .find(|change| change.domain == "agent_security" && change.record_id == "default")
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
    let pending_a =
        create_authority_request_for_command_in_chat(&app, "chat-1", "tool-call-a", "echo a").await;
    let pending_b =
        create_authority_request_for_command_in_chat(&app, "chat-1", "tool-call-b", "echo b").await;
    let approved =
        create_authority_request_for_command_in_chat(&app, "chat-1", "tool-call-c", "echo c").await;
    let denied = create_denied_authority_request(&app).await;
    let other_chat =
        create_authority_request_for_command_in_chat(&app, "chat-2", "tool-call-other", "echo x")
            .await;

    let resp = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/default/approvals/{approved}/decide"),
        Some(json!({ "decision": "run_once", "decided_by": "webui" })),
        Some("default"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = request(
        &app.app,
        Method::GET,
        "/api/agents/default/approvals/snapshot?chat_id=chat-1",
        None,
        Some("default"),
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
