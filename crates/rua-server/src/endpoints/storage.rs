use std::collections::{BTreeSet, HashMap, HashSet};

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use rua_core::ChatAddress;
use serde::{Deserialize, Serialize};

use crate::errors::ApiError;
use crate::services::chat_storage::{self, ScopeStorageSnapshot};
use crate::state::AppState;

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StorageSortField {
    #[default]
    UpdatedAt,
    Size,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StorageSortOrder {
    Asc,
    #[default]
    Desc,
}

#[derive(Debug, Default, Deserialize)]
struct ListStorageChatsQuery {
    #[serde(default)]
    sort_by: StorageSortField,
    #[serde(default)]
    sort_order: StorageSortOrder,
    #[serde(default)]
    before: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
struct DeleteStorageChatsBody {
    addresses: Vec<ChatAddress>,
    #[serde(default)]
    acknowledge_data_loss: bool,
}

#[derive(Debug, Deserialize)]
struct PreviewStorageChatsBody {
    addresses: Vec<ChatAddress>,
}

#[derive(Debug, Default, Serialize)]
struct PreviewStorageChatsResponse {
    chat_count: usize,
    attributed_bytes: u64,
    reclaimable_bytes: u64,
    shared_media_preserved: Vec<SharedMedia>,
    skipped: Vec<DeleteIssue>,
}

#[derive(Debug, Serialize)]
struct DeleteIssue {
    address: ChatAddress,
    reason: String,
}

#[derive(Debug, Serialize)]
struct SharedMedia {
    address: ChatAddress,
    media_ids: Vec<String>,
}

#[derive(Debug, Default, Serialize)]
struct DeleteStorageChatsResponse {
    deleted: Vec<ChatAddress>,
    skipped: Vec<DeleteIssue>,
    failed: Vec<DeleteIssue>,
    pending: Vec<DeleteIssue>,
    freed_bytes: u64,
    shared_media_preserved: Vec<SharedMedia>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/storage/chats", get(list_storage_chats))
        .route("/storage/chats/preview", post(preview_storage_chats))
        .route("/storage/chats/delete", post(delete_storage_chats))
}

async fn preview_storage_chats(
    State(state): State<AppState>,
    Json(body): Json<PreviewStorageChatsBody>,
) -> Response {
    let snapshot = match load_storage_snapshot(&state).await {
        Ok(snapshot) => snapshot,
        Err(error) => return ApiError::Internal(error).into_response(),
    };
    let mut response = PreviewStorageChatsResponse::default();
    let mut unique = HashSet::new();
    let mut addresses = Vec::new();
    for address in body.addresses {
        if !unique.insert(address.identity_key()) {
            continue;
        }
        let Some(entry) = snapshot
            .entries
            .iter()
            .find(|entry| entry.address == address)
        else {
            response.skipped.push(DeleteIssue {
                address,
                reason: "Chat is not present in the local Chat Index".to_string(),
            });
            continue;
        };
        response.chat_count += 1;
        response.attributed_bytes = response
            .attributed_bytes
            .saturating_add(entry.attributed_bytes);
        addresses.push(address);
    }
    let deleting = addresses
        .iter()
        .map(|address| address.chat_id().to_string())
        .collect::<BTreeSet<_>>();
    let (removable_media, preserved) = chat_storage::removable_media(&snapshot, &deleting);
    let workspace = state.chat_workspace();
    let scope_root = workspace.as_path();
    match chat_storage::planned_freed_bytes(scope_root, &snapshot, &deleting, &removable_media) {
        Ok(bytes) => response.reclaimable_bytes = response.reclaimable_bytes.saturating_add(bytes),
        Err(error) => {
            return ApiError::Internal(error).into_response();
        }
    }
    for address in addresses {
        if let Some(media_ids) = preserved.get(address.chat_id().as_str()) {
            response.shared_media_preserved.push(SharedMedia {
                address,
                media_ids: media_ids.clone(),
            });
        }
    }
    Json(response).into_response()
}

async fn list_storage_chats(
    State(state): State<AppState>,
    Query(query): Query<ListStorageChatsQuery>,
) -> Response {
    match load_storage_snapshot(&state).await {
        Ok(snapshot) => {
            let mut entries = snapshot
                .entries
                .into_iter()
                .filter(|entry| query.before.is_none_or(|before| entry.updated_at < before))
                .collect::<Vec<_>>();
            entries.sort_by(|left, right| {
                let ordering = match query.sort_by {
                    StorageSortField::UpdatedAt => left
                        .updated_at
                        .cmp(&right.updated_at)
                        .then_with(|| left.total_bytes.cmp(&right.total_bytes)),
                    StorageSortField::Size => left
                        .total_bytes
                        .cmp(&right.total_bytes)
                        .then_with(|| left.updated_at.cmp(&right.updated_at)),
                }
                .then_with(|| {
                    left.address
                        .identity_key()
                        .cmp(&right.address.identity_key())
                });
                match query.sort_order {
                    StorageSortOrder::Asc => ordering,
                    StorageSortOrder::Desc => ordering.reverse(),
                }
            });
            Json(entries).into_response()
        }
        Err(error) => ApiError::Internal(error).into_response(),
    }
}

async fn delete_storage_chats(
    State(state): State<AppState>,
    Json(body): Json<DeleteStorageChatsBody>,
) -> Response {
    if !body.acknowledge_data_loss {
        return ApiError::BadRequest("acknowledge_data_loss must be true".to_string())
            .into_response();
    }
    let snapshot = match load_storage_snapshot(&state).await {
        Ok(snapshot) => snapshot,
        Err(error) => return ApiError::Internal(error).into_response(),
    };
    let mut response = DeleteStorageChatsResponse::default();
    let mut unique = HashSet::new();
    let mut addresses = Vec::new();
    let mut project_chats = Vec::new();
    for address in body.addresses {
        if !unique.insert(address.identity_key()) {
            response.skipped.push(DeleteIssue {
                address,
                reason: "duplicate address".to_string(),
            });
            continue;
        }
        let Some(entry) = snapshot
            .entries
            .iter()
            .find(|entry| entry.address == address)
        else {
            response.skipped.push(DeleteIssue {
                address,
                reason: "Chat is not present in the local Chat Index".to_string(),
            });
            continue;
        };
        if entry.pinned {
            response.skipped.push(DeleteIssue {
                address,
                reason: "Pinned Chat cannot be deleted".to_string(),
            });
            continue;
        }
        if entry.owner.kind == "project" {
            project_chats.push((rua_core::ProjectId::new(&entry.owner.id), address));
        } else {
            addresses.push(address);
        }
    }

    let workspace = state.chat_workspace();
    let scope_root = workspace.as_path();
    let mut eligible = Vec::new();
    for address in addresses {
        match crate::services::chat_index::get(scope_root, address.chat_id().as_str()).await {
            Ok(Some(entry)) if entry.pinned => response.skipped.push(DeleteIssue {
                address,
                reason: "Chat became Pinned before deletion".to_string(),
            }),
            Ok(Some(_)) => eligible.push(address),
            Ok(None) => response.skipped.push(DeleteIssue {
                address,
                reason: "Chat no longer exists".to_string(),
            }),
            Err(error) => response.failed.push(DeleteIssue {
                address,
                reason: format!("Failed to revalidate Chat before deletion: {error}"),
            }),
        }
    }
    if !eligible.is_empty() {
        let deleting = eligible
            .iter()
            .map(|address| address.chat_id().to_string())
            .collect::<BTreeSet<_>>();
        let (removable_media, _) = chat_storage::removable_media(&snapshot, &deleting);
        let planned =
            chat_storage::planned_freed_bytes(scope_root, &snapshot, &deleting, &removable_media)
                .unwrap_or(0);
        let result = crate::endpoints::chats::batch_delete_core(
            scope_root,
            deleting.iter().cloned().collect(),
            &state,
        )
        .await;
        if result.status().is_success() {
            response.freed_bytes = response.freed_bytes.saturating_add(planned);
            response.deleted.extend(eligible);
        } else if result.status() == StatusCode::CONFLICT {
            for address in eligible {
                response.skipped.push(DeleteIssue {
                    address,
                    reason: "Chat changed or became Pinned before deletion".to_string(),
                });
            }
        } else {
            for address in eligible {
                response.failed.push(DeleteIssue {
                    address,
                    reason: format!("Chat authority delete failed with {}", result.status()),
                });
            }
        }
    }
    for (project_id, address) in project_chats {
        match crate::endpoints::projects::delete_project_chat_via_authority(
            &state,
            project_id,
            address.chat_id().to_string(),
        )
        .await
        {
            Ok(result) => {
                response.freed_bytes = response.freed_bytes.saturating_add(result.freed_bytes);
                response.deleted.push(address);
            }
            Err(crate::services::project_chat::ProjectChatDeleteError::Pinned(reason)) => {
                response.skipped.push(DeleteIssue { address, reason })
            }
            Err(error) => response.failed.push(DeleteIssue {
                address,
                reason: error.to_string(),
            }),
        }
    }
    let deleted_ids = response
        .deleted
        .iter()
        .map(|address| address.chat_id().to_string())
        .collect();
    let (_, preserved) = chat_storage::removable_media(&snapshot, &deleted_ids);
    response.shared_media_preserved = preserved
        .into_iter()
        .map(|(chat_id, media_ids)| SharedMedia {
            address: ChatAddress::new(chat_id),
            media_ids,
        })
        .collect();
    Json(response).into_response()
}

async fn load_storage_snapshot(state: &AppState) -> Result<ScopeStorageSnapshot, String> {
    let workspace = state.chat_workspace();
    let mut snapshot = chat_storage::scan_scope(workspace.as_path()).await?;
    let agents = state
        .agent_service
        .list_agents()
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|agent| (agent.id, agent.name))
        .collect::<HashMap<_, _>>();
    let projects = state
        .project_catalog
        .list()
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|project| (project.project_id.to_string(), project.display_name))
        .collect::<HashMap<_, _>>();
    for entry in &mut snapshot.entries {
        let names = if entry.owner.kind == "project" {
            &projects
        } else {
            &agents
        };
        if let Some(name) = names.get(&entry.owner.id) {
            entry.owner.name.clone_from(name);
        }
    }
    Ok(snapshot)
}
