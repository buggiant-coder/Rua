use std::path::PathBuf;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use rua_config::{AgentTemplate, AgentTemplateStore};

use crate::endpoints::dto::{
    self,
    template::{agent_template_detail, agent_template_response},
};
use crate::errors::ApiError;
use crate::middleware::AgentExtractorContext;
use crate::state::AppState;
use crate::sync_publication::{publish_sync_fact, SyncPublicationFact};

pub fn router() -> Router<AppState> {
    let collection = get(list_templates).post(create_template);
    Router::new()
        .route("/agent-templates", collection.clone())
        .route("/agent-templates/", collection)
        .route(
            "/agent-templates/{template_id}",
            get(get_template)
                .put(update_template)
                .delete(delete_template),
        )
}

struct StoreCtx {
    store: AgentTemplateStore,
    rua_root: PathBuf,
}

async fn build_ctx(state: &AppState) -> Result<StoreCtx, ApiError> {
    let runtime = state.registry.get_or_create("default").await.map_err(|e| {
        ApiError::Internal(format!(
            "failed to obtain workspace runtime for agent templates: {e}"
        ))
    })?;
    let rua_root = runtime.rua_root();
    Ok(StoreCtx {
        store: AgentTemplateStore::new(rua_root.clone()),
        rua_root,
    })
}

async fn publish_agent_template_committed(
    state: &AppState,
    template_id: &str,
    template: serde_json::Value,
) {
    let _ = publish_sync_fact(
        state,
        SyncPublicationFact::AgentTemplateCommitted {
            template_id: template_id.to_string(),
            template,
        },
    )
    .await;
}

async fn publish_agent_template_deleted(state: &AppState, template_id: &str) {
    let _ = publish_sync_fact(
        state,
        SyncPublicationFact::AgentTemplateDeleted {
            template_id: template_id.to_string(),
        },
    )
    .await;
}

fn slugify_template_id(name: &str) -> String {
    name.to_lowercase()
        .replace(' ', "-")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect::<String>()
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

async fn list_templates(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
) -> Response {
    let ctx = match build_ctx(&state).await {
        Ok(ctx) => ctx,
        Err(error) => return error.into_response(),
    };
    match ctx.store.list() {
        Ok(entries) => {
            let result: Vec<dto::AgentTemplateResponse> = entries
                .into_iter()
                .map(|(id, t)| agent_template_response(id, t))
                .collect();
            (StatusCode::OK, Json(result)).into_response()
        }
        Err(e) => ApiError::Internal(format!("read agent_templates.json: {e}")).into_response(),
    }
}

async fn get_template(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Path(template_id): Path<String>,
) -> Response {
    let ctx = match build_ctx(&state).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    match ctx.store.get(&template_id) {
        Ok(Some(template)) => {
            let prompt_library_dir = ctx.rua_root.join("prompt_library");
            let detail = agent_template_detail(template_id, template, &prompt_library_dir);
            (StatusCode::OK, Json(detail)).into_response()
        }
        Ok(None) => {
            ApiError::NotFound(format!("Agent template '{template_id}' not found")).into_response()
        }
        Err(e) => {
            ApiError::Internal(format!("failed to read agent_templates.json: {e}")).into_response()
        }
    }
}

async fn create_template(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Json(body): Json<dto::AgentTemplateCreate>,
) -> Response {
    let ctx = match build_ctx(&state).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    let id = slugify_template_id(&body.name);

    match ctx.store.get(&id) {
        Ok(Some(_)) => {
            return ApiError::Conflict(format!("Template '{id}' already exists")).into_response();
        }
        Err(e) => {
            return ApiError::Internal(format!("failed to read agent_templates.json: {e}"))
                .into_response();
        }
        Ok(None) => {}
    }

    let now = now_iso();
    let template = AgentTemplate {
        name: body.name,
        description: body.description,
        prompt_files: body.prompt_files,
        builtin: false,
        created_at: now.clone(),
        updated_at: now,
        extra: body.extra,
    };

    let content = match serde_json::to_value(&template) {
        Ok(v) => v,
        Err(e) => {
            return ApiError::Internal(format!("serialize template: {e}")).into_response();
        }
    };

    if let Err(e) = ctx.store.create(&id, template.clone()) {
        return ApiError::Internal(format!("write agent_templates.json: {e}")).into_response();
    }

    publish_agent_template_committed(&state, &id, content).await;

    let resp = agent_template_response(id, template);
    (StatusCode::CREATED, Json(resp)).into_response()
}

async fn update_template(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Path(template_id): Path<String>,
    Json(body): Json<dto::AgentTemplateUpdate>,
) -> Response {
    let ctx = match build_ctx(&state).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    let mut template = match ctx.store.get(&template_id) {
        Ok(Some(t)) => t,
        Ok(None) => {
            return ApiError::NotFound(format!("Agent template '{template_id}' not found"))
                .into_response();
        }
        Err(e) => {
            return ApiError::Internal(format!("failed to read agent_templates.json: {e}"))
                .into_response();
        }
    };
    if let Some(name) = body.name {
        template.name = name;
    }
    if let Some(description) = body.description {
        template.description = description;
    }
    if let Some(prompt_files) = body.prompt_files {
        template.prompt_files = prompt_files;
    }
    for (k, v) in body.extra {
        template.extra.insert(k, v);
    }
    template.updated_at = now_iso();

    let content = match serde_json::to_value(&template) {
        Ok(v) => v,
        Err(e) => {
            return ApiError::Internal(format!("serialize template: {e}")).into_response();
        }
    };

    if let Err(e) = ctx.store.update(&template_id, template.clone()) {
        return ApiError::Internal(format!("write agent_templates.json: {e}")).into_response();
    }

    publish_agent_template_committed(&state, &template_id, content).await;

    let resp = agent_template_response(template_id, template);
    (StatusCode::OK, Json(resp)).into_response()
}

async fn delete_template(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Path(template_id): Path<String>,
) -> Response {
    let ctx = match build_ctx(&state).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    match ctx.store.get(&template_id) {
        Ok(None) => {
            return ApiError::NotFound(format!("Agent template '{template_id}' not found"))
                .into_response();
        }
        Ok(Some(t)) if t.builtin => {
            return ApiError::BadRequest(format!("Cannot delete builtin template '{template_id}'"))
                .into_response();
        }
        Err(e) => {
            return ApiError::Internal(format!("failed to read agent_templates.json: {e}"))
                .into_response();
        }
        Ok(Some(_)) => {}
    }

    if let Err(e) = ctx.store.delete(&template_id) {
        return ApiError::Internal(format!("delete from agent_templates.json: {e}"))
            .into_response();
    }

    publish_agent_template_deleted(&state, &template_id).await;

    (StatusCode::OK, Json(dto::SuccessResponse::ok())).into_response()
}
