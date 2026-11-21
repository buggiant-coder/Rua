use sha2::{Digest, Sha256};
use std::path::PathBuf;
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
    resolve_chat_media, ChatMediaError, ChatMediaService, ChatMediaUpload, MediaId, MediaReference,
    StoredChatMedia, MAX_CHAT_MEDIA_BYTES,
};
use crate::services::conversation_authority_placement::{
    ConversationAuthorityEpoch, ConversationAuthorityPlacement,
};
use crate::services::device_command_bus::{
    deliver_chat_media_prepare_via_session, ChatMediaPrepareCommand,
};
use crate::services::device_context::current_sync_device_id;
use crate::services::sync_transfer::{
    send_transfer_stream, ReceiveConflictPolicy, TransferExecutionOptions, TransferManifest,
    TransferManifestEntry, TransferManifestEntryKind, TransferStreamSendRequest,
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
    project_id: Option<String>,
    expected_epoch: Option<ConversationAuthorityEpoch>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/chats/{chatId}/media", post(upload_chat_media))
        .route("/chats/{chatId}/media/{mediaId}", get(read_chat_media))
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
                upload = Some(ChatMediaUpload {
                    bytes,
                    mime_type,
                    original_filename: filename,
                });
            }
            Some("agent_id") => match field.text().await {
                Ok(agent_id) => context.agent_id = Some(agent_id),
                Err(error) => return ApiError::BadRequest(error.to_string()).into_response(),
            },
            Some("project_id") => match field.text().await {
                Ok(project_id) => context.project_id = Some(project_id),
                Err(error) => return ApiError::BadRequest(error.to_string()).into_response(),
            },
            Some("expected_authority_epoch") => match field.text().await {
                Ok(epoch) => match ConversationAuthorityEpoch::parse(&epoch) {
                    Ok(epoch) => context.expected_epoch = Some(epoch),
                    Err(error) => return ApiError::BadRequest(error.to_string()).into_response(),
                },
                Err(error) => return ApiError::BadRequest(error.to_string()).into_response(),
            },
            Some("model") => match field.text().await {
                Ok(model) => match serde_json::from_str::<serde_json::Value>(&model) {
                    Ok(_) => {}
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
    let Some(upload) = upload else {
        return ApiError::BadRequest("file field is required".into()).into_response();
    };
    if upload.bytes.is_empty() {
        return media_error(ChatMediaError::Empty).into_response();
    }
    let Some(local_device_id) = current_sync_device_id(&state.sync_device) else {
        return ApiError::Unavailable("Local Device identity unavailable".into()).into_response();
    };
    let scope = rua_core::ChatStore::resolve(&state.rua_root, address.clone());
    let gate = state.conversation_authority.transition_gate_at(&address);
    let transition = gate.lock().await;
    match rua_session::chat_deletion::ChatDeleteIntentStore::from_rua_root(&state.rua_root)
        .load(&address)
    {
        Ok(Some(_)) => return ApiError::NotFound("Chat was deleted".into()).into_response(),
        Err(detail) => return ApiError::Internal(detail).into_response(),
        Ok(None) => {}
    }
    let placement = match crate::services::conversation_authority_target::load_optional_chat_authority_placement(scope.root(), address.chat_id().as_str()).await {
        Ok(Some(p)) => Some(p),
        Ok(None) => match crate::services::remote_conversation_input::read_source_pending_at(&state.rua_root,&address).await {
            Ok(pending) => pending.map(|p| ConversationAuthorityPlacement {device_id:p.target_device_id, authority_epoch:p.authority_epoch, base_manifest_hash:p.base_manifest_hash.unwrap_or_default()}),
            Err(error) => return ApiError::Internal(error).into_response(),
        },
        Err(error) => return ApiError::Internal(error).into_response(),
    };
    if context
        .expected_epoch
        .is_some_and(|epoch| placement.as_ref().map(|p| p.authority_epoch) != Some(epoch))
    {
        return ApiError::Conflict("Chat Media upload Authority Epoch changed".into())
            .into_response();
    }
    if placement.is_none() {
        drop(transition);
        let draft = match crate::services::chat_context::resolve_submission_context(
            &state,
            address.chat_id().as_str(),
            context.agent_id.as_deref(),
            context.project_id.as_deref().map(Some),
        )
        .await
        {
            Ok(context) => crate::services::chat_media::StagedChatMedia {
                address,
                project_id: context.project_id,
            },
            Err(error) => return error.into_response(),
        };
        return match ChatMediaService
            .store_staged(
                &state.conversation_authority,
                &state.rua_root,
                draft,
                &local_device_id,
                upload,
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

    let accepted_authority_epoch = placement.authority_epoch;
    let result = if placement.device_id == local_device_id {
        ChatMediaService
            .store(&state.rua_root, address, &local_device_id, upload)
            .await
            .map_err(media_error)
    } else {
        drop(transition);
        upload_remote_media(&state, address, placement, local_device_id, upload).await
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
        Err(error) => error.into_response(),
    }
}

async fn read_media(state: AppState, address: ChatAddress, media_id: String) -> Response {
    let Some(device_id) = current_sync_device_id(&state.sync_device) else {
        return ApiError::Unavailable("Local Device identity unavailable".into()).into_response();
    };
    let manager = state
        .sync_transfer_session_manager
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let opened = match resolve_chat_media(
        &state.rua_root,
        &address,
        &media_id,
        &device_id,
        manager.as_ref(),
    )
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
    upload: ChatMediaUpload,
) -> Result<StoredChatMedia, ApiError> {
    let ChatMediaUpload {
        bytes,
        mime_type,
        original_filename,
    } = upload;
    let original_filename =
        crate::services::chat_media::normalize_original_filename(original_filename);
    let media_id = MediaId::generate();
    let operation_id = format!("chat-media-{}", uuid::Uuid::new_v4());
    let mime_type = crate::services::chat_media::normalize_mime_type(mime_type);
    let session_manager = state
        .sync_transfer_session_manager
        .read()
        .unwrap_or_else(|lock| lock.into_inner())
        .clone()
        .ok_or_else(|| ApiError::Unavailable("Device Session manager unavailable".into()))?;
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let command = ChatMediaPrepareCommand {
        sha256,
        original_filename: original_filename.clone(),
        command_id: uuid::Uuid::new_v4().to_string(),
        operation_id: operation_id.clone(),
        address: address.clone(),
        source_device_id: local_device_id.clone(),
        target_device_id: placement.device_id.clone(),
        authority_epoch: placement.authority_epoch,
        media_id: media_id.to_string(),
        byte_size: bytes.len() as u64,
        mime_type: mime_type.clone(),
    };
    let prepared =
        deliver_chat_media_prepare_via_session(&session_manager, &command, Duration::from_secs(10))
            .await
            .map_err(|error| match error {
                crate::services::device_command_bus::DeviceCommandDeliveryError::Rejected {
                    reason,
                    detail,
                } if reason == "conversation_authority_redirect" => ApiError::Conflict(detail),
                other => ApiError::Unavailable(other.to_string()),
            })?;
    if prepared.authority_epoch != command.authority_epoch
        || prepared.media_id != command.media_id
        || prepared.operation_id != command.operation_id
    {
        return Err(ApiError::Unavailable(
            "Invalid Chat Media preparation receipt".into(),
        ));
    }
    if !prepared.already_present {
        let temp_dir = tempfile::tempdir().map_err(|e| ApiError::Internal(e.to_string()))?;
        let source_path = temp_dir.path().join(media_id.to_string());
        tokio::fs::write(&source_path, &bytes)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        let manifest = TransferManifest {
            entries: vec![TransferManifestEntry {
                source_path: tokio::fs::canonicalize(&source_path)
                    .await
                    .map_err(|error| ApiError::Internal(error.to_string()))?,
                relative_path: PathBuf::from(media_id.to_string()),
                kind: TransferManifestEntryKind::File,
                size: bytes.len() as u64,
                sha256: format!("sha256:{}", command.sha256),
                extra: Default::default(),
            }],
            total_bytes: bytes.len() as u64,
            extra: Default::default(),
        };
        let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(RunControl::Running);
        send_transfer_stream(
            &session_manager,
            TransferStreamSendRequest {
                chat_context: None,
                operation_id,
                source_device_id: local_device_id,
                target_device_id: placement.device_id.clone(),
                manifest,
                target_dir: PathBuf::from("."),
                conflict_policy: ReceiveConflictPolicy::Fail,
                options: TransferExecutionOptions::default(),
            },
            cancel_rx,
            |_| {},
        )
        .await
        .map_err(|error| match error {
            crate::services::sync_transfer::SyncTransferPlanError::Remote(detail)
                if detail.starts_with("Conversation Authority") =>
            {
                ApiError::Conflict(detail)
            }
            other => ApiError::Unavailable(other.to_string()),
        })?;
    }
    // The authenticated recipient has verified the complete object against this command.
    command
        .manifest()
        .register(&rua_chat::ChatService::new(&state.rua_root).media_root())
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(StoredChatMedia {
        media_ref: MediaReference::from_id(media_id),
        media_id: media_id.to_string(),
        mime_type,
        byte_size: bytes.len() as u64,
        original_filename,
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
        ChatMediaError::Conflict(detail) => ApiError::Conflict(detail),
        ChatMediaError::Unavailable(detail) => ApiError::Unavailable(detail),
        ChatMediaError::NotFound => ApiError::NotFound(error.to_string()),
        ChatMediaError::Storage(_) => ApiError::Internal(error.to_string()),
    }
}

async fn upload_chat_media(
    State(state): State<AppState>,
    Path(chat_id): Path<String>,
    multipart: Multipart,
) -> Response {
    if let Err(error) = crate::services::console_chat::resolve_chat_id(&chat_id) {
        return ApiError::BadRequest(error).into_response();
    }
    upload_media(state, ChatAddress::new(chat_id), multipart).await
}

async fn read_chat_media(
    State(state): State<AppState>,
    Path((chat_id, media_id)): Path<(String, String)>,
) -> Response {
    if let Err(error) = crate::services::console_chat::resolve_chat_id(&chat_id) {
        return ApiError::BadRequest(error).into_response();
    }
    read_media(state, ChatAddress::new(chat_id), media_id).await
}
