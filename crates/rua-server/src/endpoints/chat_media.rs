use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Multipart, Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rua_core::{ChatAddress, RunControl};
use serde::Serialize;
use tokio_util::io::ReaderStream;

use crate::errors::ApiError;
use crate::services::chat_media::{
    ChatMediaError, ChatMediaService, MediaId, MediaReference, StoredChatMedia,
    MAX_CHAT_MEDIA_BYTES,
};
use crate::services::conversation_authority_placement::{
    ConversationAuthorityEpoch, ConversationAuthorityPlacement,
};
use crate::services::device_command_bus::{
    deliver_chat_media_prepare_via_session, ChatMediaPrepareCommand,
};
use crate::services::device_context::current_sync_device_id;
use crate::services::sync_transfer::{
    open_chat_media_read_stream, send_transfer_stream, ReceiveConflictPolicy,
    TransferExecutionOptions, TransferManifest, TransferManifestEntry, TransferManifestEntryKind,
    TransferStreamSendRequest,
};
use crate::state::AppState;

#[derive(Debug, Serialize)]
struct ChatMediaUploadResponse {
    #[serde(flatten)]
    media: StoredChatMedia,
    #[serde(skip_serializing_if = "Option::is_none")]
    authority_epoch: Option<ConversationAuthorityEpoch>,
}

#[derive(Debug, Default)]
struct ChatMediaUploadContext {
    agent_id: Option<String>,
    model: Option<serde_json::Value>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/agents/{agentId}/chats/{chatId}/media",
            post(upload_agent_media),
        )
        .route(
            "/agents/{agentId}/chats/{chatId}/media/{mediaId}",
            get(read_agent_media),
        )
        .route(
            "/projects/{projectId}/chats/{chatId}/media",
            post(upload_project_media),
        )
        .route(
            "/projects/{projectId}/chats/{chatId}/media/{mediaId}",
            get(read_project_media),
        )
}

async fn upload_agent_media(
    State(state): State<AppState>,
    Path((agent_id, chat_id)): Path<(String, String)>,
    multipart: Multipart,
) -> Response {
    upload_media(state, ChatAddress::agent(agent_id, chat_id), multipart).await
}

async fn upload_project_media(
    State(state): State<AppState>,
    Path((project_id, chat_id)): Path<(String, String)>,
    multipart: Multipart,
) -> Response {
    upload_media(state, ChatAddress::project(project_id, chat_id), multipart).await
}

async fn upload_media(state: AppState, address: ChatAddress, mut multipart: Multipart) -> Response {
    let mut upload = None;
    let mut context = ChatMediaUploadContext::default();
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(error) => return ApiError::BadRequest(error.to_string()).into_response(),
        };
        match field.name() {
            Some("file") => {
                if upload.is_some() {
                    return ApiError::BadRequest("exactly one file field is required".to_string())
                        .into_response();
                }
                let filename = field.file_name().map(str::to_string);
                let mime_type = field.content_type().map(str::to_string);
                let bytes = match field.bytes().await {
                    Ok(bytes) if bytes.len() <= MAX_CHAT_MEDIA_BYTES => bytes.to_vec(),
                    Ok(_) => {
                        return ApiError::TooLarge("media payload is too large".to_string())
                            .into_response()
                    }
                    Err(error) => return ApiError::BadRequest(error.to_string()).into_response(),
                };
                upload = Some((bytes, mime_type, filename));
            }
            Some("agent_id") => match field.text().await {
                Ok(agent_id) => context.agent_id = Some(agent_id),
                Err(error) => return ApiError::BadRequest(error.to_string()).into_response(),
            },
            Some("model") => match field.text().await {
                Ok(model) => match serde_json::from_str(&model) {
                    Ok(model) => context.model = Some(model),
                    Err(error) => {
                        return ApiError::BadRequest(format!("invalid Chat Media model: {error}"))
                            .into_response()
                    }
                },
                Err(error) => return ApiError::BadRequest(error.to_string()).into_response(),
            },
            _ => {}
        }
    }
    let Some((bytes, mime_type, filename)) = upload else {
        return ApiError::BadRequest("file field is required".to_string()).into_response();
    };
    let scope = rua_core::ChatScopeStore::resolve(&state.rua_root, address.clone());
    let placement = match crate::services::conversation_authority_target::load_optional_chat_authority_placement(scope.root(), address.chat_id().as_str()).await {
        Ok(placement) => placement,
        Err(error) => return ApiError::Internal(error).into_response(),
    };
    if placement.is_none() {
        if let Err(error) = validate_media_staging_scope(&state, &address).await {
            return error.into_response();
        }
        return match ChatMediaService
            .store_staged(
                &state.conversation_authority,
                &state.rua_root,
                address,
                bytes,
                mime_type,
                filename,
            )
            .await
        {
            Ok(media) => (
                StatusCode::CREATED,
                Json(ChatMediaUploadResponse {
                    media,
                    authority_epoch: None,
                }),
            )
                .into_response(),
            Err(error) => media_error(error).into_response(),
        };
    }
    let placement = placement.expect("existing media placement was checked");

    let local_device_id = current_sync_device_id(&state.sync_device)
        .expect("placement materialization requires local Device identity");
    let accepted_authority_epoch = placement.authority_epoch;
    let result = if placement.device_id == local_device_id {
        ChatMediaService
            .store(&state.rua_root, address, bytes, mime_type, filename)
            .await
    } else {
        upload_remote_media(
            &state,
            address,
            placement,
            local_device_id,
            bytes,
            mime_type,
            filename,
        )
        .await
    };
    match result {
        Ok(stored) => (
            StatusCode::CREATED,
            Json(ChatMediaUploadResponse {
                media: stored,
                authority_epoch: Some(accepted_authority_epoch),
            }),
        )
            .into_response(),
        Err(error) => media_error(error).into_response(),
    }
}

async fn validate_media_staging_scope(
    state: &AppState,
    address: &ChatAddress,
) -> Result<(), ApiError> {
    crate::services::console_chat::resolve_chat_id(address.chat_id().as_str())
        .map_err(ApiError::BadRequest)?;
    match address.scope() {
        rua_core::ChatScope::Agent { agent_id, .. } => {
            state
                .agent_service
                .get_agent(agent_id.as_str())
                .await
                .map_err(|error| ApiError::BadRequest(error.to_string()))?;
        }
        rua_core::ChatScope::Project { project_id, .. } => {
            let project = state
                .project_catalog
                .get(project_id)
                .map_err(|error| ApiError::Internal(error.to_string()))?
                .ok_or_else(|| ApiError::NotFound(format!("Project not found: {project_id}")))?;
            if project.lifecycle != rua_project::ProjectLifecycle::Active {
                return Err(ApiError::Conflict(
                    "Archived Project cannot accept media uploads".into(),
                ));
            }
        }
    }
    Ok(())
}

async fn read_agent_media(
    State(state): State<AppState>,
    Path((agent_id, chat_id, media_id)): Path<(String, String, String)>,
) -> Response {
    read_media(state, ChatAddress::agent(agent_id, chat_id), media_id).await
}

async fn read_project_media(
    State(state): State<AppState>,
    Path((project_id, chat_id, media_id)): Path<(String, String, String)>,
) -> Response {
    read_media(state, ChatAddress::project(project_id, chat_id), media_id).await
}

async fn read_media(state: AppState, address: ChatAddress, media_id: String) -> Response {
    if MediaId::from_str(&media_id).is_err() {
        return media_error(ChatMediaError::InvalidId).into_response();
    }
    let scope = rua_core::chat_store::ChatScopeStore::resolve(&state.rua_root, address.clone());
    let placement =
        match crate::services::conversation_authority_target::load_optional_chat_authority_placement(
            scope.root(),
            address.chat_id().as_str(),
        )
        .await
    {
            Ok(placement) => placement,
            Err(error) => return ApiError::Internal(error).into_response(),
        };
    let local_device_id = current_sync_device_id(&state.sync_device)
        .expect("placement materialization requires local Device identity");
    if placement.is_none() {
        if let Err(error) = validate_media_staging_scope(&state, &address).await {
            return error.into_response();
        }
    }
    if let Some(placement) = placement.filter(|placement| placement.device_id != local_device_id) {
        let session_manager = match state
            .sync_transfer_session_manager
            .read()
            .unwrap_or_else(|lock| lock.into_inner())
            .clone()
        {
            Some(manager) => manager,
            None => {
                return ApiError::Unavailable("Device Session manager is unavailable".to_string())
                    .into_response()
            }
        };
        return match open_chat_media_read_stream(
            &session_manager,
            &placement.device_id,
            address,
            placement.authority_epoch,
            media_id,
        )
        .await
        {
            Ok((mime_type, byte_size, reader)) => Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime_type)
                .header(header::CONTENT_LENGTH, byte_size)
                .body(Body::from_stream(ReaderStream::new(reader)))
                .expect("valid remote Chat Media response"),
            Err(error) => ApiError::Unavailable(error).into_response(),
        };
    }
    let opened = match ChatMediaService
        .open(&state.rua_root, address, &media_id)
        .await
    {
        Ok(opened) => opened,
        Err(error) => return media_error(error).into_response(),
    };
    let file = match tokio::fs::File::open(&opened.path).await {
        Ok(file) => file,
        Err(error) => return ApiError::Internal(error.to_string()).into_response(),
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, opened.mime_type)
        .header(header::CONTENT_LENGTH, opened.byte_size)
        .body(Body::from_stream(ReaderStream::new(file)))
        .expect("valid Chat Media response")
}

async fn upload_remote_media(
    state: &AppState,
    address: ChatAddress,
    placement: ConversationAuthorityPlacement,
    local_device_id: String,
    bytes: Vec<u8>,
    mime_type: Option<String>,
    original_filename: Option<String>,
) -> Result<StoredChatMedia, ChatMediaError> {
    let media_id = MediaId::generate();
    let operation_id = format!("chat-media-{}", uuid::Uuid::new_v4());
    let mime_type = mime_type
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let session_manager = state
        .sync_transfer_session_manager
        .read()
        .unwrap_or_else(|lock| lock.into_inner())
        .clone()
        .ok_or_else(|| {
            ChatMediaError::Storage(std::io::Error::other(
                "Device Session manager is unavailable",
            ))
        })?;
    deliver_chat_media_prepare_via_session(
        &session_manager,
        &ChatMediaPrepareCommand {
            sha256: None,
            first_submission: false,
            command_id: uuid::Uuid::new_v4().to_string(),
            operation_id: operation_id.clone(),
            address,
            source_device_id: local_device_id.clone(),
            target_device_id: placement.device_id.clone(),
            authority_epoch: placement.authority_epoch,
            media_id: media_id.to_string(),
            byte_size: bytes.len() as u64,
            mime_type: mime_type.clone(),
        },
        Duration::from_secs(10),
    )
    .await
    .map_err(|error| ChatMediaError::Storage(std::io::Error::other(error.to_string())))?;

    let temp_dir = tempfile::tempdir().map_err(ChatMediaError::Storage)?;
    let source_path = temp_dir.path().join(media_id.to_string());
    tokio::fs::write(&source_path, &bytes)
        .await
        .map_err(ChatMediaError::Storage)?;
    let manifest = TransferManifest {
        entries: vec![TransferManifestEntry {
            source_path,
            relative_path: PathBuf::from(media_id.to_string()),
            kind: TransferManifestEntryKind::File,
            size: bytes.len() as u64,
        }],
        total_bytes: bytes.len() as u64,
    };
    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(RunControl::Running);
    send_transfer_stream(
        &session_manager,
        TransferStreamSendRequest {
            operation_id,
            source_device_id: local_device_id,
            target_device_id: placement.device_id,
            manifest,
            target_dir: PathBuf::from("."),
            conflict_policy: ReceiveConflictPolicy::Fail,
            options: TransferExecutionOptions::default(),
        },
        cancel_rx,
        |_| {},
    )
    .await
    .map_err(|error| ChatMediaError::Storage(std::io::Error::other(error.to_string())))?;

    Ok(StoredChatMedia {
        media_ref: MediaReference::from_id(media_id),
        media_id: media_id.to_string(),
        mime_type,
        byte_size: bytes.len() as u64,
        original_filename: original_filename.and_then(|value| {
            std::path::Path::new(&value)
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        }),
    })
}

fn media_error(error: ChatMediaError) -> ApiError {
    match error {
        ChatMediaError::Empty => ApiError::BadRequest(error.to_string()),
        ChatMediaError::TooLarge => ApiError::TooLarge(error.to_string()),
        ChatMediaError::InvalidId => ApiError::Validation(vec![crate::errors::ValidationError {
            loc: vec!["path".to_string(), "mediaId".to_string()],
            msg: error.to_string(),
            error_type: "value_error.uuid".to_string(),
        }]),
        ChatMediaError::NotFound => ApiError::NotFound(error.to_string()),
        ChatMediaError::Storage(_) => ApiError::Internal(error.to_string()),
    }
}
