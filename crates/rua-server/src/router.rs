use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::Json;
use axum::Router;
use std::path::PathBuf;

use crate::auth;
use crate::endpoints;
use crate::errors::{api_method_not_allowed, api_not_found, json_error_middleware};
use crate::middleware;
use crate::state::AppState;
use crate::static_assets;

pub fn build_app(state: AppState, static_dir: Option<PathBuf>) -> Router {
    build_app_with_desktop_host(state, static_dir, None)
}

pub fn build_app_with_desktop_host(
    state: AppState,
    static_dir: Option<PathBuf>,
    desktop_host: Option<crate::desktop_host::DesktopHostConfig>,
) -> Router {
    let desktop_security_enabled = desktop_host.is_some();

    let agent_scoped_api = api_routes(&state).route_layer(axum::middleware::from_fn_with_state(
        state.clone(),
        middleware::agent_context_middleware,
    ));
    let admin_api = endpoints::workspace_admin::register_workspace_admin_routes()
        .merge(endpoints::health::router())
        .merge(endpoints::commands_discovery::router());
    let api_router = agent_scoped_api
        .merge(admin_api)
        .layer(axum::Extension(
            endpoints::desktop_control::DesktopControlService::from_environment(),
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::auth_middleware,
        ))
        .layer(axum::middleware::from_fn(json_error_middleware))
        .layer(axum::middleware::map_response(
            |mut response: Response| async move {
                response.headers_mut().insert(
                    axum::http::header::CACHE_CONTROL,
                    axum::http::HeaderValue::from_static("no-cache, no-store"),
                );
                response
            },
        ))
        .with_state(state.clone());

    let static_dir_clone = static_dir.clone();
    let static_dir_for_fallback = static_dir.clone();

    let mut app = Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/ws", get(crate::websocket::ws_handler))
        .route("/voice/incoming", post(endpoints::voice::voice_incoming))
        .route("/voice/ws", get(endpoints::voice::voice_ws))
        .route(
            "/voice/status-callback",
            post(endpoints::voice::voice_status_callback),
        )
        .route(
            "/voice/asr/transcribe",
            post(endpoints::voice::voice_asr_transcribe),
        )
        .route(
            "/voice/tts/synthesize",
            post(endpoints::voice::voice_tts_synthesize),
        )
        .route("/voice/tts/config", get(endpoints::voice::voice_tts_config))
        .route("/voice/asr/config", get(endpoints::voice::voice_asr_config))
        .route("/voice/{*path}", any(voice_fallback))
        .nest("/api", api_router.clone());

    if state.profile == crate::profiles::ServerProfile::MobileApk {
        app = app.route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::auth_middleware,
        ));
    }
    // Upgrade authenticates in its bounded first frame, since browsers cannot
    // attach bearer headers to WebSocket handshakes. Logical operations retain
    // the canonical API middleware and handlers.
    app = app.merge(crate::console_transport::router(
        state.clone(),
        Router::new()
            .nest("/api", api_router)
            .with_state(state.clone()),
    ));
    app = app.merge(crate::desktop_host::router(state.clone(), desktop_host));

    if let Some(ref dir) = static_dir {
        for asset in static_assets::ROOT_ASSETS {
            let dir = dir.clone();
            let asset_name = asset.to_string();
            app = app.route(
                &format!("/{asset}"),
                get(move || async move {
                    match static_assets::serve_static_file(&dir, &asset_name) {
                        Some(response) => response,
                        None => StatusCode::NOT_FOUND.into_response(),
                    }
                }),
            );
        }
    }

    if let Some(ref dir) = static_dir_clone {
        let assets_dir = dir.join("assets");
        if assets_dir.is_dir() {
            let service = tower::ServiceBuilder::new()
                .layer(
                    tower_http::set_header::SetResponseHeaderLayer::if_not_present(
                        axum::http::header::CACHE_CONTROL,
                        axum::http::HeaderValue::from_static(
                            static_assets::HASHED_ASSET_CACHE_CONTROL,
                        ),
                    ),
                )
                .service(tower_http::services::ServeDir::new(assets_dir));
            app = app.nest_service("/assets", service);
        }
    }

    let spa_dir = static_dir_for_fallback;
    app = app.fallback(move || async move { spa_fallback(spa_dir.as_deref()) });

    app = app.layer(
        tower_http::compression::CompressionLayer::new()
            .quality(tower_http::CompressionLevel::Default),
    );

    app = app.layer(tower_http::limit::RequestBodyLimitLayer::new(
        middleware::DEFAULT_BODY_LIMIT,
    ));
    app = app.layer(axum::extract::DefaultBodyLimit::disable());

    if desktop_security_enabled {
        app = app.layer(axum::middleware::from_fn(
            crate::desktop_host::security_middleware,
        ));
    } else {
        app = app.layer(middleware::cors_layer());
    }

    app = app.layer(axum::middleware::from_fn(
        middleware::panic_catcher_middleware,
    ));

    app = app.layer(axum::middleware::from_fn(
        middleware::request_logging_middleware,
    ));

    app = app.layer(axum::middleware::from_fn(middleware::request_id_middleware));

    app.with_state(state)
}

async fn healthz() -> StatusCode {
    StatusCode::OK
}

async fn readyz(State(state): State<AppState>) -> Response {
    let ready = state.is_ready();
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let body = serde_json::json!({
        "ready": ready,
        "version": env!("CARGO_PKG_VERSION"),
    });
    (status, Json(body)).into_response()
}

fn spa_fallback(static_dir: Option<&std::path::Path>) -> Response {
    match static_dir {
        Some(dir) => static_assets::serve_index(dir),
        None => static_assets::placeholder_index(),
    }
}

fn api_routes(state: &AppState) -> Router<AppState> {
    let _ = state;
    Router::new()
        .nest("/auth", endpoints::auth::router())
        .merge(endpoints::agents::router())
        .merge(endpoints::projects::router())
        .merge(endpoints::chat_media::router())
        .merge(endpoints::console::agent_scoped_router())
        .merge(endpoints::models::router())
        .merge(endpoints::user_inbox::router())
        .merge(endpoints::config_channels::root_router())
        .merge(endpoints::config_channels::agent_scoped_router())
        .merge(endpoints::access_control::root_router())
        .merge(endpoints::access_control::agent_scoped_router())
        .merge(endpoints::config_misc::router())
        .merge(endpoints::security::router())
        .merge(endpoints::network_secret_backup::router())
        .merge(endpoints::pairing::router())
        .merge(endpoints::audit::router())
        .merge(endpoints::config::agent_scoped_config_router())
        .merge(endpoints::config::file_preview_router())
        .merge(endpoints::tools::agent_scoped_router())
        .merge(endpoints::mcp::root_router())
        .merge(endpoints::mcp::agent_scoped_router())
        .merge(endpoints::mcp_oauth::root_router())
        .merge(endpoints::mcp_oauth::agent_scoped_router())
        .merge(endpoints::provider_oauth::router())
        .merge(endpoints::acp::root_router())
        .merge(endpoints::acp::agent_scoped_router())
        .merge(endpoints::acp_chat::router())
        .merge(endpoints::mcp_status::router())
        .merge(endpoints::shared_mcp::router())
        .merge(endpoints::terminal::router())
        .merge(endpoints::tasks::router())
        .merge(endpoints::skills::shared_router())
        .merge(endpoints::skills::agent_scoped_router())
        .merge(endpoints::skills_hub::register_hub_routes())
        .merge(endpoints::market::router())
        .merge(endpoints::skill_update::router())
        .merge(endpoints::skills_scanner::register_scanner_routes())
        .merge(endpoints::files::root_router())
        .merge(endpoints::files::agent_scoped_router())
        .merge(endpoints::workspace::router())
        .merge(endpoints::envs::router())
        .merge(endpoints::prompt_library::router())
        .merge(endpoints::token_usage::router())
        .merge(endpoints::cron::root_router())
        .merge(endpoints::cron::agent_scoped_router())
        .merge(endpoints::local_models::router())
        .merge(endpoints::local_models_download_stream::router())
        .merge(endpoints::plugins::router())
        .merge(endpoints::settings::router())
        .merge(endpoints::messages::router())
        .merge(endpoints::storage::router())
        .route("/doctor/version", get(doctor_version))
        .route("/doctor/runtime", get(doctor_runtime))
        .merge(endpoints::doctor_fix::router())
        .merge(endpoints::chats::root_router())
        .merge(endpoints::chats::agent_scoped_router())
        .merge(endpoints::product_hooks::router())
        .merge(endpoints::chat_prompt::root_router())
        .merge(endpoints::chat_prompt::agent_scoped_router())
        .merge(endpoints::chat_search::root_router())
        .merge(endpoints::chat_search::agent_scoped_router())
        .route("/config", get(get_global_config).put(put_global_config))
        .route(
            "/agents/{agentId}/config",
            get(get_agent_config).put(put_agent_config),
        )
        .merge(endpoints::agent_templates::router())
        .merge(endpoints::subagent_templates::router())
        .merge(endpoints::approvals::router())
        .merge(endpoints::acp_runtime::router())
        .merge(endpoints::approval_stream::router())
        .route("/security/request-approval", post(request_approval))
        .route("/security/pending-approvals", get(list_pending_approvals))
        .route(
            "/security/pending-approvals/{id}/approve",
            post(approve_approval),
        )
        .route("/security/pending-approvals/{id}/deny", post(deny_approval))
        .route("/security/approvals/{id}/approve", post(approve_approval))
        .route("/security/approvals/{id}/deny", post(deny_approval))
        .route("/security/approvals/{id}/reject", post(deny_approval))
        .merge(endpoints::config_channels::dingtalk_webhook_router())
        .merge(endpoints::config_channels::dingtalk_webhook_compat_router())
        .merge(endpoints::config_channels::dingtalk_webhook_channels_router())
        .merge(crate::endpoints::messages::channel_webhook_router())
        .route("/voice/asr", post(endpoints::voice::voice_asr_transcribe))
        .merge(endpoints::memory::root_router())
        .merge(endpoints::memory::agent_scoped_router())
        .merge(endpoints::tunnel_status::router())
        .merge(endpoints::sync_devices::router())
        .merge(endpoints::sync_transfer::router())
        .merge(endpoints::sync::router())
        .merge(endpoints::platform::router())
        .merge(endpoints::gap_fixes::agent_config_router())
        .merge(endpoints::gap_fixes::agent_stats_router())
        .merge(endpoints::gap_fixes::console_debug_router())
        .route("/agent/stream_query", post(deprecated_agent_app))
        .route("/agent/query", post(deprecated_agent_app))
        .fallback(api_fallback)
        .method_not_allowed_fallback(method_not_allowed_handler)
}

async fn api_fallback() -> Response {
    api_not_found()
}

async fn voice_fallback() -> Response {
    api_not_found()
}

async fn method_not_allowed_handler() -> Response {
    api_method_not_allowed()
}

async fn doctor_version() -> Response {
    let report = rua_doctor::run_diagnostics(&rua_config::dirs::Paths::new(None));
    Json(serde_json::json!(report.version())).into_response()
}

async fn doctor_runtime() -> Response {
    let report = rua_doctor::run_diagnostics(&rua_config::dirs::Paths::new(None));
    Json(serde_json::json!(report.runtime())).into_response()
}

async fn get_global_config(State(state): State<AppState>) -> Response {
    Json(serde_json::json!(&*state.config)).into_response()
}

async fn put_global_config(State(state): State<AppState>) -> Response {
    Json(serde_json::json!(&*state.config)).into_response()
}

async fn get_agent_config(
    State(state): State<AppState>,
    Path(_agent_id): Path<String>,
) -> Response {
    Json(serde_json::json!(&*state.config)).into_response()
}

async fn put_agent_config(
    State(state): State<AppState>,
    Path(_agent_id): Path<String>,
) -> Response {
    Json(serde_json::json!(&*state.config)).into_response()
}

#[derive(serde::Deserialize)]
struct CompatCreateApproval {
    tool_name: String,
    #[serde(default)]
    agent_id: Option<String>,
}

#[derive(serde::Deserialize)]
struct CompatDecision {
    #[serde(default)]
    decided_by: Option<String>,
    #[serde(default)]
    agent_id: Option<String>,
}

async fn prepare_compat_approval_resume(state: &AppState, agent_id: &str, approval_id: &str) {
    let authority = state.security.approval_authority();
    let Some(request) = authority.request(approval_id).await else {
        return;
    };
    if request.agent_id != agent_id {
        return;
    }
    let Some(address) = request.owner.as_chat_address() else {
        return;
    };
    let _ = state
        .conversation_authority
        .transition_active_turn_phase(
            address,
            Some(&request.turn_id),
            crate::services::conversation_authority::ConversationActiveTurnPhase::RunningTool,
        )
        .await;
}

async fn request_approval(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<crate::middleware::AgentExtractorContext>,
    Json(req): Json<CompatCreateApproval>,
) -> Response {
    use rua_security::{ApprovalRequestInput, ApprovalTrigger, ToolCallSnapshot};

    let agent_id = req
        .agent_id
        .filter(|s| !s.is_empty())
        .unwrap_or(ctx.agent_id);
    let record = state
        .security
        .approval_authority()
        .create_request(ApprovalRequestInput {
            owner: rua_security::ApprovalOwner::chat(rua_core::ChatAddress::agent(
                &agent_id, "compat",
            )),
            agent_id,
            turn_id: "compat".into(),
            tool_call_id: String::new(),
            tool_name: req.tool_name.clone(),
            trigger: ApprovalTrigger::approval_mode(format!(
                "Tool '{}' requires approval",
                req.tool_name
            )),
            tool_call: ToolCallSnapshot {
                display: req.tool_name,
                args: serde_json::json!({}),
            },
        })
        .await;
    Json(record).into_response()
}

async fn list_pending_approvals(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<crate::middleware::AgentExtractorContext>,
) -> Response {
    let records = state
        .security
        .approval_authority()
        .list_pending_for_agent(&ctx.agent_id)
        .await;
    Json(records).into_response()
}

async fn approve_approval(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<crate::middleware::AgentExtractorContext>,
    Path(id): Path<String>,
    body: Option<Json<CompatDecision>>,
) -> Response {
    let (decided_by, agent_id) = match body {
        Some(Json(b)) => (
            b.decided_by.unwrap_or_else(|| "compat".into()),
            b.agent_id.filter(|s| !s.is_empty()).unwrap_or(ctx.agent_id),
        ),
        None => ("compat".into(), ctx.agent_id),
    };
    prepare_compat_approval_resume(&state, &agent_id, &id).await;
    match state
        .security
        .approval_authority()
        .decide_for_agent(
            &agent_id,
            &id,
            rua_security::ApprovalDecision::RunOnce { decided_by },
        )
        .await
    {
        Ok(record) => Json(record).into_response(),
        Err(e) => {
            let body = serde_json::json!({"detail": e.to_string()});
            (StatusCode::NOT_FOUND, Json(body)).into_response()
        }
    }
}

async fn deny_approval(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<crate::middleware::AgentExtractorContext>,
    Path(id): Path<String>,
    body: Option<Json<CompatDecision>>,
) -> Response {
    let (decided_by, agent_id) = match body {
        Some(Json(b)) => (
            b.decided_by.unwrap_or_else(|| "compat".into()),
            b.agent_id.filter(|s| !s.is_empty()).unwrap_or(ctx.agent_id),
        ),
        None => ("compat".into(), ctx.agent_id),
    };
    prepare_compat_approval_resume(&state, &agent_id, &id).await;
    match state
        .security
        .approval_authority()
        .decide_for_agent(
            &agent_id,
            &id,
            rua_security::ApprovalDecision::Reject { decided_by },
        )
        .await
    {
        Ok(record) => Json(record).into_response(),
        Err(e) => {
            let body = serde_json::json!({"detail": e.to_string()});
            (StatusCode::NOT_FOUND, Json(body)).into_response()
        }
    }
}

async fn deprecated_agent_app() -> Response {
    api_not_found()
}
