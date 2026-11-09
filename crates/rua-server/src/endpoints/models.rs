use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete as delete_route, get, post, put};
use axum::{Extension, Json, Router};
use llm_multiplex::{ConnectionStatus, LlmError};
use rua_provider::config_loader::{
    DefaultModel as LoaderDefaultModel, ProviderConfig, ProviderModelEntry as LoaderModelEntry,
};
use rua_session::{
    early_background_compaction_coordinator, CheckpointModelIdentity, ContextProjectionRequest,
    ModelSwitchPreflight, ModelSwitchPreflightDecision, ModelSwitchPreflightInput,
    ProjectionFoldFrontier, WorkspaceSessionStore,
};
use serde::{Deserialize, Serialize};

use crate::endpoints::dto::{
    CustomProviderCreateDto, DefaultModelDto, DefaultModelResponseDto, ModelDto, ProbeRequestDto,
    ProbeResultDto, ProviderDto, ProviderTemplateInfoDto, ProviderUpdateDto,
};
use crate::errors::ApiError;
use crate::middleware::AgentExtractorContext;
use crate::services::context_projection::context_target_band_for_model;
use crate::state::AppState;
use crate::sync_publication::{publish_sync_fact, SyncPublicationFact};

async fn publish_provider_config_committed(
    state: &AppState,
    provider_id: impl Into<String>,
    config: &ProviderConfig,
) {
    let config_val = serde_json::to_value(config).unwrap_or_default();
    let _ = publish_sync_fact(
        state,
        SyncPublicationFact::ProviderConfigCommitted {
            provider_id: provider_id.into(),
            config: config_val,
        },
    )
    .await;
}

async fn publish_provider_config_deleted(state: &AppState, provider_id: impl Into<String>) {
    let _ = publish_sync_fact(
        state,
        SyncPublicationFact::ProviderConfigDeleted {
            provider_id: provider_id.into(),
        },
    )
    .await;
}

#[derive(Debug, Serialize)]
struct TestResult {
    success: bool,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Serialize)]
struct DiscoverResult {
    models: Vec<ModelDto>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TestModelRequest {
    model_id: String,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct ModelIdRequest {
    model_id: String,
}

#[derive(Debug, Deserialize)]
struct ModelConfigUpdate {
    #[serde(default)]
    model_id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    context_window: Option<Option<u64>>,
    #[serde(default)]
    thinking_enabled: Option<bool>,
    #[serde(default)]
    thinking_budget: Option<i64>,
    #[serde(default)]
    reasoning_effort: Option<String>,
    #[serde(default)]
    thinking_param_style: Option<String>,
    #[serde(default)]
    reasoning_effort_options: Option<Vec<String>>,
    #[serde(default)]
    thinking_budget_range: Option<[i64; 2]>,
    #[serde(default)]
    relay_reasoning: Option<bool>,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

fn deserialize_present_nullable<'de, D>(deserializer: D) -> Result<Option<Option<u64>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<u64>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Deserialize)]
struct DefaultModelQuery {
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    agent_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct SetDefaultModelRequest {
    #[serde(default)]
    provider_id: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    chat_id: Option<String>,
    #[serde(default)]
    confirm_context_checkpoint: bool,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct ModelSwitchCheckpointRequiredResponse {
    detail: String,
    code: &'static str,
    chat_id: String,
    target_model: DefaultModelDto,
    estimated_projection_tokens: u64,
    target_upper_tokens: u64,
    emergency_limit_tokens: u64,
    #[serde(default)]
    recommended_fold_frontier: Option<ModelSwitchFoldFrontierDto>,
}

#[derive(Debug, Serialize)]
struct ModelSwitchFoldFrontierDto {
    start_turn_ordinal: u64,
    end_turn_ordinal: u64,
    start_sequence: u64,
    end_sequence: u64,
    estimated_tokens_to_fold: u64,
}

#[derive(Debug, Serialize)]
struct ProbeMultimodalResponse {
    supports_multimodal: Option<bool>,
    supports_image: Option<bool>,
    supports_video: Option<bool>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterFilterRequest {
    #[serde(default)]
    providers: Option<Vec<String>>,
    #[serde(default)]
    input_modalities: Option<Vec<String>>,
    #[serde(default)]
    output_modalities: Option<Vec<String>>,
    #[serde(default)]
    max_prompt_price: Option<f64>,
}

#[derive(Debug, Serialize)]
struct DeleteProviderResponse {
    success: bool,
    message: String,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/models", get(list_providers))
        .route("/models/providers", get(list_providers))
        .route(
            "/models/active",
            get(get_default_model)
                .put(set_default_model)
                .delete(clear_default_model),
        )
        .route(
            "/models/providers/active",
            get(get_default_model)
                .put(set_default_model)
                .delete(clear_default_model),
        )
        .route(
            "/models/providers/{id}",
            get(get_provider)
                .put(update_provider)
                .delete(delete_provider),
        )
        .route("/models/probe", post(probe_provider))
        .route("/models/custom-providers", post(add_custom_provider))
        .route("/models/templates", get(list_templates))
        .route("/models/{provider_id}/config", put(update_provider_config))
        .route("/models/{provider_id}/test", post(test_provider))
        .route("/models/{provider_id}/discover", post(discover_models))
        .route("/models/{provider_id}/models", post(add_model))
        .route(
            "/models/{provider_id}/models/delete",
            post(delete_model_by_body),
        )
        .route(
            "/models/{provider_id}/models/config",
            put(update_model_config_by_body),
        )
        .route(
            "/models/{provider_id}/models/probe-multimodal",
            post(probe_multimodal_by_body),
        )
        .route(
            "/models/{provider_id}/models/{model_id}",
            delete_route(delete_model),
        )
        .route(
            "/models/{provider_id}/models/{model_id}/config",
            put(update_model_config),
        )
        .route("/models/{provider_id}/models/test", post(test_model))
        .route(
            "/models/{provider_id}/models/{model_id}/probe-multimodal",
            post(probe_multimodal),
        )
        .route("/models/openrouter/series", get(openrouter_series))
        .route(
            "/models/openrouter/discover-extended",
            post(openrouter_discover_extended),
        )
        .route(
            "/models/openrouter/models/filter",
            post(openrouter_models_filter),
        )
        .route("/agents/{agentId}/models", get(list_providers_for_agent))
        .route(
            "/agents/{agentId}/models/providers/active",
            get(get_default_model_for_agent).put(set_default_model_for_agent),
        )
}

fn is_redacted_key(key: &str) -> bool {
    key.contains("...") || key.contains("******") || key.chars().all(|c| c == '*')
}

async fn load_provider_config(
    state: &AppState,
    provider_id: &str,
) -> Result<ProviderConfig, ApiError> {
    match state.provider_cells.cell(provider_id).read().await {
        Ok(Some(cfg)) => Ok(cfg),
        Ok(None) => Err(ApiError::NotFound(format!(
            "Provider '{provider_id}' not found"
        ))),
        Err(e) => Err(ApiError::from(e)),
    }
}

fn dto_model_to_loader(m: ModelDto) -> LoaderModelEntry {
    let generate_kwargs = m
        .generate_kwargs
        .as_ref()
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    let mut extra = m.extra;
    if let Some(is_free) = m.is_free {
        extra.insert("is_free".into(), serde_json::Value::Bool(is_free));
    }
    LoaderModelEntry {
        id: m.id,
        name: m.name,
        supports_multimodal: m.supports_multimodal,
        supports_image: m.supports_image,
        supports_video: m.supports_video,
        probe_source: m.probe_source,
        context_window: m.context_window,
        max_output_tokens: m.max_output_tokens,
        generate_kwargs,
        thinking_enabled: m.thinking_enabled,
        thinking_budget: m.thinking_budget,
        reasoning_effort: m.reasoning_effort,
        thinking_param_style: m.thinking_param_style,
        reasoning_effort_options: m.reasoning_effort_options,
        thinking_budget_range: m.thinking_budget_range,
        relay_reasoning: m.relay_reasoning,
        extra,
    }
}

fn model_dto_from_discovered_model(provider_id: &str, model: llm_multiplex::ModelInfo) -> ModelDto {
    let mut dto = ModelDto::from(model);
    if dto.is_free.is_none() {
        dto.is_free = state_template(provider_id)
            .and_then(|template| template.preset_model_is_free(&dto.id).then_some(true));
    }
    dto
}

fn state_template(provider_id: &str) -> Option<&'static rua_provider::manager::ProviderTemplate> {
    rua_provider::manager::PROVIDER_TEMPLATES
        .iter()
        .find(|template| template.id == provider_id)
}

async fn list_effective_provider_configs(
    state: &AppState,
) -> Result<Vec<ProviderConfig>, ApiError> {
    let mut configs = state.provider_cells.list().await.map_err(ApiError::from)?;
    let existing: std::collections::HashSet<String> =
        configs.iter().map(|config| config.id.clone()).collect();
    configs.extend(
        state
            .provider_manager
            .list_templates()
            .iter()
            .filter(|template| template.is_zero_config_free_tier())
            .filter(|template| !existing.contains(template.id))
            .map(|template| template.to_config()),
    );
    Ok(configs)
}

async fn ensure_zero_config_free_provider(
    state: &AppState,
    provider_id: &str,
) -> Result<(), ApiError> {
    if state.provider_manager.get(provider_id).is_some() {
        return Ok(());
    }
    if state
        .provider_cells
        .cell(provider_id)
        .read()
        .await?
        .is_some()
    {
        return Ok(());
    }
    let Some(template) = state
        .provider_manager
        .list_templates()
        .iter()
        .find(|template| template.id == provider_id && template.is_zero_config_free_tier())
    else {
        return Ok(());
    };
    let config = template.to_config();
    state
        .provider_cells
        .cell(&config.id)
        .write(config.clone())
        .await?;
    let provider = rua_provider::manager::create_provider_from_config(&config)?;
    state.provider_manager.register(provider);
    publish_provider_config_committed(state, config.id.clone(), &config).await;
    Ok(())
}

fn default_model_error_response(err: LlmError) -> Response {
    match err {
        LlmError::Config { message }
            if message.to_lowercase().contains("provider")
                && message.to_lowercase().contains("not found") =>
        {
            ApiError::NotFound(message).into_response()
        }
        other => ApiError::from(other).into_response(),
    }
}

async fn model_switch_preflight_response(
    state: &AppState,
    ctx: &AgentExtractorContext,
    body: &SetDefaultModelRequest,
) -> Option<Response> {
    let chat_id = body.chat_id.as_deref()?.trim();
    if chat_id.is_empty() {
        return None;
    }

    state
        .provider_manager
        .get_model_context_window(&body.provider_id, &body.model)
        .await?;

    let workspace_dir = ctx.runtime.workspace_dir();
    let store = WorkspaceSessionStore::new(&workspace_dir);
    let header = match store.load_chat_header(chat_id).await {
        Ok(Some(header)) => header,
        Ok(None) => return None,
        Err(error) => return Some(ApiError::from(error).into_response()),
    };
    let running = match store.load_chat_running_config(chat_id).await {
        Ok(Some(configuration)) => match configuration.resolve() {
            Ok(running) => running,
            Err(error) => return Some(ApiError::Internal(error).into_response()),
        },
        Ok(None) => {
            return Some(
                ApiError::Internal("Chat running configuration is missing".into()).into_response(),
            )
        }
        Err(error) => return Some(ApiError::from(error).into_response()),
    };

    let global_max = running.max_input_length.max(1) as u64;
    let effective_context_window = state
        .provider_manager
        .get_known_model_context_window(&body.provider_id, &body.model)
        .await
        .map(|window| window.min(global_max))
        .unwrap_or(global_max);
    let target_band = context_target_band_for_model(&running, effective_context_window);
    let target_model = CheckpointModelIdentity {
        provider: body.provider_id.clone(),
        model: body.model.clone(),
    };
    let projection = match store
        .build_effective_context_projection(
            chat_id,
            ContextProjectionRequest {
                current_user_input: None,
            },
            running.context_compact.history_replacement_enabled,
            running.context_compact.token_count_estimate_divisor,
        )
        .await
    {
        Ok(projection) => projection,
        Err(e) => return Some(ApiError::from(e).into_response()),
    };
    let mut message_groups = match store
        .projection_message_group_estimates_effective(
            chat_id,
            running.context_compact.history_replacement_enabled,
            running.context_compact.token_count_estimate_divisor,
        )
        .await
    {
        Ok(message_groups) => message_groups,
        Err(e) => return Some(ApiError::from(e).into_response()),
    };
    rua_agent::projection_preparation::finalize_projection_message_group_estimates(
        &projection,
        &mut message_groups,
        &running,
    );
    let current_projection_tokens =
        rua_agent::projection_preparation::final_projection_token_estimate(&projection, &running);
    let checkpoint_tokens = match store.load_active_summary_checkpoint(chat_id).await {
        Ok(checkpoint) => checkpoint
            .map(|checkpoint| checkpoint.token_estimate)
            .unwrap_or_default(),
        Err(e) => return Some(ApiError::from(e).into_response()),
    };
    let Some(current_model) =
        crate::services::history_replacement::checkpoint_model_from_header(&header)
    else {
        return Some(
            ApiError::Internal("Chat model configuration is missing".into()).into_response(),
        );
    };

    match ModelSwitchPreflight::evaluate(ModelSwitchPreflightInput {
        current_model: current_model.clone(),
        target_model: target_model.clone(),
        target_band,
        current_projection_tokens,
        checkpoint_tokens,
        system_context_tokens: 0,
        pending_user_input_tokens: 0,
        message_groups,
    }) {
        ModelSwitchPreflightDecision::Allow { .. } => None,
        ModelSwitchPreflightDecision::ConfirmationRequired {
            estimated_projection_tokens,
            target_upper_tokens,
            emergency_limit_tokens,
            recommended_fold_frontier,
            ..
        } => {
            if body.confirm_context_checkpoint {
                let checkpoint_result = publish_confirmed_model_switch_checkpoint(
                    ctx,
                    chat_id,
                    &target_model,
                    recommended_fold_frontier.clone(),
                    &running,
                )
                .await;
                if let Err(response) = checkpoint_result {
                    return Some(response);
                }

                let projection = match store
                    .build_effective_context_projection(
                        chat_id,
                        ContextProjectionRequest {
                            current_user_input: None,
                        },
                        running.context_compact.history_replacement_enabled,
                        running.context_compact.token_count_estimate_divisor,
                    )
                    .await
                {
                    Ok(projection) => projection,
                    Err(e) => return Some(ApiError::from(e).into_response()),
                };
                let mut message_groups = match store
                    .projection_message_group_estimates_effective(
                        chat_id,
                        running.context_compact.history_replacement_enabled,
                        running.context_compact.token_count_estimate_divisor,
                    )
                    .await
                {
                    Ok(message_groups) => message_groups,
                    Err(e) => return Some(ApiError::from(e).into_response()),
                };
                rua_agent::projection_preparation::finalize_projection_message_group_estimates(
                    &projection,
                    &mut message_groups,
                    &running,
                );
                let current_projection_tokens =
                    rua_agent::projection_preparation::final_projection_token_estimate(
                        &projection,
                        &running,
                    );
                let checkpoint_tokens = match store.load_active_summary_checkpoint(chat_id).await {
                    Ok(checkpoint) => checkpoint
                        .map(|checkpoint| checkpoint.token_estimate)
                        .unwrap_or_default(),
                    Err(e) => return Some(ApiError::from(e).into_response()),
                };
                let after = ModelSwitchPreflight::evaluate(ModelSwitchPreflightInput {
                    current_model: current_model.clone(),
                    target_model: target_model.clone(),
                    target_band,
                    current_projection_tokens,
                    checkpoint_tokens,
                    system_context_tokens: 0,
                    pending_user_input_tokens: 0,
                    message_groups,
                });
                if matches!(after, ModelSwitchPreflightDecision::Allow { .. }) {
                    return None;
                }
            }

            let response = ModelSwitchCheckpointRequiredResponse {
                detail: "context checkpoint required before switching model".to_string(),
                code: "context_checkpoint_required",
                chat_id: chat_id.to_string(),
                target_model: DefaultModelDto {
                    provider_id: target_model.provider,
                    model: target_model.model,
                    extra: serde_json::Map::new(),
                },
                estimated_projection_tokens,
                target_upper_tokens,
                emergency_limit_tokens,
                recommended_fold_frontier: recommended_fold_frontier.map(fold_frontier_dto),
            };
            Some((StatusCode::CONFLICT, Json(response)).into_response())
        }
    }
}

async fn publish_confirmed_model_switch_checkpoint(
    ctx: &AgentExtractorContext,
    chat_id: &str,
    target_model: &CheckpointModelIdentity,
    frontier: Option<ProjectionFoldFrontier>,
    running: &rua_config::AgentsRunningConfig,
) -> Result<(), Response> {
    let Some(frontier) = frontier else {
        return Err(ApiError::Conflict(
            "context checkpoint required but no fold frontier was available".to_string(),
        )
        .into_response());
    };
    let Some(context_manager) = ctx.runtime.context_manager().await else {
        return Err(ApiError::Conflict(
            "context checkpoint confirmation requires a context manager".to_string(),
        )
        .into_response());
    };

    let workspace_dir = ctx.runtime.workspace_dir();
    let store = WorkspaceSessionStore::new(&workspace_dir);
    let chat_address = rua_core::ChatAddress::agent(&ctx.agent_id, chat_id);
    early_background_compaction_coordinator()
        .preempt_for_foreground(&early_background_compaction_key(&workspace_dir, chat_id));
    let summary_source = rua_agent::summary_checkpoint::build_summary_projection_slice(
        &store,
        &chat_address,
        running,
        frontier.end_sequence,
    )
    .await
    .map_err(|error| {
        ApiError::Conflict(format!(
            "context checkpoint projection preparation failed: {error}"
        ))
        .into_response()
    })?;
    let publisher = rua_agent::summary_checkpoint::SummaryCheckpointPublisher::new(
        rua_agent::summary_checkpoint::SummaryCheckpointPublisherInput {
            store,
            chat_id: chat_id.to_string(),
            chat_address: chat_address.clone(),
            agent_id: ctx.agent_id.clone(),
            context_manager: Some(context_manager),
            running_config: Arc::new(running.clone()),
            model: target_model.clone(),
            workspace_dir,
            replication_hook: ctx.runtime.session_replication_hook(),
            model_call_identity: Some(
                rua_agent::model_call_context::ModelCallIdentityContext::from_chat_address(
                    &chat_address,
                ),
            ),
        },
    );
    match publisher.publish(&frontier, summary_source, || true).await {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(ApiError::Conflict(
            "context checkpoint confirmation made no projection progress".to_string(),
        )
        .into_response()),
        Err(error) => Err(ApiError::Conflict(format!(
            "context checkpoint confirmation failed: {error}"
        ))
        .into_response()),
    }
}

fn early_background_compaction_key(workspace_dir: &std::path::Path, chat_id: &str) -> String {
    format!("{}::{chat_id}", workspace_dir.display())
}

fn fold_frontier_dto(frontier: ProjectionFoldFrontier) -> ModelSwitchFoldFrontierDto {
    ModelSwitchFoldFrontierDto {
        start_turn_ordinal: frontier.start_turn_ordinal,
        end_turn_ordinal: frontier.end_turn_ordinal,
        start_sequence: frontier.start_sequence,
        end_sequence: frontier.end_sequence,
        estimated_tokens_to_fold: frontier.estimated_tokens_to_fold,
    }
}

async fn list_providers(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
) -> Response {
    match list_effective_provider_configs(&state).await {
        Ok(configs) => {
            let providers: Vec<ProviderDto> = configs.iter().map(ProviderDto::from).collect();
            (StatusCode::OK, Json(providers)).into_response()
        }
        Err(e) => e.into_response(),
    }
}

async fn get_default_model(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Query(query): Query<DefaultModelQuery>,
) -> Response {
    if let Some(ref scope) = query.scope {
        if scope == "agent" && query.agent_id.is_none() {
            return ApiError::BadRequest("agent_id is required when scope=agent".into())
                .into_response();
        }
    }

    let active = match state.provider_manager.default_model().await {
        Some(a) => DefaultModelDto {
            provider_id: a.provider_id,
            model: a.model,
            extra: a.extra,
        },
        None => DefaultModelDto {
            provider_id: String::new(),
            model: String::new(),
            extra: serde_json::Map::new(),
        },
    };

    let default_llm = serde_json::to_value(&active).unwrap_or_default();
    let mut resp = serde_json::Map::new();
    resp.insert(
        "provider_id".into(),
        serde_json::Value::String(active.provider_id),
    );
    resp.insert("model".into(), serde_json::Value::String(active.model));
    for (k, v) in active.extra {
        resp.insert(k, v);
    }
    resp.insert("default_llm".into(), default_llm);
    (StatusCode::OK, Json(serde_json::Value::Object(resp))).into_response()
}

async fn set_default_model(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Json(body): Json<SetDefaultModelRequest>,
) -> Response {
    if body.provider_id.is_empty() || body.model.is_empty() {
        return ApiError::BadRequest("provider_id and model are required".into()).into_response();
    }
    if let Err(e) = ensure_zero_config_free_provider(&state, &body.provider_id).await {
        return e.into_response();
    }
    if let Some(response) = model_switch_preflight_response(&state, &ctx, &body).await {
        return response;
    }

    let active = LoaderDefaultModel {
        provider_id: body.provider_id.clone(),
        model: body.model.clone(),
        source: None,
        extra: body.extra.clone(),
    };

    match state.provider_manager.set_default_model(active).await {
        Ok(()) => {
            let dto = DefaultModelDto {
                provider_id: body.provider_id,
                model: body.model,
                extra: body.extra,
            };
            (StatusCode::OK, Json(dto)).into_response()
        }
        Err(e) => default_model_error_response(e),
    }
}

async fn clear_default_model(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
) -> Response {
    match state.provider_manager.clear_default_model().await {
        Ok(()) => {
            let resp = serde_json::json!({ "success": true, "message": "active model cleared" });
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn get_provider(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Path(id): Path<String>,
) -> Response {
    match load_provider_config(&state, &id).await {
        Ok(config) => (StatusCode::OK, Json(ProviderDto::from(&config))).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn update_provider(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Path(id): Path<String>,
    Json(body): Json<ProviderUpdateDto>,
) -> Response {
    apply_provider_update(&state, &id, body).await
}

async fn apply_provider_update(state: &AppState, id: &str, update: ProviderUpdateDto) -> Response {
    let mut config = match load_provider_config(state, id).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    if let Some(name) = update.name {
        config.name = name;
    }
    if let Some(base_url) = update.base_url {
        if !config.freeze_url || config.template_id() == "dashscope" {
            config.base_url = base_url;
        }
    }
    if let Some(api_key) = update.api_key {
        if !is_redacted_key(&api_key) {
            config.api_key = api_key;
        }
    }
    if let Some(api_key_prefixes) = update.api_key_prefixes {
        config.api_key_prefixes = api_key_prefixes;
    }
    if let Some(chat_model) = update.chat_model {
        if config.template_id() == "dashscope" {
            config.thinking_param_style = match chat_model.as_str() {
                "DashScopeResponseModel" => Some("effort".to_string()),
                "DashScopeChatModel" => Some("budget".to_string()),
                _ => config.thinking_param_style,
            };
        }
        config.chat_model = Some(chat_model);
    }
    if let Some(custom_headers) = update.custom_headers {
        config.custom_headers = custom_headers;
    }
    let existing_model_ids: HashSet<String> = config
        .models
        .iter()
        .chain(config.extra_models.iter())
        .map(|model| model.id.clone())
        .collect();
    if let Some(models) = update.models {
        config.models = models
            .into_iter()
            .map(dto_model_to_loader)
            .map(|mut model| {
                if !existing_model_ids.contains(&model.id) {
                    rua_provider::manager::apply_model_catalog_defaults_for_new_model(&mut model);
                }
                model
            })
            .collect();
    }
    if let Some(generate_kwargs) = update.generate_kwargs {
        let serde_json::Value::Object(generate_kwargs) = generate_kwargs else {
            return ApiError::BadRequest("generate_kwargs must be an object".into())
                .into_response();
        };
        config.generate_kwargs = generate_kwargs;
    }
    if let Some(provider_group) = update.provider_group {
        config.provider_group = provider_group;
    }
    if let Some(provider_group_name) = update.provider_group_name {
        config.provider_group_name = provider_group_name;
    }
    if let Some(provider_variant) = update.provider_variant {
        config.provider_variant = provider_variant;
    }
    if let Some(base_url_options) = update.base_url_options {
        config.base_url_options = base_url_options;
    }
    if update.thinking_param_style.is_some() {
        config.thinking_param_style = update.thinking_param_style;
    }
    if let Some(reasoning_effort_options) = update.reasoning_effort_options {
        config.reasoning_effort_options = reasoning_effort_options;
    }
    if update.thinking_budget_range.is_some() {
        config.thinking_budget_range = update.thinking_budget_range;
    }
    if let Some(extra_models_val) = update.extra.get("extra_models") {
        if let Ok(parsed) = serde_json::from_value::<Vec<ModelDto>>(extra_models_val.clone()) {
            config.extra_models = parsed
                .into_iter()
                .map(dto_model_to_loader)
                .map(|mut model| {
                    if !existing_model_ids.contains(&model.id) {
                        rua_provider::manager::apply_model_catalog_defaults_for_new_model(
                            &mut model,
                        );
                    }
                    model
                })
                .collect();
        }
    }
    for (k, v) in update.extra {
        if k == "extra_models" {
            continue;
        }
        config.extra.insert(k, v);
    }

    if let Err(e) = state.provider_cells.cell(id).write(config.clone()).await {
        return ApiError::from(e).into_response();
    }

    publish_provider_config_committed(state, id.to_string(), &config).await;

    (StatusCode::OK, Json(ProviderDto::from(&config))).into_response()
}

async fn delete_provider(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Path(id): Path<String>,
) -> Response {
    match state.provider_cells.cell(&id).read().await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return ApiError::NotFound(format!("Provider '{id}' not found")).into_response();
        }
        Err(e) => return ApiError::from(e).into_response(),
    }

    if state.provider_manager.get(&id).is_none() {
        tracing::debug!(
            provider_id = %id,
            "deleting provider config that is not registered in runtime manager"
        );
    }

    if let Err(e) = state.provider_cells.cell(&id).delete().await {
        return ApiError::from(e).into_response();
    }

    publish_provider_config_deleted(&state, id.clone()).await;

    let resp = DeleteProviderResponse {
        success: true,
        message: format!("Provider '{id}' deleted"),
    };
    (StatusCode::OK, Json(resp)).into_response()
}

async fn probe_provider(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Json(body): Json<ProbeRequestDto>,
) -> Response {
    let provider_id = body.provider_id.as_deref().unwrap_or("");
    if provider_id.is_empty() {
        return ApiError::BadRequest("provider_id is required".into()).into_response();
    }

    let start = std::time::Instant::now();
    let result = match state.provider_manager.check_connection(provider_id).await {
        Ok(ConnectionStatus::Connected) => {
            let models = state
                .provider_manager
                .discover_models(provider_id)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|model| model_dto_from_discovered_model(provider_id, model))
                .collect();
            ProbeResultDto {
                success: true,
                latency_ms: Some(start.elapsed().as_millis() as u64),
                models,
                error: None,
                extra: serde_json::Map::new(),
            }
        }
        Ok(ConnectionStatus::Failed(msg)) => ProbeResultDto {
            success: false,
            latency_ms: Some(start.elapsed().as_millis() as u64),
            models: vec![],
            error: Some(msg),
            extra: serde_json::Map::new(),
        },
        Err(e) => ProbeResultDto {
            success: false,
            latency_ms: Some(start.elapsed().as_millis() as u64),
            models: vec![],
            error: Some(e.to_string()),
            extra: serde_json::Map::new(),
        },
    };
    (StatusCode::OK, Json(result)).into_response()
}

async fn update_provider_config(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Path(id): Path<String>,
    Json(body): Json<ProviderUpdateDto>,
) -> Response {
    apply_provider_update(&state, &id, body).await
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct TestProviderBody {
    api_key: Option<String>,
    base_url: Option<String>,
    chat_model: Option<String>,
}

async fn test_provider(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Path(id): Path<String>,
    body: Option<Json<TestProviderBody>>,
) -> Response {
    if state.provider_manager.get(&id).is_none() {
        return ApiError::NotFound(format!("Provider '{id}' not found")).into_response();
    }

    let body = body.map(|Json(b)| b).unwrap_or_default();
    let has_override =
        body.api_key.is_some() || body.base_url.is_some() || body.chat_model.is_some();

    let result = if has_override {
        let mut cfg = match load_provider_config(&state, &id).await {
            Ok(c) => c,
            Err(e) => return e.into_response(),
        };
        if let Some(k) = body.api_key {
            if !is_redacted_key(&k) {
                cfg.api_key = k;
            }
        }
        if let Some(u) = body.base_url {
            cfg.base_url = u;
        }
        if let Some(m) = body.chat_model {
            cfg.chat_model = Some(m);
        }
        match rua_provider::manager::create_provider_from_config(&cfg) {
            Ok(prov) => prov.check_connection().await,
            Err(e) => return ApiError::from(e).into_response(),
        }
    } else {
        state.provider_manager.check_connection(&id).await
    };

    let test = match result {
        Ok(ConnectionStatus::Connected) => TestResult {
            success: true,
            message: Some("Connection successful".into()),
        },
        Ok(ConnectionStatus::Failed(msg)) => TestResult {
            success: false,
            message: Some(msg),
        },
        Err(e) => TestResult {
            success: false,
            message: Some(e.to_string()),
        },
    };
    (StatusCode::OK, Json(test)).into_response()
}

async fn discover_models(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Path(id): Path<String>,
) -> Response {
    match state.provider_manager.discover_models(&id).await {
        Ok(models) => {
            let result = DiscoverResult {
                models: models
                    .into_iter()
                    .map(|model| model_dto_from_discovered_model(&id, model))
                    .collect(),
            };
            (StatusCode::OK, Json(result)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn add_custom_provider(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Json(body): Json<CustomProviderCreateDto>,
) -> Response {
    if body.id.is_empty() {
        return ApiError::BadRequest("id is required".into()).into_response();
    }
    if state.provider_manager.get(&body.id).is_some() {
        return ApiError::Conflict(format!("provider '{}' already exists", body.id))
            .into_response();
    }

    let mut chat_model = body
        .extra
        .get("chat_model")
        .and_then(|v| v.as_str())
        .map(String::from);

    let models = body
        .models
        .into_iter()
        .map(dto_model_to_loader)
        .map(|mut model| {
            rua_provider::manager::apply_model_catalog_defaults_for_new_model(&mut model);
            model
        })
        .collect();

    let template_id = body
        .extra
        .get("template_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(&body.id);
    if body.extra.contains_key("template_id") && state_template(template_id).is_none() {
        return ApiError::BadRequest(format!("unknown provider template '{template_id}'"))
            .into_response();
    }
    let template = state_template(template_id).filter(|template| {
        chat_model
            .as_deref()
            .is_none_or(|protocol| protocol == template.chat_model)
    });
    let template_config = template.map(|template| template.to_config());
    if chat_model.is_none() {
        chat_model = template.map(|template| template.chat_model.to_string());
    }
    let support_model_discovery = template
        .is_some_and(|template| template.support_model_discovery())
        || (template.is_none()
            && matches!(
                chat_model.as_deref(),
                None | Some(
                    "OpenAIChatModel"
                        | "OpenAIResponseModel"
                        | "AnthropicChatModel"
                        | "GeminiChatModel"
                        | "OllamaChatModel"
                        | "DashScopeChatModel"
                )
            ));
    let mut config = ProviderConfig {
        id: body.id.clone(),
        name: body.name,
        base_url: body.base_url,
        api_key: body.api_key,
        api_keys: Vec::new(),
        api_key_prefixes: body.api_key_prefixes,
        headers: serde_json::Map::new(),
        custom_headers: body.custom_headers,
        chat_model,
        models,
        extra_models: vec![],
        timeout: None,
        generate_kwargs: template_config
            .as_ref()
            .map(|config| config.generate_kwargs.clone())
            .unwrap_or_default(),
        support_model_discovery,
        support_connection_check: template.is_none_or(|template| template.support_connection_check),
        is_local: template.is_some_and(|template| template.is_local),
        api_key_prefix: template
            .map(|template| template.api_key_prefix.to_string())
            .unwrap_or_default(),
        is_custom: template.is_none_or(|template| template.id != body.id),
        is_plugin: false,
        freeze_url: template.is_some_and(|template| template.freeze_url),
        require_api_key: template.is_none_or(|template| template.require_api_key),
        provider_group: body.provider_group,
        provider_group_name: body.provider_group_name,
        provider_variant: body.provider_variant,
        base_url_options: body.base_url_options,
        thinking_param_style: body.thinking_param_style,
        reasoning_effort_options: body.reasoning_effort_options,
        thinking_budget_range: body.thinking_budget_range,
        extra: body.extra,
    };
    if let Some(defaults) = template_config {
        if config.name.is_empty() {
            config.name = defaults.name;
        }
        if config.base_url.is_empty() {
            config.base_url = defaults.base_url;
        }
        if config.models.is_empty() {
            config.models = defaults.models;
        }
        if config.api_key_prefixes.is_empty() {
            config.api_key_prefixes = defaults.api_key_prefixes;
        }
        if config.provider_group.is_empty() {
            config.provider_group = defaults.provider_group;
        }
        if config.provider_group_name.is_empty() {
            config.provider_group_name = defaults.provider_group_name;
        }
        if config.provider_variant.is_empty() {
            config.provider_variant = defaults.provider_variant;
        }
        if config.base_url_options.is_empty() {
            config.base_url_options = defaults.base_url_options;
        }
        if config.thinking_param_style.is_none() {
            config.thinking_param_style = defaults.thinking_param_style;
        }
        if config.reasoning_effort_options.is_empty() {
            config.reasoning_effort_options = defaults.reasoning_effort_options;
        }
        if config.thinking_budget_range.is_none() {
            config.thinking_budget_range = defaults.thinking_budget_range;
        }
        for (key, value) in defaults.extra {
            config.extra.entry(key).or_insert(value);
        }
    }
    if let Some(serde_json::Value::Object(kwargs)) = config.extra.remove("generate_kwargs") {
        rua_provider::manager::deep_merge_maps(&mut config.generate_kwargs, &kwargs);
    }
    if let Some(serde_json::Value::String(prefix)) = config.extra.remove("api_key_prefix") {
        config.api_key_prefix = prefix;
    }
    // Typed fields are serialized once; the create DTO accepts protocol in extras.
    config.extra.remove("chat_model");
    if let Some(template) = template {
        config
            .extra
            .insert("template_id".into(), serde_json::json!(template.id));
    } else {
        config.extra.remove("template_id");
    }
    config.apply_model_defaults();

    if let Err(e) = state
        .provider_cells
        .cell(&config.id)
        .write(config.clone())
        .await
    {
        return ApiError::from(e).into_response();
    }

    publish_provider_config_committed(&state, config.id.clone(), &config).await;

    (StatusCode::CREATED, Json(ProviderDto::from(&config))).into_response()
}

async fn add_model(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Path(id): Path<String>,
    Json(model): Json<ModelDto>,
) -> Response {
    if model.id.is_empty() {
        return ApiError::BadRequest("model id is required".into()).into_response();
    }

    let existing = match load_provider_config(&state, &id).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    if existing.models.iter().any(|m| m.id == model.id)
        || existing.extra_models.iter().any(|m| m.id == model.id)
    {
        return ApiError::Conflict(format!("Model '{}' already exists", model.id)).into_response();
    }

    let mut new_entry = dto_model_to_loader(model);
    rua_provider::manager::apply_model_catalog_defaults_for_new_model(&mut new_entry);
    let config = match state
        .provider_cells
        .cell(&id)
        .update(move |maybe_existing| {
            let mut cfg = maybe_existing.unwrap_or(existing);
            cfg.models.push(new_entry);
            cfg
        })
        .await
    {
        Ok(cfg) => cfg,
        Err(e) => return ApiError::from(e).into_response(),
    };

    publish_provider_config_committed(&state, id.clone(), &config).await;

    (StatusCode::CREATED, Json(ProviderDto::from(&config))).into_response()
}

async fn delete_model(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Path((id, model_id)): Path<(String, String)>,
) -> Response {
    delete_model_inner(state, id, model_id).await
}

async fn delete_model_by_body(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Path(id): Path<String>,
    Json(body): Json<ModelIdRequest>,
) -> Response {
    delete_model_inner(state, id, body.model_id).await
}

async fn delete_model_inner(state: AppState, id: String, model_id: String) -> Response {
    let existing = match load_provider_config(&state, &id).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    let present = existing.models.iter().any(|m| m.id == model_id)
        || existing.extra_models.iter().any(|m| m.id == model_id);
    if !present {
        return ApiError::NotFound(format!("Model '{model_id}' not found in provider '{id}'"))
            .into_response();
    }

    let model_id_for_mutator = model_id.clone();
    let config = match state
        .provider_cells
        .cell(&id)
        .update(move |maybe_existing| {
            let mut cfg = maybe_existing.unwrap_or(existing);
            cfg.models.retain(|m| m.id != model_id_for_mutator);
            cfg.extra_models.retain(|m| m.id != model_id_for_mutator);
            cfg
        })
        .await
    {
        Ok(cfg) => cfg,
        Err(e) => return ApiError::from(e).into_response(),
    };

    publish_provider_config_committed(&state, id.clone(), &config).await;

    (StatusCode::OK, Json(ProviderDto::from(&config))).into_response()
}

async fn update_model_config(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Path((id, model_id)): Path<(String, String)>,
    Json(body): Json<ModelConfigUpdate>,
) -> Response {
    update_model_config_inner(state, id, model_id, body).await
}

async fn update_model_config_by_body(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Path(id): Path<String>,
    Json(body): Json<ModelConfigUpdate>,
) -> Response {
    let model_id = body.model_id.clone();
    if model_id.is_empty() {
        return ApiError::BadRequest("model id is required".into()).into_response();
    }
    update_model_config_inner(state, id, model_id, body).await
}

async fn update_model_config_inner(
    state: AppState,
    id: String,
    model_id: String,
    body: ModelConfigUpdate,
) -> Response {
    let existing = match load_provider_config(&state, &id).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    let present = existing.models.iter().any(|m| m.id == model_id)
        || existing.extra_models.iter().any(|m| m.id == model_id);
    if !present {
        return ApiError::NotFound(format!("Model '{model_id}' not found in provider '{id}'"))
            .into_response();
    }

    let model_id_for_mutator = model_id.clone();
    let body_extra = body.extra.clone();
    let body_name = body.name.clone();
    let body_context_window = body.context_window;
    let body_thinking_enabled = body.thinking_enabled;
    let body_thinking_budget = body.thinking_budget;
    let body_reasoning_effort = body.reasoning_effort.clone();
    let body_thinking_param_style = body.thinking_param_style.clone();
    let body_reasoning_effort_options = body.reasoning_effort_options.clone();
    let body_thinking_budget_range = body.thinking_budget_range;
    let body_relay_reasoning = body.relay_reasoning;
    let config = match state
        .provider_cells
        .cell(&id)
        .update(move |maybe_existing| {
            let mut cfg = maybe_existing.unwrap_or(existing);
            for m in cfg.models.iter_mut().chain(cfg.extra_models.iter_mut()) {
                if m.id == model_id_for_mutator {
                    if let Some(ref name) = body_name {
                        m.name = name.clone();
                    }
                    if let Some(context_window) = body_context_window {
                        m.context_window = context_window;
                    }
                    if body.thinking_enabled.is_some() {
                        m.thinking_enabled = body_thinking_enabled;
                    }
                    if body.thinking_budget.is_some() {
                        m.thinking_budget = body_thinking_budget;
                    }
                    if body.reasoning_effort.is_some() {
                        m.reasoning_effort = body_reasoning_effort.clone();
                    }
                    if body.thinking_param_style.is_some() {
                        m.thinking_param_style = body_thinking_param_style.clone();
                    }
                    if let Some(options) = &body_reasoning_effort_options {
                        m.reasoning_effort_options = options.clone();
                    }
                    if body.thinking_budget_range.is_some() {
                        m.thinking_budget_range = body_thinking_budget_range;
                    }
                    if body.relay_reasoning.is_some() {
                        m.relay_reasoning = body_relay_reasoning;
                    }
                    for (k, v) in &body_extra {
                        m.extra.insert(k.clone(), v.clone());
                    }
                    break;
                }
            }
            cfg
        })
        .await
    {
        Ok(cfg) => cfg,
        Err(e) => return ApiError::from(e).into_response(),
    };

    publish_provider_config_committed(&state, id.clone(), &config).await;

    (StatusCode::OK, Json(ProviderDto::from(&config))).into_response()
}

async fn list_templates(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
) -> Response {
    let templates: Vec<ProviderTemplateInfoDto> = state
        .provider_manager
        .list_templates()
        .iter()
        .map(ProviderTemplateInfoDto::from)
        .collect();
    (StatusCode::OK, Json(templates)).into_response()
}

async fn test_model(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Path(provider_id): Path<String>,
    Json(body): Json<TestModelRequest>,
) -> Response {
    let test = match state
        .provider_manager
        .check_model_connection(&provider_id, &body.model_id)
        .await
    {
        Ok((true, _msg)) => {
            if let Ok(existing) = load_provider_config(&state, &provider_id).await {
                let mid = body.model_id.clone();
                let needs_infer = existing
                    .models
                    .iter()
                    .chain(existing.extra_models.iter())
                    .any(|m| m.id == mid && m.context_window.is_none());
                if needs_infer {
                    if let Some(ctx) = rua_provider::infer_context_window(&mid) {
                        let mid2 = mid.clone();
                        let updated = state
                            .provider_cells
                            .cell(&provider_id)
                            .update(move |maybe| {
                                let mut cfg = maybe.unwrap_or(existing);
                                for m in cfg.models.iter_mut().chain(cfg.extra_models.iter_mut()) {
                                    if m.id == mid2 {
                                        m.context_window = Some(ctx);
                                        break;
                                    }
                                }
                                cfg
                            })
                            .await;
                        if let Ok(config) = updated {
                            publish_provider_config_committed(&state, provider_id.clone(), &config)
                                .await;
                        }
                    }
                }
            }
            TestResult {
                success: true,
                message: Some("Connection successful".into()),
            }
        }
        Ok((false, msg)) => TestResult {
            success: false,
            message: Some(msg),
        },
        Err(e) => TestResult {
            success: false,
            message: Some(e.to_string()),
        },
    };
    (StatusCode::OK, Json(test)).into_response()
}

async fn probe_multimodal(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Path((provider_id, model_id)): Path<(String, String)>,
) -> Response {
    probe_multimodal_inner(state, provider_id, model_id).await
}

async fn probe_multimodal_by_body(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Path(provider_id): Path<String>,
    Json(body): Json<TestModelRequest>,
) -> Response {
    probe_multimodal_inner(state, provider_id, body.model_id).await
}

async fn probe_multimodal_inner(
    state: AppState,
    provider_id: String,
    model_id: String,
) -> Response {
    let result = match state
        .provider_manager
        .probe_model_multimodal(&provider_id, &model_id)
        .await
    {
        Ok(r) => r,
        Err(e) => return ApiError::from(e).into_response(),
    };

    if let Some(error) = &result.error {
        return ApiError::Unavailable(format!("Multimodal probe failed: {error}")).into_response();
    }

    if let Ok(existing) = load_provider_config(&state, &provider_id).await {
        let mid = model_id.clone();
        let si = result.supports_image;
        let sv = result.supports_video;
        let sm = result.supports_multimodal;
        let updated = state
            .provider_cells
            .cell(&provider_id)
            .update(move |maybe| {
                let mut cfg = maybe.unwrap_or(existing);
                for m in cfg.models.iter_mut().chain(cfg.extra_models.iter_mut()) {
                    if m.id == mid {
                        m.supports_image = Some(si);
                        m.supports_video = Some(sv);
                        m.supports_multimodal = Some(sm);
                        m.probe_source = Some("probed".into());
                        break;
                    }
                }
                cfg
            })
            .await;
        if let Ok(config) = updated {
            publish_provider_config_committed(&state, provider_id.clone(), &config).await;
        }
    }

    let resp = ProbeMultimodalResponse {
        supports_multimodal: Some(result.supports_multimodal),
        supports_image: Some(result.supports_image),
        supports_video: Some(result.supports_video),
        message: Some(result.image_message),
    };
    (StatusCode::OK, Json(resp)).into_response()
}

async fn openrouter_series(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
) -> Response {
    let client = match state.provider_manager.openrouter().await {
        Ok(c) => c,
        Err(e) => return ApiError::Internal(e.to_string()).into_response(),
    };
    match client.get_available_providers().await {
        Ok(series) => (
            StatusCode::OK,
            Json(serde_json::json!({ "series": series })),
        )
            .into_response(),
        Err(e) => ApiError::Internal(e.to_string()).into_response(),
    }
}

async fn openrouter_discover_extended(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
) -> Response {
    let client = match state.provider_manager.openrouter().await {
        Ok(c) => c,
        Err(e) => return ApiError::Internal(e.to_string()).into_response(),
    };
    match client.fetch_extended_models().await {
        Ok(models) => (
            StatusCode::OK,
            Json(serde_json::json!({ "models": models })),
        )
            .into_response(),
        Err(e) => ApiError::Internal(e.to_string()).into_response(),
    }
}

async fn openrouter_models_filter(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Json(body): Json<OpenRouterFilterRequest>,
) -> Response {
    let client = match state.provider_manager.openrouter().await {
        Ok(c) => c,
        Err(e) => return ApiError::Internal(e.to_string()).into_response(),
    };
    let models = match client.fetch_extended_models().await {
        Ok(m) => m,
        Err(e) => return ApiError::Internal(e.to_string()).into_response(),
    };
    let criteria = rua_provider::openrouter_extended::FilterCriteria {
        providers: body.providers,
        input_modalities: body.input_modalities,
        output_modalities: body.output_modalities,
        max_prompt_price: body.max_prompt_price,
    };
    let filtered =
        rua_provider::openrouter::OpenRouterExtendedClient::filter_models(&models, &criteria);
    (
        StatusCode::OK,
        Json(serde_json::json!({ "models": filtered })),
    )
        .into_response()
}

async fn list_providers_for_agent(
    State(state): State<AppState>,
    Path(_agent_id): Path<String>,
) -> Response {
    match list_effective_provider_configs(&state).await {
        Ok(configs) => {
            let providers: Vec<ProviderDto> = configs.iter().map(ProviderDto::from).collect();
            (StatusCode::OK, Json(providers)).into_response()
        }
        Err(e) => e.into_response(),
    }
}

async fn get_default_model_for_agent(
    State(state): State<AppState>,
    Path(_agent_id): Path<String>,
) -> Response {
    let active = state
        .provider_manager
        .default_model()
        .await
        .map(|a| DefaultModelDto {
            provider_id: a.provider_id,
            model: a.model,
            extra: a.extra,
        });
    let resp = DefaultModelResponseDto {
        default_llm: active,
    };
    (StatusCode::OK, Json(resp)).into_response()
}

async fn set_default_model_for_agent(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(_agent_id): Path<String>,
    Json(body): Json<SetDefaultModelRequest>,
) -> Response {
    if body.provider_id.is_empty() || body.model.is_empty() {
        return ApiError::BadRequest("provider_id and model are required".into()).into_response();
    }
    if let Err(e) = ensure_zero_config_free_provider(&state, &body.provider_id).await {
        return e.into_response();
    }
    if let Some(response) = model_switch_preflight_response(&state, &ctx, &body).await {
        return response;
    }

    let active = LoaderDefaultModel {
        provider_id: body.provider_id.clone(),
        model: body.model.clone(),
        source: None,
        extra: body.extra.clone(),
    };

    match state.provider_manager.set_default_model(active).await {
        Ok(()) => {
            let dto = DefaultModelDto {
                provider_id: body.provider_id,
                model: body.model,
                extra: body.extra,
            };
            (StatusCode::OK, Json(dto)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}
