use std::path::{Path, PathBuf};

use rua_chat::TurnAgentAssignment;
use rua_core::ChatAddress;
use rua_session::WorkspaceSessionStore;

use crate::services::chat_index::{self, ChatIndexEntry};
use crate::services::conversation_authority::{
    ConversationInputAcceptanceMode, ConversationInputPayload,
    FirstConversationSubmissionPersistenceContext,
};
use crate::services::conversation_authority_placement::ConversationAuthorityPlacement;

pub(crate) struct AgentChatMaterializationSnapshot {
    pub initial_model: Option<serde_json::Value>,
    pub configuration: crate::services::product_hooks::ChatHookConfiguration,
}

pub(crate) async fn persist_first_agent_submission(
    scope_root: &Path,
    address: &ChatAddress,
    agent_id: &str,
    input: &ConversationInputPayload,
    persistence_context: &FirstConversationSubmissionPersistenceContext,
    placement: &ConversationAuthorityPlacement,
    snapshot: AgentChatMaterializationSnapshot,
) -> Result<(), String> {
    let media_ids = validate_submission_media(scope_root, input).await?;
    let store = WorkspaceSessionStore::new(scope_root);
    let indexed = chat_index::get(scope_root, address.chat_id().as_str()).await?;
    let header = store
        .load_chat_header(address.chat_id().as_str())
        .await
        .map_err(|error| error.to_string())?;
    match (indexed, header) {
        (None, None) => {
            let assignment = snapshot
                .configuration
                .agent_assignment
                .clone()
                .ok_or_else(|| "Agent Turn Assignment is unavailable".to_string())?;
            materialize_staged_chat(scope_root, address, |staging_root| async move {
                crate::services::console_chat::materialize_targeted_chat_with_model(
                    &staging_root,
                    agent_id,
                    address.chat_id().as_str(),
                    &input.message,
                    placement,
                    snapshot.initial_model,
                )
                .await
                .map_err(|error| error.to_string())?;
                let staging_store = WorkspaceSessionStore::new(&staging_root);
                let configuration_value = serde_json::to_value(&snapshot.configuration)
                    .map_err(|error| error.to_string())?;
                staging_store
                    .update_chat_header(address.chat_id().as_str(), |header| {
                        let mut activity = serde_json::Value::Object(header.extra.clone());
                        crate::services::chat_device_activity::apply_chat_created(
                            &mut activity,
                            &crate::services::chat_device_activity::ChatDeviceActivityUpdate {
                                device_id: Some(placement.device_id.clone()),
                                host_profile: String::new(),
                            },
                        );
                        crate::services::chat_device_activity::copy_chat_activity_to_header_extra(
                            &activity,
                            &mut header.extra,
                        );
                        header.extra.insert(
                            crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY
                                .to_string(),
                            configuration_value,
                        );
                        if let Some(selection) = input
                            .extra
                            .get(crate::services::conversation_input_intake::INITIAL_ACP_KEY)
                        {
                            header.extra.insert(
                                crate::services::conversation_input_intake::INITIAL_ACP_KEY.into(),
                                selection.clone(),
                            );
                        }
                    })
                    .await
                    .map_err(|error| error.to_string())?;
                append_accepted_submission(
                    &staging_root,
                    address,
                    &assignment,
                    input,
                    persistence_context,
                    placement,
                )
                .await?;
                let mut index = chat_index::get(&staging_root, address.chat_id().as_str())
                    .await?
                    .ok_or_else(|| "staged Agent Chat index entry is missing".to_string())?;
                crate::services::chat_device_activity::apply_chat_created(
                    &mut index.meta,
                    &crate::services::chat_device_activity::ChatDeviceActivityUpdate {
                        device_id: Some(placement.device_id.clone()),
                        host_profile: String::new(),
                    },
                );
                if let Some(user) = input
                    .extra
                    .get("user_id")
                    .and_then(serde_json::Value::as_str)
                {
                    index.user_id = user.to_string();
                }
                if let Some(channel) = input
                    .extra
                    .get("channel")
                    .and_then(serde_json::Value::as_str)
                {
                    index.channel = channel.to_string();
                }
                Ok(index)
            })
            .await?;
            if let Err(error) = crate::services::chat_media::ChatMediaService::confirm_staged_media(
                scope_root, &media_ids,
            )
            .await
            {
                tracing::warn!(%error, "accepted Chat media staging marker retained");
            }
            Ok(())
        }
        (Some(_), Some(header)) => {
            let winner_placement =
                crate::services::conversation_authority_placement::parse_conversation_authority_placement(
                    &serde_json::Value::Object(header.extra.clone()),
                )
                .map_err(|error| error.to_string())?;
            if winner_placement.device_id != placement.device_id
                || winner_placement.base_manifest_hash != placement.base_manifest_hash
            {
                return Err("Agent Chat was materialized by a different target".to_string());
            }
            let winner = header
                .extra
                .get(crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY)
                .cloned()
                .ok_or_else(|| "Chat configuration snapshot is unavailable".to_string())
                .and_then(crate::services::product_hooks::ChatHookConfiguration::from_persisted)?;
            let assignment = winner
                .agent_assignment
                .clone()
                .ok_or_else(|| "Agent Turn Assignment is unavailable".to_string())?;
            let effective_input = input_with_configuration(input, &winner)?;
            append_accepted_submission(
                scope_root,
                address,
                &assignment,
                &effective_input,
                persistence_context,
                &winner_placement,
            )
            .await
        }
        _ => Err("Agent Chat storage and index are inconsistent".to_string()),
    }
}

pub(crate) async fn materialize_staged_chat<F, Fut>(
    scope_root: &Path,
    address: &ChatAddress,
    stage: F,
) -> Result<(), String>
where
    F: FnOnce(PathBuf) -> Fut,
    Fut: std::future::Future<Output = Result<ChatIndexEntry, String>>,
{
    if chat_index::get(scope_root, address.chat_id().as_str())
        .await?
        .is_some()
    {
        return Err("Chat already exists".to_string());
    }
    let final_store = WorkspaceSessionStore::new(scope_root);
    let final_chat_dir = final_store.chat_dir(address.chat_id().as_str());
    if final_chat_dir.exists() {
        return Err("Chat storage already exists".to_string());
    }

    let staging_root = scope_root.join(".materializing").join(format!(
        "{}-{}",
        address.chat_id(),
        uuid::Uuid::new_v4()
    ));
    let result = async {
        let entry = stage(staging_root.clone()).await?;
        commit_staged_chat(scope_root, address, &staging_root, entry).await
    }
    .await;
    remove_dir_if_exists(&staging_root).await;
    result
}

pub(crate) async fn validate_submission_media(
    root: &Path,
    input: &ConversationInputPayload,
) -> Result<Vec<String>, String> {
    let mut ids = Vec::new();
    let messages = input.input.as_ref().or(input.messages.as_ref());
    if let Some(messages) = messages {
        for message in messages {
            if let Some(parts) = message.get("content").and_then(serde_json::Value::as_array) {
                for part in parts {
                    if let Some(reference) =
                        part.get("media_ref").and_then(serde_json::Value::as_str)
                    {
                        let id = reference
                            .strip_prefix("media/")
                            .ok_or("invalid media reference")?;
                        uuid::Uuid::parse_str(id).map_err(|_| "invalid media identity")?;
                        crate::services::remote_conversation_input::media_fingerprint(
                            &root.join("media").join(id),
                        )
                        .await?;
                        ids.push(id.to_string());
                    }
                }
            }
        }
    }
    Ok(ids)
}

pub(crate) async fn append_accepted_submission(
    scope_root: &Path,
    address: &ChatAddress,
    assignment: &TurnAgentAssignment,
    input: &ConversationInputPayload,
    persistence_context: &FirstConversationSubmissionPersistenceContext,
    placement: &ConversationAuthorityPlacement,
) -> Result<(), String> {
    let store = WorkspaceSessionStore::new(scope_root);
    let user_message =
        crate::services::conversation_input_intake::user_input_message_from_payload(input)?;
    let assignment_metadata =
        serde_json::to_value(assignment).map_err(|error| error.to_string())?;
    let delivery = crate::services::console_chat::DeviceDeliveryMetadata::from_extra(&input.extra);
    let collaboration_mode = input
        .extra
        .get(rua_core::TURN_COLLABORATION_MODE_OVERRIDE_KEY)
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| format!("invalid Agent Collaboration Mode: {error}"))?
        .unwrap_or_default();
    let acceptance_mode = match persistence_context.mode {
        ConversationInputAcceptanceMode::Started => "started",
        ConversationInputAcceptanceMode::Queued => "queued",
    };
    let (turn, _) = crate::services::console_chat::build_submitted_user_turn(
        &assignment.agent_id,
        &user_message,
        &placement.device_id,
        crate::services::console_chat::SubmittedUserTurnMetadata {
            turn_id: Some(&persistence_context.turn_id),
            input_id: Some(&persistence_context.input_id),
            acceptance_mode: Some(acceptance_mode),
            submission_id: Some(&persistence_context.submission_id),
            device_delivery: delivery.as_ref(),
            acp_origin_runner: None,
            memory_signal: None,
            turn_agent_assignment: Some(&assignment_metadata),
            turn_model: input.extra.get(rua_chat::TURN_MODEL_KEY),
            turn_thinking: input.extra.get(rua_chat::TURN_THINKING_KEY),
            collaboration_mode,
            authority_device_id: &placement.device_id,
            authority_epoch: placement.authority_epoch,
        },
    )
    .map_err(|error| error.to_string())?
    .ok_or_else(|| "first Conversation submission has no user content".to_string())?;
    store
        .append_conversation_turn(address.chat_id().as_str(), turn)
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
}

pub(crate) fn input_with_configuration(
    input: &ConversationInputPayload,
    configuration: &crate::services::product_hooks::ChatHookConfiguration,
) -> Result<ConversationInputPayload, String> {
    let mut input = input.clone();
    if let Some(assignment) = configuration.agent_assignment.as_ref() {
        input.extra.insert(
            rua_chat::TURN_AGENT_ASSIGNMENT_META_KEY.to_string(),
            serde_json::to_value(assignment).map_err(|error| error.to_string())?,
        );
    }
    let (model, thinking) = rua_chat::capture_turn_model_and_thinking(
        configuration.model.as_ref(),
        configuration.thinking.as_ref(),
    );
    match model {
        Some(model) => {
            input
                .extra
                .insert(rua_chat::TURN_MODEL_KEY.to_string(), model);
        }
        None => {
            input.extra.remove(rua_chat::TURN_MODEL_KEY);
        }
    }
    match thinking {
        Some(thinking) => {
            input
                .extra
                .insert(rua_chat::TURN_THINKING_KEY.to_string(), thinking);
        }
        None => {
            input.extra.remove(rua_chat::TURN_THINKING_KEY);
        }
    }
    Ok(input)
}

async fn commit_staged_chat(
    scope_root: &Path,
    address: &ChatAddress,
    staging_root: &Path,
    entry: ChatIndexEntry,
) -> Result<(), String> {
    let final_store = WorkspaceSessionStore::new(scope_root);
    let final_chat_dir = final_store.chat_dir(address.chat_id().as_str());
    let staging_chat_dir =
        WorkspaceSessionStore::new(staging_root).chat_dir(address.chat_id().as_str());
    let final_sessions_dir = final_store.sessions_dir().to_path_buf();
    tokio::fs::create_dir_all(&final_sessions_dir)
        .await
        .map_err(|error| format!("create Chat storage directory: {error}"))?;
    rename_directory_atomically(staging_chat_dir, final_chat_dir.clone()).await?;
    if let Err(error) = chat_index::insert(scope_root, entry).await {
        remove_dir_if_exists(&final_chat_dir).await;
        return Err(error);
    }
    Ok(())
}

async fn rename_directory_atomically(source: PathBuf, target: PathBuf) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        std::fs::rename(&source, &target).map_err(|error| {
            format!(
                "commit Chat directory {} -> {}: {error}",
                source.display(),
                target.display()
            )
        })?;
        if let Some(parent) = target.parent() {
            if let Err(error) =
                std::fs::File::open(parent).and_then(|directory| directory.sync_all())
            {
                if let Err(cleanup_error) = std::fs::remove_dir_all(&target) {
                    tracing::warn!(%cleanup_error, path = %target.display(), "failed to roll back Chat directory");
                }
                return Err(format!(
                    "fsync Chat storage directory {}: {error}",
                    parent.display()
                ));
            }
        }
        Ok(())
    })
    .await
    .map_err(|error| format!("join Chat directory commit: {error}"))?
}

async fn remove_dir_if_exists(path: &Path) {
    if let Err(error) = tokio::fs::remove_dir_all(path).await {
        if error.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(
                path = %path.display(),
                error = %error,
                "failed to clean Chat materialization directory"
            );
        }
    }
}
