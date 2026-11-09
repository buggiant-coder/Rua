use axum::extract::{Multipart, Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};

use crate::endpoints::dto;
use crate::errors::ApiError;
use crate::middleware::AgentExtractorContext;
use crate::services::prompt_library::prompt_library;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/prompt-library/files", get(list_files))
        .route(
            "/prompt-library/files/{md_name}",
            get(get_file).put(update_file).delete(delete_file),
        )
        .route("/prompt-library/files/{md_name}/rename", post(rename_file))
        .route("/prompt-library/subdirs-info", get(list_subdirs_info))
        .route(
            "/prompt-library/subdirs/{subdir}",
            get(list_subdir_files).post(create_subdir_file),
        )
        .route(
            "/prompt-library/subdirs/{subdir}/{md_name}",
            get(get_subdir_file)
                .put(write_subdir_file)
                .delete(delete_subdir_file),
        )
        .route(
            "/prompt-library/subdirs/{subdir}/{md_name}/rename",
            post(rename_subdir_file),
        )
        .route("/prompt-library/broadcast", post(broadcast))
        .route("/prompt-library/sync", post(sync))
        .route("/prompt-library/download", get(download))
        .route("/prompt-library/upload", post(upload))
        .route("/prompt-library/restore-builtin", post(restore_builtin))
}

async fn list_files(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
) -> Response {
    let lib = prompt_library(&state);
    match lib.list_files().await {
        Ok(files) => {
            let dto_files: Vec<dto::FileEntry> = files.into_iter().map(Into::into).collect();
            (StatusCode::OK, Json(dto_files)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn get_file(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Path(md_name): Path<String>,
) -> Response {
    let lib = prompt_library(&state);
    match lib.get_file(&md_name).await {
        Ok(Some(file)) => {
            let dto_file: dto::FileContent = file.into();
            (StatusCode::OK, Json(dto_file)).into_response()
        }
        Ok(None) => ApiError::NotFound(format!("Prompt '{md_name}' not found")).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn update_file(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Path(md_name): Path<String>,
    Json(body): Json<dto::FileWriteRequest>,
) -> Response {
    let lib = prompt_library(&state);
    match lib.update_file(&md_name, &body.content).await {
        Ok(file) => {
            let dto_file: dto::FileContent = file.into();
            (StatusCode::OK, Json(dto_file)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn delete_file(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Path(md_name): Path<String>,
) -> Response {
    let lib = prompt_library(&state);
    match lib.delete_file(&md_name).await {
        Ok(()) => (StatusCode::OK, Json(dto::SuccessResponse::ok())).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn rename_file(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Path(md_name): Path<String>,
    Json(body): Json<dto::FileRenameRequest>,
) -> Response {
    let lib = prompt_library(&state);
    match lib.rename_file(&md_name, &body.new_name).await {
        Ok(file) => {
            let dto_file: dto::FileContent = file.into();
            (StatusCode::OK, Json(dto_file)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn list_subdirs_info(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
) -> Response {
    let lib = prompt_library(&state);
    match lib.list_subdirs_info().await {
        Ok(subdirs) => (StatusCode::OK, Json(subdirs)).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn list_subdir_files(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Path(subdir): Path<String>,
) -> Response {
    let lib = prompt_library(&state);
    match lib.list_subdir_files(&subdir).await {
        Ok(files) => {
            let dto_files: Vec<dto::FileEntry> = files.into_iter().map(Into::into).collect();
            (StatusCode::OK, Json(dto_files)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn create_subdir_file(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Path(subdir): Path<String>,
    Json(body): Json<dto::SubdirFileCreateRequest>,
) -> Response {
    let lib = prompt_library(&state);
    match lib
        .create_subdir_file(&subdir, body.filename.as_deref(), &body.content)
        .await
    {
        Ok(file) => {
            let dto_file: dto::FileContent = file.into();
            (StatusCode::CREATED, Json(dto_file)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn get_subdir_file(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Path((subdir, md_name)): Path<(String, String)>,
) -> Response {
    let lib = prompt_library(&state);
    match lib.get_subdir_file(&subdir, &md_name).await {
        Ok(Some(file)) => {
            let dto_file: dto::FileContent = file.into();
            (StatusCode::OK, Json(dto_file)).into_response()
        }
        Ok(None) => {
            ApiError::NotFound(format!("File '{md_name}' not found in '{subdir}'")).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn write_subdir_file(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Path((subdir, md_name)): Path<(String, String)>,
    Json(body): Json<dto::FileWriteRequest>,
) -> Response {
    let lib = prompt_library(&state);
    match lib
        .write_subdir_file(&subdir, &md_name, &body.content)
        .await
    {
        Ok(file) => {
            let dto_file: dto::FileContent = file.into();
            (StatusCode::OK, Json(dto_file)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn rename_subdir_file(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Path((subdir, md_name)): Path<(String, String)>,
    Json(body): Json<dto::FileRenameRequest>,
) -> Response {
    let lib = prompt_library(&state);
    match lib
        .rename_subdir_file(&subdir, &md_name, &body.new_name)
        .await
    {
        Ok(file) => {
            let dto_file: dto::FileContent = file.into();
            (StatusCode::OK, Json(dto_file)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn delete_subdir_file(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Path((subdir, md_name)): Path<(String, String)>,
) -> Response {
    let lib = prompt_library(&state);
    match lib.delete_subdir_file(&subdir, &md_name).await {
        Ok(()) => (StatusCode::OK, Json(dto::SuccessResponse::ok())).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn broadcast(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Json(body): Json<dto::PromptBroadcastRequest>,
) -> Response {
    let lib = prompt_library(&state);
    let workspaces_root = state.rua_root.join("workspaces");
    let resolver = move |agent_id: &str| -> Option<std::path::PathBuf> {
        let dir = workspaces_root.join(agent_id);
        if dir.is_dir() {
            Some(dir)
        } else {
            std::fs::create_dir_all(&dir).ok().map(|_| dir)
        }
    };
    match lib.broadcast(body, resolver).await {
        Ok(result) => (StatusCode::OK, Json(result)).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn sync(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Json(body): Json<dto::PromptLibrarySyncRequest>,
) -> Response {
    let lib = prompt_library(&state);
    match lib.sync(body).await {
        Ok(result) => (StatusCode::OK, Json(result)).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn download(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
) -> Response {
    let lib = prompt_library(&state);
    match lib.download().await {
        Ok(data) => {
            let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
            let filename = format!("prompt_library_{timestamp}.zip");
            let headers = [
                (header::CONTENT_TYPE, "application/zip".to_string()),
                (
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{filename}\""),
                ),
            ];
            (StatusCode::OK, headers, data).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn upload(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    mut multipart: Multipart,
) -> Response {
    let data = match multipart.next_field().await {
        Ok(Some(field)) => match field.bytes().await {
            Ok(bytes) => bytes,
            Err(e) => {
                return ApiError::BadRequest(format!("Failed to read upload: {e}")).into_response();
            }
        },
        Ok(None) => {
            return ApiError::BadRequest("No file field in multipart body".to_string())
                .into_response();
        }
        Err(e) => {
            return ApiError::BadRequest(format!("Invalid multipart: {e}")).into_response();
        }
    };
    let lib = prompt_library(&state);
    match lib.upload(&data[..]).await {
        Ok(message) => (
            StatusCode::OK,
            Json(dto::SuccessResponse::with_message(message)),
        )
            .into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn restore_builtin(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
) -> Response {
    let lib = prompt_library(&state);
    match lib.restore_builtin().await {
        Ok(result) => (
            StatusCode::OK,
            Json(dto::SuccessResponse::with_message(result.message)),
        )
            .into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}
