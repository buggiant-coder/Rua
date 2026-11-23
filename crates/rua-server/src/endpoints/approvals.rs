use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use futures::StreamExt;
use rua_security::approval_authority::{
    ApprovalAuthorityError, ApprovalDecision, ApprovalOwner, ApprovalRequest, ApprovalReuseMatcher,
    ApprovalReuseRule, ApprovalReuseScope, ApprovalSnapshot, PermissionRuleEffect,
    PermissionRuleInput, PermissionRuleSource,
};
use serde::{Deserialize, Serialize};

use crate::errors::ApiError;
use crate::extractors::ChatPath;
use crate::services::device_command_bus::{
    ConversationApprovalDecision, ConversationControlDeviceAction,
    ConversationControlDeviceResponse,
};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
struct AuthorityDecisionBody {
    decision: AuthorityDecisionWire,
    #[serde(default)]
    decided_by: Option<String>,
    #[serde(default)]
    matcher: Option<ApprovalReuseMatcher>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AuthorityDecisionWire {
    RunOnce,
    AllowChat,
    AllowRuntimeSession,
    AllowAgent,
    Reject,
}

#[derive(Debug, Serialize)]
struct ApprovalRuleResponse {
    id: String,
    agent_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    chat_id: Option<String>,
    scope: ApprovalReuseScope,
    effect: PermissionRuleEffect,
    enabled: bool,
    source: PermissionRuleSource,
    label: String,
    matcher: ApprovalReuseMatcher,
    source_approval_id: String,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: Option<chrono::DateTime<chrono::Utc>>,
    hit_count: u64,
    status: String,
}

impl From<ApprovalReuseRule> for ApprovalRuleResponse {
    fn from(rule: ApprovalReuseRule) -> Self {
        let chat_id = rule
            .owner
            .as_ref()
            .and_then(ApprovalOwner::as_chat_address)
            .map(|address| address.chat_id().to_string());
        Self {
            id: rule.id,
            agent_id: rule.agent_id,
            chat_id,
            scope: rule.scope,
            effect: rule.effect,
            enabled: rule.enabled,
            source: rule.source,
            label: rule.label,
            matcher: rule.matcher,
            source_approval_id: rule.source_approval_id,
            created_at: rule.created_at,
            updated_at: rule.updated_at,
            hit_count: rule.hit_count,
            status: if rule.enabled { "active" } else { "disabled" }.into(),
        }
    }
}

/// The identity that decides approvals on a server without account authentication.
const LOCAL_OPERATOR_ID: &str = "local";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/chats/{chatId}/approvals/snapshot", get(chat_snapshot))
        .route(
            "/chats/{chatId}/approvals/{approvalId}/decide",
            post(chat_decide),
        )
        .route(
            "/agents/{agent_id}/approvals/runtime-sessions/{session_id}/snapshot",
            get(get_runtime_session_approval_snapshot),
        )
        .route(
            "/agents/{agent_id}/approvals/{approval_id}/decide",
            post(decide_runtime_session_approval),
        )
        .route(
            "/agents/{agent_id}/permission-rules",
            get(list_approval_rules).post(create_permission_rule),
        )
        .route(
            "/agents/{agent_id}/permission-rules/{rule_id}",
            put(update_permission_rule).delete(delete_approval_rule),
        )
}

async fn get_runtime_session_approval_snapshot(
    State(state): State<AppState>,
    Path((agent_id, session_id)): Path<(String, String)>,
) -> Response {
    let runtime = match state.registry.get_or_create(&agent_id).await {
        Ok(runtime) => runtime,
        Err(_) => return ApiError::NotFound(agent_id).into_response(),
    };
    let Some(security) = runtime.security_module().await else {
        return ApiError::Unavailable("security module not available".into()).into_response();
    };
    let snapshot = security
        .approval_authority()
        .snapshot_for_runtime_session(&agent_id, &session_id)
        .await;
    (StatusCode::OK, Json::<ApprovalSnapshot>(snapshot)).into_response()
}

async fn chat_snapshot(
    State(state): State<AppState>,
    ChatPath((agent_id, chat_id)): ChatPath<(String, String)>,
) -> Response {
    chat_control_approval(
        &state,
        &agent_id,
        &chat_id,
        ConversationControlDeviceAction::ApprovalSnapshot,
        None,
    )
    .await
}

async fn chat_decide(
    State(state): State<AppState>,
    ChatPath((agent_id, chat_id, approval_id)): ChatPath<(String, String, String)>,
    headers: HeaderMap,
    Json(decision): Json<ConversationApprovalDecision>,
) -> Response {
    let login_user = super::auth::extract_bearer(&headers)
        .and_then(|token| state.auth.verified_login_user(&token));
    chat_control_approval(
        &state,
        &agent_id,
        &chat_id,
        ConversationControlDeviceAction::DecideApproval {
            approval_id,
            decision,
        },
        login_user.as_ref(),
    )
    .await
}

async fn chat_control_approval(
    state: &AppState,
    agent_id: &str,
    chat_id: &str,
    action: ConversationControlDeviceAction,
    login_user: Option<&crate::auth::VerifiedLoginUser>,
) -> Response {
    // A Task approval records who decided it. When this server has account
    // authentication, only a signed login may decide; without it there is exactly
    // one local operator, who is that same decider under a stable identity.
    let decider = match login_user {
        Some(user) => Some(user.id().to_string()),
        None if !state.auth.has_registered_users() => Some(LOCAL_OPERATOR_ID.to_string()),
        None => None,
    };
    if let ConversationControlDeviceAction::DecideApproval {
        approval_id,
        decision,
    } = &action
    {
        // Task approvals belong to their execution Device even after their originating
        // Turn ends or the Chat selects another Agent.
        match task_approval_snapshots(state, chat_id, true).await {
            Ok(snapshots) => {
                for (task_id, attempt_no, snapshot) in snapshots {
                    if snapshot
                        .pending
                        .iter()
                        .chain(&snapshot.resolved)
                        .any(|request| request.id == *approval_id)
                    {
                        let Some(decider) = decider.as_deref() else {
                            return ApiError::Unauthorized(
                                "A logged-in user must decide the Task approval".into(),
                            )
                            .into_response();
                        };
                        return match state
                            .chat_task
                            .decide_task_approval(
                                &rua_task::TaskOwner::new(chat_id),
                                &task_id,
                                attempt_no,
                                approval_id,
                                decision.clone(),
                                decider,
                            )
                            .await
                        {
                            Ok(request) => Json(request).into_response(),
                            Err(error) => ApiError::Conflict(error).into_response(),
                        };
                    }
                }
            }
            Err(error) => return ApiError::Unavailable(error).into_response(),
        }
    }
    let workspace = state.chat_workspace();
    match super::chats::dispatch_saved_chat_control(
        &state.conversation_input_intake,
        crate::services::chat_device_activity::device_id_from_sync_device(&state.sync_device),
        agent_id,
        workspace.as_path(),
        chat_id,
        action,
        login_user,
    )
    .await
    {
        Ok(ConversationControlDeviceResponse::ApprovalSnapshot { snapshot }) => {
            let mut snapshot: ApprovalSnapshot = match serde_json::from_value(snapshot) {
                Ok(snapshot) => snapshot,
                Err(error) => return ApiError::Internal(error.to_string()).into_response(),
            };
            match task_approval_snapshots(state, chat_id, false).await {
                Ok(remote) => {
                    for (_, _, other) in remote {
                        for request in other.pending {
                            if !snapshot
                                .pending
                                .iter()
                                .any(|existing| existing.id == request.id)
                            {
                                snapshot.pending.push(request);
                            }
                        }
                        for request in other.resolved {
                            if !snapshot
                                .resolved
                                .iter()
                                .any(|existing| existing.id == request.id)
                            {
                                snapshot.resolved.push(request);
                            }
                        }
                    }
                }
                Err(error) => return ApiError::Unavailable(error).into_response(),
            }
            Json(snapshot).into_response()
        }
        Ok(ConversationControlDeviceResponse::ApprovalDecided { request }) => {
            Json(request).into_response()
        }
        Ok(_) => ApiError::Internal("unexpected Chat approval response".into()).into_response(),
        Err(error) => error.into_response(),
    }
}

async fn task_approval_snapshots(
    state: &AppState,
    chat_id: &str,
    include_terminal: bool,
) -> Result<Vec<(String, u32, ApprovalSnapshot)>, String> {
    let owner = rua_task::TaskOwner::new(chat_id);
    let scope = rua_core::ChatStore::resolve(&state.rua_root, owner.chat_address.clone());
    let references = rua_session::task_refs::ChatTaskRef::list(
        &scope.root().join("sessions").join(chat_id),
        chat_id,
    )
    .map_err(|error| error.to_string())?;
    let requests = references.into_iter().filter_map(|reference| {
        let record = reference.last_target_evidence?.record;
        // Terminal Tasks cannot open new cards. Their already decided requests need
        // no cross-device poll, and offline history must not block foreground approvals.
        if record.state.is_terminal() && !include_terminal {
            return None;
        }
        Some((reference.task_id, record.attempt_no))
    });
    let outcomes = futures::stream::iter(requests.map(|(task_id, attempt_no)| {
        let owner = owner.clone();
        async move {
            state
                .chat_task
                .approval_snapshot(&owner, &task_id, attempt_no)
                .await
                .map(|snapshot| (task_id, attempt_no, snapshot))
        }
    }))
    .buffer_unordered(8)
    .collect::<Vec<_>>()
    .await;
    // An unreachable execution Device has no new authoritative decision. Local
    // Chat cards remain usable while the same references are retried next refresh.
    Ok(outcomes
        .into_iter()
        .filter_map(|outcome| outcome.ok())
        .collect())
}

async fn decide_runtime_session_approval(
    State(state): State<AppState>,
    Path((agent_id, approval_id)): Path<(String, String)>,
    Json(body): Json<AuthorityDecisionBody>,
) -> Response {
    let should_emit_agent_security = matches!(body.decision, AuthorityDecisionWire::AllowAgent);
    let runtime = match state.registry.get_or_create(&agent_id).await {
        Ok(runtime) => runtime,
        Err(_) => return ApiError::NotFound(agent_id).into_response(),
    };
    let Some(security) = runtime.security_module().await else {
        return ApiError::Unavailable("security module not available".into()).into_response();
    };
    let decided_by = body.decided_by.unwrap_or_else(|| "webui".into());
    let decision = match body.decision {
        AuthorityDecisionWire::RunOnce => ApprovalDecision::RunOnce { decided_by },
        AuthorityDecisionWire::AllowChat => {
            let Some(matcher) = body.matcher else {
                return ApiError::BadRequest("matcher is required for allow_chat".into())
                    .into_response();
            };
            ApprovalDecision::AllowChat {
                decided_by,
                matcher,
            }
        }
        AuthorityDecisionWire::AllowRuntimeSession => {
            let Some(matcher) = body.matcher else {
                return ApiError::BadRequest(
                    "matcher is required for allow_runtime_session".into(),
                )
                .into_response();
            };
            ApprovalDecision::AllowRuntimeSession {
                decided_by,
                matcher,
            }
        }
        AuthorityDecisionWire::AllowAgent => {
            let Some(matcher) = body.matcher else {
                return ApiError::BadRequest("matcher is required for allow_agent".into())
                    .into_response();
            };
            ApprovalDecision::AllowAgent {
                decided_by,
                matcher,
            }
        }
        AuthorityDecisionWire::Reject => ApprovalDecision::Reject { decided_by },
    };

    let approval_authority = security.approval_authority();
    let belongs_to_agent_session = approval_authority.request(&approval_id).await.is_some_and(|request|
        request.agent_id == agent_id && matches!(&request.owner, ApprovalOwner::RuntimeSession { agent_id: owner_agent_id, .. } if owner_agent_id == &agent_id)
    );
    if !belongs_to_agent_session {
        return authority_error_to_response(ApprovalAuthorityError::NotFound(approval_id));
    }
    let outcome = approval_authority
        .decide_for_agent(&agent_id, &approval_id, decision)
        .await;
    match outcome {
        Ok(request) => {
            if should_emit_agent_security {
                publish_agent_security_config(&state, &agent_id).await;
            }
            (StatusCode::OK, Json::<ApprovalRequest>(request)).into_response()
        }
        Err(e) => authority_error_to_response(e),
    }
}

async fn list_approval_rules(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Response {
    let runtime = match state.registry.get_or_create(&agent_id).await {
        Ok(runtime) => runtime,
        Err(_) => return ApiError::NotFound(agent_id).into_response(),
    };
    let Some(security) = runtime.security_module().await else {
        return ApiError::Unavailable("security module not available".into()).into_response();
    };
    let rules = security
        .approval_authority()
        .list_agent_rules(&agent_id)
        .await
        .into_iter()
        .map(ApprovalRuleResponse::from)
        .collect::<Vec<_>>();
    (StatusCode::OK, Json(rules)).into_response()
}

async fn create_permission_rule(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<PermissionRuleInput>,
) -> Response {
    let runtime = match state.registry.get_or_create(&agent_id).await {
        Ok(runtime) => runtime,
        Err(_) => return ApiError::NotFound(agent_id).into_response(),
    };
    let Some(security) = runtime.security_module().await else {
        return ApiError::Unavailable("security module not available".into()).into_response();
    };
    match security
        .approval_authority()
        .create_agent_rule(&agent_id, body)
        .await
    {
        Ok(rule) => {
            publish_agent_security_config(&state, &agent_id).await;
            (StatusCode::CREATED, Json(ApprovalRuleResponse::from(rule))).into_response()
        }
        Err(message) => ApiError::BadRequest(message).into_response(),
    }
}

async fn update_permission_rule(
    State(state): State<AppState>,
    Path((agent_id, rule_id)): Path<(String, String)>,
    Json(body): Json<PermissionRuleInput>,
) -> Response {
    let runtime = match state.registry.get_or_create(&agent_id).await {
        Ok(runtime) => runtime,
        Err(_) => return ApiError::NotFound(agent_id).into_response(),
    };
    let Some(security) = runtime.security_module().await else {
        return ApiError::Unavailable("security module not available".into()).into_response();
    };
    match security
        .approval_authority()
        .update_agent_rule(&agent_id, &rule_id, body)
        .await
    {
        Ok(Some(rule)) => {
            publish_agent_security_config(&state, &agent_id).await;
            (StatusCode::OK, Json(ApprovalRuleResponse::from(rule))).into_response()
        }
        Ok(None) => ApiError::NotFound(rule_id).into_response(),
        Err(message) => ApiError::BadRequest(message).into_response(),
    }
}

async fn delete_approval_rule(
    State(state): State<AppState>,
    Path((agent_id, rule_id)): Path<(String, String)>,
) -> Response {
    let runtime = match state.registry.get_or_create(&agent_id).await {
        Ok(runtime) => runtime,
        Err(_) => return ApiError::NotFound(agent_id).into_response(),
    };
    let Some(security) = runtime.security_module().await else {
        return ApiError::Unavailable("security module not available".into()).into_response();
    };
    match security
        .approval_authority()
        .delete_agent_rule(&agent_id, &rule_id)
        .await
    {
        Ok(true) => {
            publish_agent_security_config(&state, &agent_id).await;
            (StatusCode::NO_CONTENT, "").into_response()
        }
        Ok(false) => ApiError::NotFound(rule_id).into_response(),
        Err(message) => ApiError::Internal(message).into_response(),
    }
}

async fn publish_agent_security_config(state: &AppState, agent_id: &str) {
    match state.security.config_store().load(agent_id) {
        Ok(config) => {
            crate::sync_publication::publish_agent_security_committed(
                state,
                agent_id,
                serde_json::to_value(config).unwrap_or_default(),
            )
            .await;
        }
        Err(error) => {
            tracing::warn!(
                error = %error,
                agent_id,
                "[sync-publication] agent security publication skipped: failed to load config"
            );
        }
    }
}

fn authority_error_to_response(e: ApprovalAuthorityError) -> Response {
    match e {
        ApprovalAuthorityError::NotFound(id) => ApiError::NotFound(id).into_response(),
        ApprovalAuthorityError::InvalidState {
            id,
            current,
            attempted,
        } => ApiError::Conflict(format!("approval {id} is {current:?}, cannot {attempted}"))
            .into_response(),
        ApprovalAuthorityError::Store(message) => ApiError::Internal(message).into_response(),
    }
}
