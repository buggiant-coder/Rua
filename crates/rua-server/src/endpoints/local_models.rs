use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};

use rua_local_model::{
    manager::ModelSourceConfig, DownloadProgress, LocalModelInfo, RecommendedModel, ServerStatus,
    ServerUpdateInfo, StartServerRequest,
};

use crate::endpoints::dto;
use crate::errors::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/local-models/server",
            get(get_server_status)
                .post(start_server)
                .delete(stop_server),
        )
        .route("/local-models/server/update", get(check_server_update))
        .route(
            "/local-models/server/download",
            post(download_server)
                .get(download_server_progress)
                .delete(cancel_download_server),
        )
        .route("/local-models/models", get(list_models))
        .route("/local-models/recommended", get(get_recommended_models))
        .route(
            "/local-models/models/download",
            post(download_model)
                .get(download_model_progress)
                .delete(cancel_download_model),
        )
        .route(
            "/local-models/models/{*model_id}",
            delete(delete_local_model),
        )
}

fn map_server_status(s: ServerStatus, installed_binary: bool) -> dto::LocalModelServerStatus {
    let model_name = s.model.clone();
    let installed = s.running || installed_binary || command_exists("llama-server");
    let message = if !installed {
        Some("llama.cpp server is not installed".to_string())
    } else if s.running {
        None
    } else {
        Some("local model server is not running".to_string())
    };
    dto::LocalModelServerStatus {
        available: s.running,
        installable: true,
        installed,
        running: s.running,
        pid: s.pid,
        port: s.port,
        model: s.model,
        model_name,
        message,
        extra: Default::default(),
    }
}

fn command_exists(binary: &str) -> bool {
    let candidate = std::path::Path::new(binary);
    if binary.contains('/') || binary.contains('\\') {
        return candidate.is_file();
    }

    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        command_file_names(binary)
            .into_iter()
            .any(|name| dir.join(name).is_file())
    })
}

fn command_file_names(binary: &str) -> Vec<String> {
    #[cfg(windows)]
    {
        let mut names = vec![binary.to_string()];
        if std::path::Path::new(binary).extension().is_none() {
            let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
            for ext in pathext.split(';').filter(|ext| !ext.trim().is_empty()) {
                names.push(format!("{binary}{ext}"));
            }
        }
        names
    }
    #[cfg(not(windows))]
    {
        vec![binary.to_string()]
    }
}

fn map_download(p: DownloadProgress) -> dto::LocalModelDownloadProgress {
    dto::LocalModelDownloadProgress {
        model_id: p.model_id,
        model_name: p.model_name,
        status: p.status,
        progress: p.progress,
        downloaded_bytes: p.downloaded_bytes,
        total_bytes: p.total_bytes,
        speed_bytes_per_sec: p.speed_bytes_per_sec,
        source: p.source,
        error: p.error,
        local_path: p.local_path,
        extra: Default::default(),
    }
}

fn map_local_model(m: LocalModelInfo) -> dto::LocalModel {
    dto::LocalModel {
        id: m.id,
        name: m.name,
        path: m.path,
        size: m.size,
        format: m.format,
        quantization: m.quantization,
        extra: Default::default(),
    }
}

fn map_recommended_model(m: RecommendedModel) -> dto::RecommendedModelDto {
    dto::RecommendedModelDto {
        id: m.id,
        name: m.name,
        size_bytes: m.size_bytes,
        downloaded: m.downloaded,
        source: m.source,
        extra: Default::default(),
    }
}

fn map_server_update(u: ServerUpdateInfo) -> dto::LocalModelServerUpdate {
    dto::LocalModelServerUpdate {
        has_update: u.has_update,
        current_version: u.current_version,
        latest_version: u.latest_version,
        extra: Default::default(),
    }
}

async fn get_server_status(State(state): State<AppState>) -> Response {
    let status = state.local_model.server_status().await;
    let installed_binary = state.local_model.server_binary_installed();
    (
        StatusCode::OK,
        Json(map_server_status(status, installed_binary)),
    )
        .into_response()
}

pub async fn start_server(
    State(state): State<AppState>,
    Json(body): Json<dto::LocalModelServerRequest>,
) -> Response {
    let req = StartServerRequest {
        model_id: body.model_id,
        port: body.port,
    };
    match state.local_model.start_server(req).await {
        Ok(status) => {
            if let Some(port) = status.port {
                let pm = state.provider_manager.clone();
                let cells = state.provider_cells.clone();
                tokio::spawn(async move {
                    crate::local_model_resume::sync_local_provider_url(pm, cells, port).await;
                });
            }
            let installed_binary = state.local_model.server_binary_installed();
            (
                StatusCode::OK,
                Json(map_server_status(status, installed_binary)),
            )
                .into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

pub async fn stop_server(State(state): State<AppState>) -> Response {
    match state.local_model.stop_server().await {
        Ok(()) => (StatusCode::OK, Json(dto::SuccessResponse::ok())).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn check_server_update(State(state): State<AppState>) -> Response {
    let info = state.local_model.check_server_update().await;
    (StatusCode::OK, Json(map_server_update(info))).into_response()
}

pub async fn download_server(
    State(state): State<AppState>,
    Json(body): Json<dto::LocalModelDownloadRequest>,
) -> Response {
    let progress = state.local_model.download_server(body.model_id).await;
    (StatusCode::OK, Json(map_download(progress))).into_response()
}

async fn download_server_progress(State(state): State<AppState>) -> Response {
    match state.local_model.server_download_progress().await {
        Ok(progress) => (StatusCode::OK, Json(map_download(progress))).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn cancel_download_server(State(state): State<AppState>) -> Response {
    match state.local_model.cancel_server_download().await {
        Ok(()) => (StatusCode::OK, Json(dto::SuccessResponse::ok())).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn get_recommended_models(State(state): State<AppState>) -> Response {
    let ram_mb = rua_local_model::detect_ram_mb();
    let memory_gb = ram_mb / 1024;
    let models = state.local_model.get_recommended_models(memory_gb);
    let dto_models: Vec<dto::RecommendedModelDto> =
        models.into_iter().map(map_recommended_model).collect();
    (StatusCode::OK, Json(dto_models)).into_response()
}

pub async fn list_local_models_handler(State(state): State<AppState>) -> Response {
    let models = state.local_model.list_models().await;
    let dto_models: Vec<dto::LocalModel> = models.into_iter().map(map_local_model).collect();
    (StatusCode::OK, Json(dto_models)).into_response()
}

async fn list_models(state: State<AppState>) -> Response {
    list_local_models_handler(state).await
}

async fn delete_local_model(
    State(state): State<AppState>,
    Path(model_id): Path<String>,
) -> Response {
    match state.local_model.delete_model(&model_id).await {
        Ok(()) => (StatusCode::OK, Json(dto::SuccessResponse::ok())).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

pub async fn start_local_model_handler(
    state: State<AppState>,
    body: Json<dto::LocalModelServerRequest>,
) -> Response {
    start_server(state, body).await
}

pub async fn stop_local_model_handler(state: State<AppState>) -> Response {
    stop_server(state).await
}

pub async fn download_local_model_handler(
    State(state): State<AppState>,
    Json(body): Json<dto::LocalModelDownloadRequest>,
) -> Response {
    let progress = if let Some(source_config) = download_source_config(&body) {
        state
            .local_model
            .download_model_with_source(body.model_id, source_config)
            .await
    } else {
        state.local_model.download_model(body.model_id).await
    };
    (StatusCode::OK, Json(map_download(progress))).into_response()
}

fn download_source_config(body: &dto::LocalModelDownloadRequest) -> Option<ModelSourceConfig> {
    if let Some(url) = body.url.as_ref().filter(|url| !url.trim().is_empty()) {
        return Some(ModelSourceConfig::CustomUrl {
            url: url.trim().to_string(),
        });
    }

    let source = body
        .extra
        .get("source")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .unwrap_or("auto");

    match source {
        "huggingface" => Some(ModelSourceConfig::HuggingFace {
            repo_id: strip_source_prefix(&body.model_id, &["hf:", "huggingface:"]).to_string(),
            filename: None,
        }),
        "modelscope" => Some(ModelSourceConfig::ModelScope {
            repo_id: strip_source_prefix(&body.model_id, &["ms:", "modelscope:"]).to_string(),
            filename: None,
        }),
        _ => None,
    }
}

fn strip_source_prefix<'a>(model_id: &'a str, prefixes: &[&str]) -> &'a str {
    prefixes
        .iter()
        .find_map(|prefix| model_id.strip_prefix(prefix))
        .unwrap_or(model_id)
}

async fn download_model(
    state: State<AppState>,
    body: Json<dto::LocalModelDownloadRequest>,
) -> Response {
    download_local_model_handler(state, body).await
}

async fn download_model_progress(State(state): State<AppState>) -> Response {
    match state.local_model.model_download_progress().await {
        Ok(progress) => (StatusCode::OK, Json(map_download(progress))).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn cancel_download_model(State(state): State<AppState>) -> Response {
    match state.local_model.cancel_model_download().await {
        Ok(()) => (StatusCode::OK, Json(dto::SuccessResponse::ok())).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}
