use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use rua_workspace::WorkspaceRuntime;

use crate::errors::ApiError;
use crate::state::AppState;

pub fn root_router() -> Router<AppState> {
    Router::new()
}

pub fn agent_scoped_router() -> Router<AppState> {
    Router::new()
        .route("/agents/{agentId}/memory/search", post(search_memory_agent))
        .route("/agents/{agentId}/memory/write", post(write_memory_agent))
        .route("/agents/{agentId}/memory/get", post(get_memory_agent))
        .route("/agents/{agentId}/memory", get(list_memory_agent))
        .route(
            "/agents/{agentId}/memory/visibility-options",
            get(visibility_options_agent),
        )
        .route("/agents/{agentId}/memory/health", get(health_check_agent))
        .route(
            "/agents/{agentId}/memory/diagnose",
            get(diagnose_memory_agent),
        )
        .route("/agents/{agentId}/memory/repair", post(repair_memory_agent))
        .route(
            "/agents/{agentId}/memory/reindex",
            post(reindex_memory_agent),
        )
        .route(
            "/agents/{agentId}/memory/{memoryId}",
            delete(delete_memory_agent).patch(update_memory_agent),
        )
        .route(
            "/agents/{agentId}/memory/{memoryId}/visibility",
            get(get_memory_visibility_agent).patch(update_memory_visibility_agent),
        )
}

#[derive(Debug, Deserialize)]
struct SearchRequest {
    query: String,
    #[serde(default = "default_top_k")]
    top_k: usize,
}

fn default_top_k() -> usize {
    10
}

#[derive(Debug, Serialize)]
struct SearchHitResponse {
    content: String,
    score: f64,
    source: String,
    #[serde(default)]
    metadata: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct WriteRequest {
    kind: Option<rua_memory::MemoryKind>,
    #[serde(default)]
    scope: Option<rua_memory::MemoryScope>,
    title: String,
    content: String,
    #[serde(default)]
    confidence: Option<f32>,
    #[serde(default)]
    importance: Option<f32>,
    #[serde(default)]
    ttl_days: Option<i64>,
    #[serde(default)]
    labels: Vec<String>,
    search_terms: Vec<String>,
    #[serde(default)]
    evidence: Option<rua_memory::MemorySourceEvidence>,
    #[serde(default)]
    metadata: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct GetRequest {
    ids: Vec<rua_memory::MemoryId>,
    #[serde(default)]
    include_inactive: bool,
}

#[derive(Debug, Deserialize)]
struct UpdateRequest {
    kind: rua_memory::MemoryKind,
    scope: rua_memory::MemoryScope,
    title: String,
    content: String,
    confidence: f32,
    #[serde(default)]
    importance: Option<f32>,
    search_terms: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ListMemoryResponse {
    records: Vec<rua_memory::MemoryRecord>,
    #[serde(default)]
    visibility: HashMap<String, rua_memory::MemoryVisibility>,
}

#[derive(Debug, Serialize)]
struct VisibilityAgentOption {
    id: String,
    name: String,
    enabled: bool,
}

#[derive(Debug, Serialize)]
struct VisibilityOptionsResponse {
    agents: Vec<VisibilityAgentOption>,
}

async fn search_memory_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<SearchRequest>,
) -> Response {
    do_search(state, &agent_id, body).await
}

async fn do_search(state: AppState, agent_id: &str, body: SearchRequest) -> Response {
    let mm = match state.memory_manager_for(agent_id).await {
        Some(mm) => mm,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "error": "memory manager not available" })),
            )
                .into_response();
        }
    };

    let req = rua_agent::memory_manager::MemorySearchRequest {
        search_mode: match state.registry.get_or_create(agent_id).await {
            Ok(runtime) => match runtime.resolved_agent().await {
                Ok(agent) => agent.running.memory_summary.search_mode,
                Err(error) => return ApiError::Internal(error.to_string()).into_response(),
            },
            Err(error) => return ApiError::Internal(error.to_string()).into_response(),
        },
        query: body.query,
        agent_id: agent_id.to_string(),
        top_k: body.top_k,
    };

    match mm.memory_search(req).await {
        Ok(hits) => {
            let results: Vec<SearchHitResponse> = hits
                .into_iter()
                .map(|h| SearchHitResponse {
                    content: h.content,
                    score: h.score,
                    source: h.source,
                    metadata: h.metadata,
                })
                .collect();
            (
                StatusCode::OK,
                Json(serde_json::json!({ "results": results })),
            )
                .into_response()
        }
        Err(e) => ApiError::Internal(e.to_string()).into_response(),
    }
}

async fn write_memory_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<WriteRequest>,
) -> Response {
    if let Err(message) = rua_memory::normalize_memory_title(&body.title) {
        return ApiError::BadRequest(message).into_response();
    }
    if !body.search_terms.iter().any(|term| !term.trim().is_empty()) {
        return ApiError::BadRequest("memory search_terms must not be empty".to_string())
            .into_response();
    }
    let mm = match state.memory_manager_for(&agent_id).await {
        Some(mm) => mm,
        None => return memory_manager_unavailable(),
    };
    let device_id = crate::services::device_context::current_sync_device_id(&state.sync_device);
    let device_name = device_id
        .as_deref()
        .and_then(|id| crate::services::device_context::device_name_snapshot(&state.rua_root, id));
    let host_profile = state.profile.to_string();
    let metadata = inject_memory_device_context(
        body.metadata,
        device_id.as_deref(),
        device_name.as_deref(),
        Some(host_profile.as_str()),
    );

    let req = rua_agent::memory_manager::MemoryWriteRequest {
        agent_id,
        kind: body.kind.unwrap_or(rua_memory::MemoryKind::Fact),
        scope: body.scope.unwrap_or(rua_memory::MemoryScope::Agent),
        title: body.title,
        content: body.content,
        confidence: body.confidence,
        importance: body.importance,
        ttl_days: body.ttl_days,
        labels: body.labels,
        search_terms: body.search_terms,
        evidence: body.evidence,
        metadata,
    };

    match mm.memory_write(req).await {
        Ok(outcome) => {
            (StatusCode::OK, Json(serde_json::to_value(outcome).unwrap())).into_response()
        }
        Err(e) => ApiError::Internal(e.to_string()).into_response(),
    }
}

async fn list_memory_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Response {
    let mm = match state.memory_manager_for(&agent_id).await {
        Some(mm) => mm,
        None => return memory_manager_unavailable(),
    };

    match mm.memory_list(&agent_id).await {
        Ok(records) => {
            let visibility = match mm.memory_visibility_list(&agent_id).await {
                Ok(items) => items
                    .into_iter()
                    .map(|item| (item.memory_id.to_string(), item))
                    .collect::<HashMap<_, _>>(),
                Err(e) => {
                    tracing::warn!(agent_id = %agent_id, error = %e, "failed to load Memory visibility summaries");
                    HashMap::new()
                }
            };
            (
                StatusCode::OK,
                Json(ListMemoryResponse {
                    records,
                    visibility,
                }),
            )
                .into_response()
        }
        Err(e) => ApiError::Internal(e.to_string()).into_response(),
    }
}

async fn get_memory_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<GetRequest>,
) -> Response {
    let mm = match state.memory_manager_for(&agent_id).await {
        Some(mm) => mm,
        None => return memory_manager_unavailable(),
    };

    let req = rua_agent::memory_manager::MemoryGetRequest {
        ids: body.ids,
        include_inactive: body.include_inactive,
    };
    match mm.memory_get(req).await {
        Ok(records) => (
            StatusCode::OK,
            Json(serde_json::json!({ "records": records })),
        )
            .into_response(),
        Err(e) => ApiError::Internal(e.to_string()).into_response(),
    }
}

async fn get_memory_visibility_agent(
    State(state): State<AppState>,
    Path((agent_id, memory_id)): Path<(String, String)>,
) -> Response {
    let mm = match state.memory_manager_for(&agent_id).await {
        Some(mm) => mm,
        None => return memory_manager_unavailable(),
    };
    match mm
        .memory_visibility(&agent_id, &rua_memory::MemoryId::new(memory_id))
        .await
    {
        Ok(Some(visibility)) => (
            StatusCode::OK,
            Json(serde_json::to_value(visibility).unwrap()),
        )
            .into_response(),
        Ok(None) => ApiError::NotFound("memory record not found".to_string()).into_response(),
        Err(e) => ApiError::Internal(e.to_string()).into_response(),
    }
}

async fn update_memory_visibility_agent(
    State(state): State<AppState>,
    Path((agent_id, memory_id)): Path<(String, String)>,
    Json(body): Json<rua_memory::MemoryVisibilityUpdateRequest>,
) -> Response {
    if let Err(e) = validate_visibility_grantees(&state, &agent_id, &body).await {
        return e.into_response();
    }

    let mm = match state.memory_manager_for(&agent_id).await {
        Some(mm) => mm,
        None => return memory_manager_unavailable(),
    };

    match mm
        .memory_visibility_update(&agent_id, rua_memory::MemoryId::new(memory_id), body)
        .await
    {
        Ok(Some(visibility)) => (
            StatusCode::OK,
            Json(serde_json::to_value(visibility).unwrap()),
        )
            .into_response(),
        Ok(None) => ApiError::NotFound("memory record not found".to_string()).into_response(),
        Err(e) => ApiError::Internal(e.to_string()).into_response(),
    }
}

async fn visibility_options_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Response {
    match state.agent_service.list_agents().await {
        Ok(agents) => {
            let options = agents
                .into_iter()
                .filter(|agent| agent.id != agent_id)
                .map(|agent| VisibilityAgentOption {
                    id: agent.id,
                    name: agent.name,
                    enabled: agent.enabled,
                })
                .collect::<Vec<_>>();
            (
                StatusCode::OK,
                Json(VisibilityOptionsResponse { agents: options }),
            )
                .into_response()
        }
        Err(e) => ApiError::Internal(e.to_string()).into_response(),
    }
}

async fn validate_visibility_grantees(
    state: &AppState,
    owner_agent_id: &str,
    body: &rua_memory::MemoryVisibilityUpdateRequest,
) -> Result<(), ApiError> {
    if body.mode != rua_memory::MemoryVisibilityMode::Selected {
        return Ok(());
    }
    let agents = state
        .agent_service
        .list_agents()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let known = agents
        .into_iter()
        .map(|agent| agent.id)
        .collect::<std::collections::HashSet<_>>();
    for id in body.grantee_agent_ids.iter().map(|id| id.trim()) {
        if id.is_empty() || id == owner_agent_id {
            continue;
        }
        if !known.contains(id) {
            return Err(ApiError::BadRequest(format!(
                "unknown memory visibility target agent: {id}"
            )));
        }
    }
    Ok(())
}

async fn delete_memory_agent(
    State(state): State<AppState>,
    Path((agent_id, memory_id)): Path<(String, String)>,
) -> Response {
    let mm = match state.memory_manager_for(&agent_id).await {
        Some(mm) => mm,
        None => return memory_manager_unavailable(),
    };

    match mm.delete_memory_for_agent(&agent_id, &memory_id).await {
        Ok(deleted) => (
            StatusCode::OK,
            Json(serde_json::json!({ "deleted": deleted })),
        )
            .into_response(),
        Err(e) => ApiError::Internal(e.to_string()).into_response(),
    }
}

async fn update_memory_agent(
    State(state): State<AppState>,
    Path((agent_id, memory_id)): Path<(String, String)>,
    Json(body): Json<UpdateRequest>,
) -> Response {
    if let Err(message) = rua_memory::normalize_memory_title(&body.title) {
        return ApiError::BadRequest(message).into_response();
    }
    if !body.search_terms.iter().any(|term| !term.trim().is_empty()) {
        return ApiError::BadRequest("memory search_terms must not be empty".to_string())
            .into_response();
    }
    let mm = match state.memory_manager_for(&agent_id).await {
        Some(mm) => mm,
        None => return memory_manager_unavailable(),
    };
    let id = rua_memory::MemoryId::new(memory_id);
    match mm
        .memory_get_for_agent(
            &agent_id,
            rua_agent::memory_manager::MemoryGetRequest {
                ids: vec![id.clone()],
                include_inactive: false,
            },
        )
        .await
    {
        Ok(mut records) => match records.pop() {
            Some(_) => {}
            None => {
                return ApiError::NotFound("memory record not found".to_string()).into_response()
            }
        },
        Err(e) => return ApiError::Internal(e.to_string()).into_response(),
    }

    let req = rua_agent::memory_manager::MemoryUpdateRequest {
        id,
        agent_id,
        kind: body.kind,
        scope: body.scope,
        title: body.title,
        content: body.content,
        confidence: body.confidence,
        importance: body.importance,
        search_terms: Some(body.search_terms),
    };

    match mm.memory_update(req).await {
        Ok(Some(record)) => {
            (StatusCode::OK, Json(serde_json::to_value(record).unwrap())).into_response()
        }
        Ok(None) => ApiError::NotFound("memory record not found".to_string()).into_response(),
        Err(e) => ApiError::Internal(e.to_string()).into_response(),
    }
}

async fn health_check_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Response {
    do_health_check(state, &agent_id).await
}

async fn do_health_check(state: AppState, agent_id: &str) -> Response {
    let mm = match state.memory_manager_for(agent_id).await {
        Some(mm) => mm,
        None => return memory_manager_unavailable(),
    };

    match mm.health_check().await {
        Ok(status) => (StatusCode::OK, Json(serde_json::to_value(status).unwrap())).into_response(),
        Err(e) => ApiError::Internal(e.to_string()).into_response(),
    }
}

fn memory_manager_unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({
            "detail": "memory manager not configured"
        })),
    )
        .into_response()
}

fn inject_memory_device_context(
    metadata: serde_json::Value,
    device_id: Option<&str>,
    device_name: Option<&str>,
    host_profile: Option<&str>,
) -> serde_json::Value {
    let mut object = metadata.as_object().cloned().unwrap_or_default();
    insert_optional(&mut object, "written_on_device_id", device_id);
    insert_optional(&mut object, "written_on_device_name_snapshot", device_name);
    insert_optional(&mut object, "written_on_host_profile", host_profile);
    insert_optional(&mut object, "resource_device_id", device_id);
    insert_optional(&mut object, "resource_device_name_snapshot", device_name);
    serde_json::Value::Object(object)
}

fn insert_optional(
    object: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: Option<&str>,
) {
    if let Some(value) = value {
        object.insert(
            key.to_string(),
            serde_json::Value::String(value.to_string()),
        );
    } else {
        object.remove(key);
    }
}

async fn runtime_for_agent(
    state: &AppState,
    agent_id: &str,
) -> Result<Arc<WorkspaceRuntime>, ApiError> {
    state
        .registry
        .get_or_create(agent_id)
        .await
        .map_err(|e| ApiError::Unavailable(format!("workspace for '{agent_id}' unavailable: {e}")))
}

async fn diagnose_memory_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Response {
    let runtime = match runtime_for_agent(&state, &agent_id).await {
        Ok(rt) => rt,
        Err(e) => return e.into_response(),
    };
    do_diagnose(&runtime).await
}

async fn do_diagnose(runtime: &WorkspaceRuntime) -> Response {
    let ws_dir = runtime.workspace_dir();
    let maintenance = rua_memory::MemoryMaintenance::new(&ws_dir);
    match maintenance.diagnose().await {
        Ok(plan) => (StatusCode::OK, Json(serde_json::to_value(plan).unwrap())).into_response(),
        Err(e) => ApiError::Internal(e.to_string()).into_response(),
    }
}

async fn repair_memory_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Response {
    let runtime = match runtime_for_agent(&state, &agent_id).await {
        Ok(rt) => rt,
        Err(e) => return e.into_response(),
    };
    do_repair(&runtime, &state).await
}

async fn do_repair(runtime: &WorkspaceRuntime, _state: &AppState) -> Response {
    let ws_dir = runtime.workspace_dir();
    let maintenance = rua_memory::MemoryMaintenance::new(&ws_dir);
    match maintenance.repair().await {
        Ok(actions) => (
            StatusCode::OK,
            Json(serde_json::json!({ "actions": actions })),
        )
            .into_response(),
        Err(e) => ApiError::Internal(e.to_string()).into_response(),
    }
}

async fn reindex_memory_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Response {
    let runtime = match runtime_for_agent(&state, &agent_id).await {
        Ok(rt) => rt,
        Err(e) => return e.into_response(),
    };
    let mm = match runtime.memory_manager().await {
        Some(mm) => mm,
        None => return memory_manager_unavailable(),
    };
    match mm.reindex(&runtime.workspace_dir()).await {
        Ok(count) => (
            StatusCode::OK,
            Json(serde_json::json!({ "documents_indexed": count })),
        )
            .into_response(),
        Err(e) => ApiError::Internal(e.to_string()).into_response(),
    }
}
