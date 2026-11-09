use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use rua_security::approval_authority::{
    ApprovalAuthorityError, ApprovalDecision, ApprovalOwner, ApprovalRequest, ApprovalReuseMatcher,
    ApprovalReuseRule, ApprovalReuseScope, ApprovalSnapshot, ApprovalStatus, PermissionRuleEffect,
    PermissionRuleInput, PermissionRuleSource,
};
use serde::{Deserialize, Serialize};

use crate::errors::ApiError;
use crate::services::conversation_authority::ConversationActiveTurnPhase;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
struct AuthoritySnapshotQuery {
    chat_id: String,
}

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

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/agents/{agent_id}/approvals/snapshot",
            get(get_authority_snapshot),
        )
        .route(
            "/agents/{agent_id}/approvals/runtime-sessions/{session_id}/snapshot",
            get(get_runtime_session_approval_snapshot),
        )
        .route(
            "/agents/{agent_id}/approvals/{approval_id}/decide",
            post(decide_authority_approval),
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

async fn get_authority_snapshot(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Query(query): Query<AuthoritySnapshotQuery>,
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
        .snapshot_at(&rua_core::ChatAddress::agent(&agent_id, &query.chat_id))
        .await;
    (StatusCode::OK, Json::<ApprovalSnapshot>(snapshot)).into_response()
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

async fn decide_authority_approval(
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
    let resume_request = approval_authority.request(&approval_id).await;
    if let Some(request) = resume_request
        .as_ref()
        .filter(|request| request.agent_id == agent_id)
    {
        if let Some(address) = request.owner.as_chat_address() {
            let _ = state
                .conversation_authority
                .transition_active_turn_phase(
                    address,
                    Some(&request.turn_id),
                    ConversationActiveTurnPhase::RunningTool,
                )
                .await;
        }
    }

    let outcome = approval_authority
        .decide_for_agent(&agent_id, &approval_id, decision)
        .await;
    if outcome.is_err()
        && approval_authority
            .request(&approval_id)
            .await
            .is_some_and(|request| request.status == ApprovalStatus::Pending)
    {
        if let Some(request) = resume_request.as_ref() {
            if let Some(address) = request.owner.as_chat_address() {
                let _ = state
                    .conversation_authority
                    .transition_active_turn_phase(
                        address,
                        Some(&request.turn_id),
                        ConversationActiveTurnPhase::WaitingApproval,
                    )
                    .await;
            }
        }
    }

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
