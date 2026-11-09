use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::Deserialize;
use serde_json::json;

use rua_channel::core::acl_store::AccessControlStore;

use crate::errors::ApiError;
use crate::middleware::AgentExtractorContext;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
struct AclActionEntry {
    channel: String,
    user_id: String,
    #[serde(default)]
    remark: String,
    #[serde(default)]
    username: String,
}

#[derive(Debug, Deserialize)]
struct AclActionBody {
    #[serde(default)]
    entries: Vec<AclActionEntry>,
}

#[derive(Debug, Deserialize)]
struct UpdateRemarkBody {
    channel: String,
    user_id: String,
    remark: String,
}

#[derive(Debug, Deserialize)]
struct UpdateUsernameBody {
    channel: String,
    user_id: String,
    username: String,
}

pub fn root_router() -> Router<AppState> {
    Router::new()
        .route("/access-control", get(get_all_root))
        .route("/access-control/{channel}", get(get_channel_root))
        .route("/access-control/pending/all", get(get_all_pending_root))
        .route(
            "/access-control/pending/approve",
            post(approve_pending_root),
        )
        .route("/access-control/pending/deny", post(deny_pending_root))
        .route(
            "/access-control/pending/dismiss",
            post(dismiss_pending_root),
        )
        .route(
            "/access-control/pending/remark",
            post(update_pending_remark_root),
        )
        .route("/access-control/whitelist/add", post(add_whitelist_root))
        .route(
            "/access-control/whitelist/remove",
            post(remove_whitelist_root),
        )
        .route("/access-control/blacklist/add", post(add_blacklist_root))
        .route(
            "/access-control/blacklist/remove",
            post(remove_blacklist_root),
        )
        .route("/access-control/remark", post(update_remark_root))
        .route("/access-control/username", post(update_username_root))
}

pub fn agent_scoped_router() -> Router<AppState> {
    Router::new()
        .route("/agents/{agentId}/access-control", get(get_all_agent))
        .route(
            "/agents/{agentId}/access-control/{channel}",
            get(get_channel_agent),
        )
        .route(
            "/agents/{agentId}/access-control/pending/all",
            get(get_all_pending_agent),
        )
        .route(
            "/agents/{agentId}/access-control/pending/approve",
            post(approve_pending_agent),
        )
        .route(
            "/agents/{agentId}/access-control/pending/deny",
            post(deny_pending_agent),
        )
        .route(
            "/agents/{agentId}/access-control/pending/dismiss",
            post(dismiss_pending_agent),
        )
        .route(
            "/agents/{agentId}/access-control/pending/remark",
            post(update_pending_remark_agent),
        )
        .route(
            "/agents/{agentId}/access-control/whitelist/add",
            post(add_whitelist_agent),
        )
        .route(
            "/agents/{agentId}/access-control/whitelist/remove",
            post(remove_whitelist_agent),
        )
        .route(
            "/agents/{agentId}/access-control/blacklist/add",
            post(add_blacklist_agent),
        )
        .route(
            "/agents/{agentId}/access-control/blacklist/remove",
            post(remove_blacklist_agent),
        )
        .route(
            "/agents/{agentId}/access-control/remark",
            post(update_remark_agent),
        )
        .route(
            "/agents/{agentId}/access-control/username",
            post(update_username_agent),
        )
}

fn store_for(state: &AppState, agent_id: &str) -> AccessControlStore {
    AccessControlStore::for_workspace(&state.rua_root.join("workspaces").join(agent_id))
}

fn ok_count(count: usize) -> Response {
    Json(json!({"status": "ok", "count": count})).into_response()
}

fn get_all_for(state: &AppState, agent_id: &str) -> Response {
    Json(store_for(state, agent_id).get_all_acls()).into_response()
}

fn get_channel_for(state: &AppState, agent_id: &str, channel: &str) -> Response {
    Json(store_for(state, agent_id).get_acl(channel)).into_response()
}

fn get_all_pending_for(state: &AppState, agent_id: &str) -> Response {
    Json(store_for(state, agent_id).get_all_pending()).into_response()
}

fn approve_pending_for(state: &AppState, agent_id: &str, body: AclActionBody) -> Response {
    let store = store_for(state, agent_id);
    for entry in &body.entries {
        store.approve_pending(&entry.channel, &entry.user_id, &entry.remark);
    }
    ok_count(body.entries.len())
}

fn deny_pending_for(state: &AppState, agent_id: &str, body: AclActionBody) -> Response {
    let store = store_for(state, agent_id);
    for entry in &body.entries {
        store.deny_pending(&entry.channel, &entry.user_id, &entry.remark);
    }
    ok_count(body.entries.len())
}

fn dismiss_pending_for(state: &AppState, agent_id: &str, body: AclActionBody) -> Response {
    let store = store_for(state, agent_id);
    for entry in &body.entries {
        store.dismiss_pending(&entry.channel, &entry.user_id);
    }
    ok_count(body.entries.len())
}

fn update_pending_remark_for(state: &AppState, agent_id: &str, body: UpdateRemarkBody) -> Response {
    let store = store_for(state, agent_id);
    if !store.update_pending_remark(&body.channel, &body.user_id, &body.remark) {
        return ApiError::NotFound("Pending entry not found".to_string()).into_response();
    }
    Json(json!({"status": "ok"})).into_response()
}

fn add_whitelist_for(state: &AppState, agent_id: &str, body: AclActionBody) -> Response {
    let store = store_for(state, agent_id);
    for entry in &body.entries {
        store.add_to_whitelist(
            &entry.channel,
            &entry.user_id,
            &entry.remark,
            &entry.username,
        );
    }
    ok_count(body.entries.len())
}

fn remove_whitelist_for(state: &AppState, agent_id: &str, body: AclActionBody) -> Response {
    let store = store_for(state, agent_id);
    for entry in &body.entries {
        store.remove_from_whitelist(&entry.channel, &entry.user_id);
    }
    ok_count(body.entries.len())
}

fn add_blacklist_for(state: &AppState, agent_id: &str, body: AclActionBody) -> Response {
    let store = store_for(state, agent_id);
    for entry in &body.entries {
        store.add_to_blacklist(
            &entry.channel,
            &entry.user_id,
            &entry.remark,
            &entry.username,
        );
    }
    ok_count(body.entries.len())
}

fn remove_blacklist_for(state: &AppState, agent_id: &str, body: AclActionBody) -> Response {
    let store = store_for(state, agent_id);
    for entry in &body.entries {
        store.remove_from_blacklist(&entry.channel, &entry.user_id);
    }
    ok_count(body.entries.len())
}

fn update_remark_for(state: &AppState, agent_id: &str, body: UpdateRemarkBody) -> Response {
    let store = store_for(state, agent_id);
    if !store.update_remark(&body.channel, &body.user_id, &body.remark) {
        return ApiError::NotFound("User not found in any list".to_string()).into_response();
    }
    Json(json!({"status": "ok"})).into_response()
}

fn update_username_for(state: &AppState, agent_id: &str, body: UpdateUsernameBody) -> Response {
    let store = store_for(state, agent_id);
    if !store.update_username(&body.channel, &body.user_id, &body.username) {
        return ApiError::NotFound("User not found in any list".to_string()).into_response();
    }
    Json(json!({"status": "ok"})).into_response()
}

async fn get_all_root(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
) -> Response {
    get_all_for(&state, &ctx.agent_id)
}

async fn get_channel_root(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(channel): Path<String>,
) -> Response {
    get_channel_for(&state, &ctx.agent_id, &channel)
}

async fn get_all_pending_root(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
) -> Response {
    get_all_pending_for(&state, &ctx.agent_id)
}

async fn approve_pending_root(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Json(body): Json<AclActionBody>,
) -> Response {
    approve_pending_for(&state, &ctx.agent_id, body)
}

async fn deny_pending_root(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Json(body): Json<AclActionBody>,
) -> Response {
    deny_pending_for(&state, &ctx.agent_id, body)
}

async fn dismiss_pending_root(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Json(body): Json<AclActionBody>,
) -> Response {
    dismiss_pending_for(&state, &ctx.agent_id, body)
}

async fn update_pending_remark_root(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Json(body): Json<UpdateRemarkBody>,
) -> Response {
    update_pending_remark_for(&state, &ctx.agent_id, body)
}

async fn add_whitelist_root(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Json(body): Json<AclActionBody>,
) -> Response {
    add_whitelist_for(&state, &ctx.agent_id, body)
}

async fn remove_whitelist_root(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Json(body): Json<AclActionBody>,
) -> Response {
    remove_whitelist_for(&state, &ctx.agent_id, body)
}

async fn add_blacklist_root(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Json(body): Json<AclActionBody>,
) -> Response {
    add_blacklist_for(&state, &ctx.agent_id, body)
}

async fn remove_blacklist_root(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Json(body): Json<AclActionBody>,
) -> Response {
    remove_blacklist_for(&state, &ctx.agent_id, body)
}

async fn update_remark_root(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Json(body): Json<UpdateRemarkBody>,
) -> Response {
    update_remark_for(&state, &ctx.agent_id, body)
}

async fn update_username_root(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Json(body): Json<UpdateUsernameBody>,
) -> Response {
    update_username_for(&state, &ctx.agent_id, body)
}

async fn get_all_agent(State(state): State<AppState>, Path(agent_id): Path<String>) -> Response {
    get_all_for(&state, &agent_id)
}

async fn get_channel_agent(
    State(state): State<AppState>,
    Path((agent_id, channel)): Path<(String, String)>,
) -> Response {
    get_channel_for(&state, &agent_id, &channel)
}

async fn get_all_pending_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Response {
    get_all_pending_for(&state, &agent_id)
}

async fn approve_pending_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<AclActionBody>,
) -> Response {
    approve_pending_for(&state, &agent_id, body)
}

async fn deny_pending_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<AclActionBody>,
) -> Response {
    deny_pending_for(&state, &agent_id, body)
}

async fn dismiss_pending_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<AclActionBody>,
) -> Response {
    dismiss_pending_for(&state, &agent_id, body)
}

async fn update_pending_remark_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<UpdateRemarkBody>,
) -> Response {
    update_pending_remark_for(&state, &agent_id, body)
}

async fn add_whitelist_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<AclActionBody>,
) -> Response {
    add_whitelist_for(&state, &agent_id, body)
}

async fn remove_whitelist_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<AclActionBody>,
) -> Response {
    remove_whitelist_for(&state, &agent_id, body)
}

async fn add_blacklist_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<AclActionBody>,
) -> Response {
    add_blacklist_for(&state, &agent_id, body)
}

async fn remove_blacklist_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<AclActionBody>,
) -> Response {
    remove_blacklist_for(&state, &agent_id, body)
}

async fn update_remark_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<UpdateRemarkBody>,
) -> Response {
    update_remark_for(&state, &agent_id, body)
}

async fn update_username_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<UpdateUsernameBody>,
) -> Response {
    update_username_for(&state, &agent_id, body)
}
