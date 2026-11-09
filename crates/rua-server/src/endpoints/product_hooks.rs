use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;

use crate::errors::ApiError;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
struct ActivationBody {
    definition_id: String,
    enabled: bool,
}

#[derive(Debug, Default, Deserialize)]
struct RunsQuery {
    chat_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ContributionsQuery {
    chat_id: String,
    turn_id: String,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/agents/{agentId}/hooks/definitions",
            get(list_definitions).post(create_definition),
        )
        .route(
            "/agents/{agentId}/hooks/definitions/{ownerNamespace}/{logicalId}",
            get(get_definition).put(revise_definition),
        )
        .route(
            "/agents/{agentId}/hooks/defaults",
            put(update_agent_defaults),
        )
        .route(
            "/agents/{agentId}/hooks/chats/{chatId}/configuration",
            get(get_chat_configuration),
        )
        .route("/agents/{agentId}/hooks/runs", get(list_runs))
        .route(
            "/agents/{agentId}/hooks/contributions",
            get(list_contributions),
        )
        .route(
            "/agents/{agentId}/hooks/effective-preview",
            post(effective_preview),
        )
        .route(
            "/projects/{projectId}/hooks/definitions",
            get(list_project_definitions),
        )
        .route(
            "/projects/{projectId}/hooks/defaults",
            put(update_project_defaults),
        )
}

async fn effective_preview(
    Path(_agent_id): Path<String>,
    Json(input): Json<crate::services::scope_composition::CompositionInput>,
) -> Response {
    match crate::services::scope_composition::compose(input) {
        Ok(preview) => Json(preview).into_response(),
        Err(detail) => ApiError::BadRequest(detail).into_response(),
    }
}

async fn list_definitions(State(state): State<AppState>, Path(agent_id): Path<String>) -> Response {
    Json(state.product_hooks.definitions_for_agent(&agent_id).await).into_response()
}

async fn list_project_definitions(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Response {
    Json(
        state
            .product_hooks
            .definitions_for_project(&project_id)
            .await,
    )
    .into_response()
}

async fn create_definition(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<crate::services::product_hooks::CreateUserHookDefinition>,
) -> Response {
    match state
        .product_hooks
        .create_user_definition(&agent_id, body)
        .await
    {
        Ok(definition) => (axum::http::StatusCode::CREATED, Json(definition)).into_response(),
        Err(detail) => ApiError::BadRequest(detail).into_response(),
    }
}

async fn get_definition(
    State(state): State<AppState>,
    Path((agent_id, owner_namespace, logical_id)): Path<(String, String, String)>,
) -> Response {
    match state
        .product_hooks
        .definition_detail(&agent_id, &owner_namespace, &logical_id)
        .await
    {
        Some(definition) => Json(definition).into_response(),
        None => ApiError::NotFound(format!("{owner_namespace}/{logical_id}")).into_response(),
    }
}

async fn revise_definition(
    State(state): State<AppState>,
    Path((agent_id, owner_namespace, logical_id)): Path<(String, String, String)>,
    Json(body): Json<crate::services::product_hooks::ReviseUserHookDefinition>,
) -> Response {
    match state
        .product_hooks
        .revise_user_definition(&agent_id, &owner_namespace, &logical_id, body)
        .await
    {
        Ok(definition) => Json(definition).into_response(),
        Err(detail) if detail.contains("not found") => ApiError::NotFound(detail).into_response(),
        Err(detail) => ApiError::BadRequest(detail).into_response(),
    }
}

async fn update_agent_defaults(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<ActivationBody>,
) -> Response {
    if let Err(detail) = state
        .product_hooks
        .set_agent_activation(&agent_id, &body.definition_id, body.enabled)
        .await
    {
        return ApiError::NotFound(detail).into_response();
    }
    Json(state.product_hooks.definitions_for_agent(&agent_id).await).into_response()
}

async fn update_project_defaults(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Json(body): Json<ActivationBody>,
) -> Response {
    if let Err(detail) = state
        .product_hooks
        .set_project_activation(&project_id, &body.definition_id, body.enabled)
        .await
    {
        return ApiError::NotFound(detail).into_response();
    }
    Json(
        state
            .product_hooks
            .definitions_for_project(&project_id)
            .await,
    )
    .into_response()
}

async fn get_chat_configuration(
    State(state): State<AppState>,
    Path((agent_id, chat_id)): Path<(String, String)>,
) -> Response {
    let address = rua_core::ChatAddress::agent(&agent_id, &chat_id);
    let scope = rua_core::ChatScopeStore::resolve(&state.rua_root, address);
    let store = rua_session::WorkspaceSessionStore::new(scope.root());
    let header = match store.load_chat_header(&chat_id).await {
        Ok(Some(header)) => header,
        Ok(None) => return ApiError::NotFound(chat_id).into_response(),
        Err(error) => return ApiError::from(error).into_response(),
    };
    let Some(value) = header
        .extra
        .get(crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY)
    else {
        return ApiError::NotFound("Chat configuration snapshot missing".to_string())
            .into_response();
    };
    let configuration: crate::services::product_hooks::ChatHookConfiguration =
        match serde_json::from_value(value.clone()) {
            Ok(configuration) => configuration,
            Err(error) => {
                return ApiError::Internal(format!("invalid stored Chat configuration: {error}"))
                    .into_response()
            }
        };
    match configuration.validate() {
        Ok(()) => Json(configuration).into_response(),
        Err(error) => ApiError::Internal(error).into_response(),
    }
}

async fn list_runs(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Query(query): Query<RunsQuery>,
) -> Response {
    Json(
        state
            .product_hooks
            .runs(&agent_id, query.chat_id.as_deref())
            .await,
    )
    .into_response()
}

async fn list_contributions(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Query(query): Query<ContributionsQuery>,
) -> Response {
    let address = rua_core::ChatAddress::agent(agent_id, query.chat_id);
    Json(
        state
            .product_hooks
            .contributions(&address, &query.turn_id)
            .await,
    )
    .into_response()
}
