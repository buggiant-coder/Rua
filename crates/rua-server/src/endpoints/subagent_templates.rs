use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use rua_config::SubagentTemplateStore;
use serde::Deserialize;

use crate::endpoints::dto::{self, template::parse_subagent_template};
use crate::errors::ApiError;
use crate::state::AppState;
use crate::sync_publication::{publish_sync_fact, SyncPublicationFact};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/subagent-templates",
            get(list_templates).post(create_template),
        )
        .route(
            "/subagent-templates/{template_id}",
            get(get_template)
                .put(update_template)
                .delete(delete_template),
        )
}

#[derive(Debug, Deserialize)]
pub struct ListSubagentTemplatesQuery {
    #[serde(default)]
    pub agent_id: Option<String>,
}

async fn build_store(state: &AppState) -> Result<SubagentTemplateStore, ApiError> {
    let runtime = state.registry.get_or_create("default").await.map_err(|e| {
        ApiError::Internal(format!(
            "failed to obtain workspace runtime for subagent templates: {e}"
        ))
    })?;
    Ok(SubagentTemplateStore::new(runtime.rua_root()))
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

async fn publish_subagent_templates_after_write(state: &AppState, store: &SubagentTemplateStore) {
    let Ok(file) = store.load_file() else {
        return;
    };
    let value = serde_json::to_value(file).unwrap_or_default();
    let _ = publish_sync_fact(
        state,
        SyncPublicationFact::SubagentTemplatesCommitted { templates: value },
    )
    .await;
}

async fn list_templates(
    State(state): State<AppState>,
    Query(_query): Query<ListSubagentTemplatesQuery>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match store.list() {
        Ok(values) => {
            let templates: Vec<dto::SubagentTemplateResponse> = values
                .into_iter()
                .filter_map(parse_subagent_template)
                .collect();
            (StatusCode::OK, Json(templates)).into_response()
        }
        Err(e) => ApiError::Internal(format!("failed to read subagent_templates.json: {e}"))
            .into_response(),
    }
}

async fn get_template(State(state): State<AppState>, Path(template_id): Path<String>) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match store.get(&template_id) {
        Ok(Some(value)) => match parse_subagent_template(value) {
            Some(t) => (StatusCode::OK, Json(t)).into_response(),
            None => ApiError::Internal(format!(
                "subagent template '{template_id}' is malformed on disk"
            ))
            .into_response(),
        },
        Ok(None) => ApiError::NotFound(format!("SubAgent template '{template_id}' not found"))
            .into_response(),
        Err(e) => ApiError::Internal(format!("failed to read subagent_templates.json: {e}"))
            .into_response(),
    }
}

async fn create_template(
    State(state): State<AppState>,
    Json(body): Json<dto::SubagentTemplateCreate>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    let id = uuid::Uuid::new_v4().to_string();
    let now = now_iso();
    let template = dto::SubagentTemplateResponse {
        id: id.clone(),
        name: body.name,
        description: body.description,
        system_prompt: body.system_prompt,
        tools: body.tools,
        visible_to: body.visible_to,
        builtin: false,
        created_at: Some(now.clone()),
        updated_at: Some(now),
        extra: body.extra,
    };
    let value = match serde_json::to_value(&template) {
        Ok(v) => v,
        Err(e) => {
            return ApiError::Internal(format!("failed to serialize subagent template: {e}"))
                .into_response();
        }
    };
    match store.create(value) {
        Ok(true) => {
            publish_subagent_templates_after_write(&state, &store).await;
            (StatusCode::CREATED, Json(template)).into_response()
        }
        Ok(false) => {
            ApiError::Conflict(format!("Subagent template '{id}' already exists")).into_response()
        }
        Err(e) => ApiError::Internal(format!("failed to write subagent_templates.json: {e}"))
            .into_response(),
    }
}

async fn update_template(
    State(state): State<AppState>,
    Path(template_id): Path<String>,
    Json(body): Json<dto::SubagentTemplateUpdate>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };

    let mut current = match store.get(&template_id) {
        Ok(Some(v)) => match parse_subagent_template(v) {
            Some(t) => t,
            None => {
                return ApiError::Internal(format!(
                    "subagent template '{template_id}' is malformed on disk"
                ))
                .into_response();
            }
        },
        Ok(None) => {
            return ApiError::NotFound(format!("SubAgent template '{template_id}' not found"))
                .into_response();
        }
        Err(e) => {
            return ApiError::Internal(format!("failed to read subagent_templates.json: {e}"))
                .into_response();
        }
    };

    if let Some(name) = body.name {
        current.name = name;
    }
    if let Some(description) = body.description {
        current.description = description;
    }
    if let Some(system_prompt) = body.system_prompt {
        current.system_prompt = system_prompt;
    }
    if let Some(tools) = body.tools {
        current.tools = tools;
    }
    if let Some(visible_to) = body.visible_to {
        current.visible_to = visible_to;
    }
    for (k, v) in body.extra {
        current.extra.insert(k, v);
    }
    current.updated_at = Some(now_iso());

    let value = match serde_json::to_value(&current) {
        Ok(v) => v,
        Err(e) => {
            return ApiError::Internal(format!("failed to serialize subagent template: {e}"))
                .into_response();
        }
    };
    match store.update(&template_id, value) {
        Ok(true) => {
            publish_subagent_templates_after_write(&state, &store).await;
            (StatusCode::OK, Json(current)).into_response()
        }
        Ok(false) => ApiError::NotFound(format!("SubAgent template '{template_id}' not found"))
            .into_response(),
        Err(e) => ApiError::Internal(format!("failed to write subagent_templates.json: {e}"))
            .into_response(),
    }
}

async fn delete_template(
    State(state): State<AppState>,
    Path(template_id): Path<String>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match store.delete(&template_id) {
        Ok(true) => {
            publish_subagent_templates_after_write(&state, &store).await;
            (StatusCode::OK, Json(dto::SuccessResponse::ok())).into_response()
        }
        Ok(false) => ApiError::NotFound(format!("SubAgent template '{template_id}' not found"))
            .into_response(),
        Err(e) => ApiError::Internal(format!("failed to write subagent_templates.json: {e}"))
            .into_response(),
    }
}
