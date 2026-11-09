use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use futures::{stream, StreamExt};
use rua_core::{ChatAddress, ChatScope, ChatScopeStore};
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
    let snapshots = match load_storage_snapshots(&state).await {
        Ok(snapshots) => snapshots,
        Err(error) => return ApiError::Internal(error).into_response(),
    };
    let mut response = PreviewStorageChatsResponse::default();
    let mut unique = HashSet::new();
    let mut groups: BTreeMap<String, Vec<ChatAddress>> = BTreeMap::new();
    for address in body.addresses {
        if !unique.insert(address.identity_key()) {
            continue;
        }
        let key = scope_key(address.scope());
        let Some(snapshot) = snapshots.get(&key) else {
            response.skipped.push(DeleteIssue {
                address,
                reason: "address is outside the local Agent/Project storage whitelist".to_string(),
            });
            continue;
        };
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
        groups.entry(key).or_default().push(address);
    }
    for (key, addresses) in groups {
        let snapshot = snapshots.get(&key).expect("validated storage scope");
        let deleting = addresses
            .iter()
            .map(|address| address.chat_id().to_string())
            .collect::<BTreeSet<_>>();
        let (removable_media, preserved) = chat_storage::removable_media(snapshot, &deleting);
        let scope_root = ChatScopeStore::resolve(&state.rua_root, addresses[0].clone())
            .root()
            .to_path_buf();
        match chat_storage::planned_freed_bytes(&scope_root, snapshot, &deleting, &removable_media)
        {
            Ok(bytes) => {
                response.reclaimable_bytes = response.reclaimable_bytes.saturating_add(bytes)
            }
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
    }
    Json(response).into_response()
}

async fn list_storage_chats(
    State(state): State<AppState>,
    Query(query): Query<ListStorageChatsQuery>,
) -> Response {
    match load_storage_snapshots(&state).await {
        Ok(snapshots) => {
            let mut entries = snapshots
                .into_values()
                .flat_map(|snapshot| snapshot.entries)
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
    let mut snapshots = match load_storage_snapshots(&state).await {
        Ok(snapshots) => snapshots,
        Err(error) => return ApiError::Internal(error).into_response(),
    };
    let mut response = DeleteStorageChatsResponse::default();
    let mut unique = HashSet::new();
    let mut groups: BTreeMap<String, Vec<ChatAddress>> = BTreeMap::new();
    for address in body.addresses {
        if !unique.insert(address.identity_key()) {
            response.skipped.push(DeleteIssue {
                address,
                reason: "duplicate address".to_string(),
            });
            continue;
        }
        let key = scope_key(address.scope());
        let Some(snapshot) = snapshots.get(&key) else {
            response.skipped.push(DeleteIssue {
                address,
                reason: "address is outside the local Agent/Project storage whitelist".to_string(),
            });
            continue;
        };
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
        groups.entry(key).or_default().push(address);
    }

    let executions = groups
        .into_iter()
        .filter_map(|(key, addresses)| snapshots.remove(&key).map(|snapshot| (snapshot, addresses)))
        .map(|(snapshot, addresses)| {
            let state = state.clone();
            async move { execute_storage_group(&state, &snapshot, addresses).await }
        });
    for mut group_response in stream::iter(executions)
        .buffer_unordered(4)
        .collect::<Vec<_>>()
        .await
    {
        response.deleted.append(&mut group_response.deleted);
        response.skipped.append(&mut group_response.skipped);
        response.failed.append(&mut group_response.failed);
        response.pending.append(&mut group_response.pending);
        response.freed_bytes = response
            .freed_bytes
            .saturating_add(group_response.freed_bytes);
        response
            .shared_media_preserved
            .append(&mut group_response.shared_media_preserved);
    }
    Json(response).into_response()
}

async fn execute_storage_group(
    state: &AppState,
    snapshot: &ScopeStorageSnapshot,
    addresses: Vec<ChatAddress>,
) -> DeleteStorageChatsResponse {
    let mut response = DeleteStorageChatsResponse::default();
    if let Some(first) = addresses.first() {
        match first.scope().clone() {
            ChatScope::Agent { agent_id, .. } => {
                let scope_root = ChatScopeStore::resolve(&state.rua_root, first.clone())
                    .root()
                    .to_path_buf();
                let mut eligible = Vec::new();
                for address in addresses {
                    match crate::services::chat_index::get(&scope_root, address.chat_id().as_str())
                        .await
                    {
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
                if eligible.is_empty() {
                    return response;
                }
                let deleting = eligible
                    .iter()
                    .map(|address| address.chat_id().to_string())
                    .collect::<BTreeSet<_>>();
                let (removable_media, preserved) =
                    chat_storage::removable_media(snapshot, &deleting);
                let planned = chat_storage::planned_freed_bytes(
                    &scope_root,
                    snapshot,
                    &deleting,
                    &removable_media,
                )
                .unwrap_or(0);
                let result = crate::endpoints::chats::batch_delete_core(
                    agent_id.as_str(),
                    &scope_root,
                    deleting.iter().cloned().collect(),
                    state,
                )
                .await;
                if result.status().is_success() {
                    response.freed_bytes = response.freed_bytes.saturating_add(planned);
                    for address in eligible {
                        if let Some(media_ids) = preserved.get(address.chat_id().as_str()) {
                            response.shared_media_preserved.push(SharedMedia {
                                address: address.clone(),
                                media_ids: media_ids.clone(),
                            });
                        }
                        response.deleted.push(address);
                    }
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
                            reason: format!(
                                "Chat authority delete failed with {}",
                                result.status()
                            ),
                        });
                    }
                }
            }
            ChatScope::Project { project_id, .. } => {
                for address in addresses {
                    match crate::endpoints::projects::delete_project_chat_via_authority(
                        state,
                        project_id.clone(),
                        address.chat_id().to_string(),
                    )
                    .await
                    {
                        Ok(result) => {
                            response.freed_bytes =
                                response.freed_bytes.saturating_add(result.freed_bytes);
                            if !result.shared_media_preserved.is_empty() {
                                response.shared_media_preserved.push(SharedMedia {
                                    address: address.clone(),
                                    media_ids: result.shared_media_preserved,
                                });
                            }
                            response.deleted.push(address);
                        }
                        Err(crate::services::project_chat::ProjectChatDeleteError::Pinned(_)) => {
                            response.skipped.push(DeleteIssue {
                                address,
                                reason: "Chat became Pinned before deletion".to_string(),
                            });
                        }
                        Err(error) => response.failed.push(DeleteIssue {
                            address,
                            reason: error.to_string(),
                        }),
                    }
                }
            }
        }
    }
    response
}

async fn load_storage_snapshots(
    state: &AppState,
) -> Result<HashMap<String, ScopeStorageSnapshot>, String> {
    let mut snapshots = HashMap::new();
    let mut agents = state
        .agent_service
        .list_agents()
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|agent| (agent.id, agent.name))
        .collect::<BTreeMap<_, _>>();
    extend_local_owner_dirs(&state.rua_root.join("workspaces"), &mut agents)?;
    for (agent_id, agent_name) in agents {
        let scope = ChatScope::agent(agent_id);
        let scope_root = ChatScopeStore::resolve(
            &state.rua_root,
            ChatAddress::new(scope.clone(), "__storage_scan__"),
        )
        .root()
        .to_path_buf();
        snapshots.insert(
            scope_key(&scope),
            chat_storage::scan_scope(&scope_root, scope, agent_name).await?,
        );
    }
    let mut projects = state
        .project_catalog
        .list()
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|project| (project.project_id.to_string(), project.display_name))
        .collect::<BTreeMap<_, _>>();
    extend_local_owner_dirs(&state.rua_root.join("projects"), &mut projects)?;
    for (project_id, project_name) in projects {
        let scope = ChatScope::project(project_id);
        let scope_root = ChatScopeStore::resolve(
            &state.rua_root,
            ChatAddress::new(scope.clone(), "__storage_scan__"),
        )
        .root()
        .to_path_buf();
        snapshots.insert(
            scope_key(&scope),
            chat_storage::scan_scope(&scope_root, scope, project_name).await?,
        );
    }
    Ok(snapshots)
}

fn extend_local_owner_dirs(
    root: &std::path::Path,
    owners: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "read local storage root {}: {error}",
                root.display()
            ))
        }
    };
    for entry in entries {
        let entry = entry.map_err(|error| format!("read {} entry: {error}", root.display()))?;
        if !entry
            .file_type()
            .map_err(|error| format!("inspect {}: {error}", entry.path().display()))?
            .is_dir()
        {
            continue;
        }
        let Some(owner_id) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        owners.entry(owner_id.clone()).or_insert(owner_id);
    }
    Ok(())
}

fn scope_key(scope: &ChatScope) -> String {
    format!("{}:{}", scope.kind(), scope.owner_id())
}
