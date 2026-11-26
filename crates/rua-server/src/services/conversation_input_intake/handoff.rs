//! Cooperative transfer of the one Chat writer. The local journal, rather than
//! a network acknowledgement, decides whether the old writer may still admit work.

use std::collections::BTreeMap;
use std::time::Duration;

use rua_core::{ChatAddress, ChatStore, ExecutionDirectorySelection};
use rua_session::chat_handoff::{ChatHandoffPhase, ChatHandoffRecord};
use rua_session::WorkspaceSessionStore;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::handoff_history::{self, HandoffHistorySnapshot};
use super::handoff_settings::{self, HandoffSettingsSnapshot};
use super::ConversationInputIntake;
use crate::services::chat_metadata_store::{load_chat_record, ChatMetadataRecord};
use crate::services::conversation_authority_placement::{
    parse_conversation_authority_placement, ConversationAuthorityEpoch,
    ConversationAuthorityPlacement,
};
use crate::services::conversation_history_readiness::{
    ensure_conversation_history_ready_via_device_session, ConversationHistoryGapRequest,
    ConversationHistorySyncRequest,
};
use crate::services::conversation_manifest::{load_conversation_manifest, ConversationManifest};
use crate::services::device_context::current_sync_device_id;
use crate::services::product_hooks::ChatHookConfiguration;

const HANDOFF_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatHandoffCommand {
    pub source_device_id: String,
    pub target_device_id: String,
    pub address: ChatAddress,
    pub action: ChatHandoffAction,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatHandoffAction {
    Request {
        expected: ConversationAuthorityPlacement,
        selection: ExecutionDirectorySelection,
    },
    Prepare {
        record: Box<ChatHandoffRecord>,
        configuration: Box<Value>,
    },
    Activate {
        handoff_id: String,
    },
    Status {
        handoff_id: String,
    },
    Abort {
        handoff_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatHandoffResponse {
    pub record: ChatHandoffRecord,
    pub candidate: Option<Value>,
    pub chat: Option<ChatMetadataRecord>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Requirements {
    agent_id: String,
    selection: ExecutionDirectorySelection,
    manifest: ConversationManifest,
    history: Option<HandoffHistorySnapshot>,
    queue_paused: bool,
    settings: HandoffSettingsSnapshot,
    /// Whether the Authority could publish this snapshot. With Sync off nothing was
    /// published, so the target keeps its own applied Chat rules.
    #[serde(default)]
    settings_published: bool,
    // Only immutable routing identity is required. A live Task may change its
    // status throughout handoff, including while its Device is unreachable.
    task_refs: BTreeMap<String, Value>,
    #[serde(default, flatten)]
    extra: BTreeMap<String, Value>,
}

fn decode<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T, String> {
    serde_json::from_value(value.clone()).map_err(|error| error.to_string())
}

fn task_ref_identities(
    store: &WorkspaceSessionStore,
    chat_id: &str,
) -> Result<BTreeMap<String, Value>, String> {
    rua_session::task_refs::ChatTaskRef::list(&store.chat_dir(chat_id), chat_id)
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|reference| {
            Ok((
                reference.task_id,
                serde_json::json!({
                    "owner": reference.owner,
                    "execution": reference.execution,
                    "origin": reference.origin,
                }),
            ))
        })
        .collect()
}

impl ConversationInputIntake {
    pub(crate) async fn handoff_chat_authority(
        &self,
        address: &ChatAddress,
        expected: ConversationAuthorityPlacement,
        selection: ExecutionDirectorySelection,
    ) -> Result<ChatMetadataRecord, String> {
        selection.validate().map_err(|error| error.to_string())?;
        let local =
            current_sync_device_id(&self.sync_device).ok_or("Device identity unavailable")?;
        if expected.device_id != local {
            let response = self
                .handoff_rpc(
                    address,
                    &expected.device_id,
                    ChatHandoffAction::Request {
                        expected: expected.clone(),
                        selection,
                    },
                )
                .await?;
            return response
                .chat
                .ok_or("Handoff did not confirm a new Chat authority".into());
        }
        let response = self
            .handoff_from_current(address, expected, selection)
            .await?;
        response
            .chat
            .ok_or("Handoff did not confirm a new Chat authority".into())
    }

    pub(crate) async fn handle_chat_handoff(
        &self,
        command: ChatHandoffCommand,
    ) -> Result<ChatHandoffResponse, String> {
        let local =
            current_sync_device_id(&self.sync_device).ok_or("Device identity unavailable")?;
        if command.target_device_id != local || command.source_device_id == local {
            return Err("Invalid handoff Device route".into());
        }
        let address = &command.address;
        match command.action {
            ChatHandoffAction::Request {
                expected,
                selection,
            } => {
                if expected.device_id != local {
                    return Err("Only the current Authority can release this Chat".into());
                }
                Box::pin(self.handoff_from_current(address, expected, selection)).await
            }
            ChatHandoffAction::Prepare {
                record,
                configuration,
            } => {
                self.prepare_handoff_target(
                    address,
                    &command.source_device_id,
                    *record,
                    *configuration,
                )
                .await
            }
            ChatHandoffAction::Activate { handoff_id } => {
                self.activate_handoff_target(address, &command.source_device_id, &handoff_id)
                    .await
            }
            ChatHandoffAction::Status { handoff_id } => {
                let store = self.handoff_store(address);
                let record = store
                    .load_chat_handoff(address.chat_id().as_str())
                    .await
                    .map_err(|e| e.to_string())?
                    .ok_or("Handoff is unavailable")?;
                self.validate_handoff_participant(
                    address,
                    &command.source_device_id,
                    &record,
                    &handoff_id,
                )?;
                if record.phase == ChatHandoffPhase::Released {
                    self.activate_handoff_target(address, &command.source_device_id, &handoff_id)
                        .await
                } else {
                    self.handoff_response(address, record).await
                }
            }
            ChatHandoffAction::Abort { handoff_id } => {
                let store = self.handoff_store(address);
                let record = store
                    .load_chat_handoff(address.chat_id().as_str())
                    .await
                    .map_err(|e| e.to_string())?
                    .ok_or("Handoff is unavailable")?;
                self.validate_handoff_participant(
                    address,
                    &command.source_device_id,
                    &record,
                    &handoff_id,
                )?;
                let authority = self
                    .conversation_authority
                    .as_ref()
                    .ok_or("Conversation Authority unavailable")?;
                let _gate = authority.transition_gate(address).lock_owned().await;
                let record = store
                    .abort_chat_handoff(address.chat_id().as_str(), &handoff_id)
                    .await
                    .map_err(|e| e.to_string())?;
                self.handoff_response(address, record).await
            }
        }
    }

    /// Activation rewrites the Session header outside the ordinary settings path,
    /// so the Chat Index projection must be repaired for readers of Chat metadata.
    async fn repair_chat_projection(&self, address: &ChatAddress) -> Result<(), String> {
        let scope = ChatStore::resolve(self.session_sync.rua_root(), address.clone());
        rua_chat::ChatService::from_workspace(rua_core::WorkspaceRoot::new(scope.root()))
            .repair_projection(address)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn handoff_store(&self, address: &ChatAddress) -> WorkspaceSessionStore {
        WorkspaceSessionStore::new(
            ChatStore::resolve(self.session_sync.rua_root(), address.clone()).root(),
        )
    }

    async fn handoff_rpc(
        &self,
        address: &ChatAddress,
        target: &str,
        action: ChatHandoffAction,
    ) -> Result<ChatHandoffResponse, String> {
        let source =
            current_sync_device_id(&self.sync_device).ok_or("Device identity unavailable")?;
        let manager = crate::services::device_session_access::current_session_manager(
            self.remote_input_delivery.device_network(),
        )
        .await
        .map_err(|error| format!("Chat handoff unavailable: {error}"))?;
        let timeout = match &action {
            ChatHandoffAction::Abort { .. } => Duration::from_secs(5),
            ChatHandoffAction::Status { .. } => Duration::from_secs(15),
            _ => HANDOFF_TIMEOUT,
        };
        crate::services::device_command_bus::deliver_chat_handoff_via_session(
            &manager,
            &ChatHandoffCommand {
                address: address.clone(),
                source_device_id: source,
                target_device_id: target.into(),
                action,
                extra: BTreeMap::new(),
            },
            timeout,
        )
        .await
        .map_err(|error| match error {
            crate::services::device_command_bus::DeviceCommandDeliveryError::Rejected {
                detail,
                ..
            } => detail,
            other => format!("Chat handoff unavailable: {other}"),
        })
    }

    async fn sync_handoff_state(&self, peer: &str) -> Result<(), String> {
        let tx = self
            .history_sync_tx
            .as_ref()
            .ok_or("Chat sync runtime unavailable")?;
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        tx.send(ConversationHistorySyncRequest {
            source_device_id: peer.into(),
            result_tx,
        })
        .map_err(|_| "Chat sync runtime stopped")?;
        let result = tokio::time::timeout(Duration::from_secs(30), result_rx)
            .await
            .map_err(|_| "Chat settings synchronization timed out".to_string())
            .and_then(|result| {
                result.map_err(|_| "Chat settings synchronization was interrupted".to_string())
            })
            .and_then(|result| result);
        if let Err(error) = result {
            // General synchronization may still contain unrelated work from an
            // offline Device. Ready/Release below require the applied history,
            // configuration, permissions and Task identities for this Chat;
            // they never infer readiness from a successful transport cycle.
            tracing::debug!(peer, %error, "Handoff will verify its required applied Chat state");
        }
        Ok(())
    }

    async fn check_handoff_idle(&self, address: &ChatAddress) -> Result<(), String> {
        let authority = self
            .conversation_authority
            .as_ref()
            .ok_or("Conversation Authority unavailable")?;
        let approvals = self
            .approval_authority
            .as_ref()
            .ok_or("Approval Authority unavailable")?;
        let header = self.chat_header(address).await?;
        // A Project Chat's authority is fixed at creation; report that before any
        // transient blocker so the user is never told to drain a queue that could
        // not make the handoff legal anyway.
        let scope = ChatStore::resolve(self.session_sync.rua_root(), address.clone());
        let policy =
            rua_chat::ChatService::from_workspace(rua_core::WorkspaceRoot::new(scope.root()))
                .policy(address)
                .await
                .map_err(|e| e.to_string())?;
        if policy.authority_placement == rua_chat::AuthorityPlacementPolicy::Fixed {
            return Err("Chat authority placement is fixed on its execution Device".into());
        }
        let state = crate::services::project_chat::chat_agent_state_from_header(
            address, &header, true, authority, approvals,
        )
        .await?;
        if !state.blockers.is_empty() {
            use crate::services::project_chat::ChatAgentSwitchBlocker;
            if state.blockers.contains(&ChatAgentSwitchBlocker::ActiveTurn) {
                return Err(
                    "cannot change authority placement while the chat has an active turn".into(),
                );
            }
            if state
                .blockers
                .contains(&ChatAgentSwitchBlocker::ActiveAcpBinding)
            {
                return Err(
                    "Current chat is in ACP mode. Exit ACP before changing the authority device."
                        .into(),
                );
            }
            return Err(format!(
                "Chat handoff blocked by {}",
                serde_json::json!(state.blockers)
            ));
        }
        Ok(())
    }

    async fn handoff_from_current(
        &self,
        address: &ChatAddress,
        expected: ConversationAuthorityPlacement,
        selection: ExecutionDirectorySelection,
    ) -> Result<ChatHandoffResponse, String> {
        let _project = self.project_work_admission(address).await?;
        let authority = self
            .conversation_authority
            .as_ref()
            .ok_or("Conversation Authority unavailable")?;
        let approvals = self
            .approval_authority
            .as_ref()
            .ok_or("Approval Authority unavailable")?;
        let store = self.handoff_store(address);
        let scope = ChatStore::resolve(self.session_sync.rua_root(), address.clone());
        let chat_id = address.chat_id().as_str();
        let mut record = {
            let _gate = authority.transition_gate(address).lock_owned().await;
            let _registrations = approvals.hold_chat_registrations(address).await;
            let previous = store
                .load_chat_handoff(chat_id)
                .await
                .map_err(|e| e.to_string())?;
            if let Some(record) =
                previous.filter(|record| record.phase != ChatHandoffPhase::Aborted)
            {
                let requirements: Requirements = decode(&record.requirements)?;
                let old: ConversationAuthorityPlacement = decode(&record.old_placement)?;
                if old == expected && requirements.selection == selection {
                    record
                } else if matches!(
                    record.phase,
                    ChatHandoffPhase::Prepared | ChatHandoffPhase::Released
                ) {
                    return Err(
                        "Another Chat handoff is pending; reconcile that operation first".into(),
                    );
                } else {
                    self.new_handoff_record(address, &expected, &selection)
                        .await?
                }
            } else {
                self.new_handoff_record(address, &expected, &selection)
                    .await?
            }
        };
        if record.phase == ChatHandoffPhase::Activated {
            return self.handoff_response(address, record).await;
        }
        if record.phase == ChatHandoffPhase::Prepared {
            let prepared = self.ready_handoff_target(address, &mut record).await;
            let candidate = match prepared {
                Ok(candidate) => candidate,
                Err(error) => {
                    self.abort_handoff_preparation(address, &record).await;
                    return Err(error);
                }
            };
            let released: Result<ChatHandoffRecord, String> = async {
                let _gate = authority.transition_gate(address).lock_owned().await;
                let _registrations = approvals.hold_chat_registrations(address).await;
                self.check_handoff_idle(address).await?;
                let current = self.chat_header(address).await?;
                if parse_conversation_authority_placement(&Value::Object(current.extra.clone()))
                    .map_err(|e| e.to_string())?
                    != expected
                {
                    return Err("Chat authority changed before handoff release".into());
                }
                let requirements: Requirements = decode(&record.requirements)?;
                let manifest = load_conversation_manifest(scope.root(), address)
                    .await
                    .map_err(|e| e.to_string())?
                    .ok_or("Chat history unavailable")?;
                if !manifest.compare(&requirements.manifest).matches
                    || task_ref_identities(&store, chat_id)? != requirements.task_refs
                    || authority.is_queue_drain_paused(address).await != requirements.queue_paused
                    || Some(handoff_history::capture(scope.root(), chat_id).await?)
                        != requirements.history
                {
                    return Err(
                        "Chat history or Task references changed before handoff release".into(),
                    );
                }
                store
                    .prepare_chat_handoff(&record, Some(&candidate))
                    .await
                    .map_err(|e| e.to_string())?;
                handoff_settings::with_unchanged(
                    self.session_sync.rua_root(),
                    address,
                    &requirements.agent_id,
                    &requirements.settings,
                    || {
                        store
                            .release_chat_handoff(chat_id, &record.handoff_id)
                            .map_err(std::io::Error::other)
                    },
                )
                .map_err(|e| e.to_string())
            }
            .await;
            match released {
                Ok(released) => record = released,
                Err(error) => {
                    self.abort_handoff_preparation(address, &record).await;
                    return Err(error);
                }
            }
        }
        self.finish_released_handoff(address, record).await
    }

    pub(super) async fn recover_released_handoff_before_routing(
        &self,
        address: &ChatAddress,
    ) -> Result<(), String> {
        let Some(record) = self
            .handoff_store(address)
            .load_chat_handoff(address.chat_id().as_str())
            .await
            .map_err(|error| error.to_string())?
        else {
            return Ok(());
        };
        if record.phase != ChatHandoffPhase::Released {
            return Ok(());
        }
        let old: ConversationAuthorityPlacement = decode(&record.old_placement)?;
        let new: ConversationAuthorityPlacement = decode(&record.new_placement)?;
        let local =
            current_sync_device_id(&self.sync_device).ok_or("Device identity unavailable")?;
        if local == old.device_id {
            self.finish_released_handoff(address, record).await?;
        } else if local == new.device_id {
            self.activate_handoff_target(address, &old.device_id, &record.handoff_id)
                .await?;
        } else {
            return Err("Chat handoff journal belongs to another Device".into());
        }
        Ok(())
    }

    async fn finish_released_handoff(
        &self,
        address: &ChatAddress,
        record: ChatHandoffRecord,
    ) -> Result<ChatHandoffResponse, String> {
        let next: ConversationAuthorityPlacement = decode(&record.new_placement)?;
        let authority = self
            .conversation_authority
            .as_ref()
            .ok_or("Conversation Authority unavailable")?;
        let store = self.handoff_store(address);
        let chat_id = address.chat_id().as_str();
        let response = match self
            .handoff_rpc(
                address,
                &next.device_id,
                ChatHandoffAction::Activate {
                    handoff_id: record.handoff_id.clone(),
                },
            )
            .await
        {
            Ok(response) => response,
            Err(error) => {
                let status = self
                    .handoff_rpc(
                        address,
                        &next.device_id,
                        ChatHandoffAction::Status {
                            handoff_id: record.handoff_id.clone(),
                        },
                    )
                    .await;
                match status {
                    Ok(response) if response.record.phase == ChatHandoffPhase::Activated => {
                        response
                    }
                    _ => {
                        return Err(format!(
                            "Handoff {} is released and awaiting target confirmation: {error}",
                            record.handoff_id
                        ))
                    }
                }
            }
        };
        if response.record.handoff_id != record.handoff_id
            || response.record.new_placement != record.new_placement
            || response.record.phase != ChatHandoffPhase::Activated
        {
            return Err("Target confirmed a different Chat handoff".into());
        }
        let _gate = authority.transition_gate(address).lock_owned().await;
        let installed = store
            .activate_chat_handoff(chat_id, &record.handoff_id)
            .await
            .map_err(|e| e.to_string())?;
        self.repair_chat_projection(address).await?;
        self.publish_handoff_installed(address, &installed).await?;
        Ok(response)
    }

    async fn abort_handoff_preparation(&self, address: &ChatAddress, record: &ChatHandoffRecord) {
        let Some(authority) = self.conversation_authority.as_ref() else {
            return;
        };
        let _gate = authority.transition_gate(address).lock_owned().await;
        if self
            .handoff_store(address)
            .abort_chat_handoff(address.chat_id().as_str(), &record.handoff_id)
            .await
            .is_err()
        {
            return; // A durable Released intent can never reopen the old writer.
        }
        drop(_gate);
        if let Ok(next) = decode::<ConversationAuthorityPlacement>(&record.new_placement) {
            let _ = self
                .handoff_rpc(
                    address,
                    &next.device_id,
                    ChatHandoffAction::Abort {
                        handoff_id: record.handoff_id.clone(),
                    },
                )
                .await;
        }
    }

    async fn new_handoff_record(
        &self,
        address: &ChatAddress,
        expected: &ConversationAuthorityPlacement,
        selection: &ExecutionDirectorySelection,
    ) -> Result<ChatHandoffRecord, String> {
        self.check_handoff_idle(address).await?;
        let header = self.chat_header(address).await?;
        let current = parse_conversation_authority_placement(&Value::Object(header.extra.clone()))
            .map_err(|e| e.to_string())?;
        if current != *expected || expected.device_id == selection.device_id {
            return Err("Chat authority placement changed or target is already current".into());
        }
        let scope = ChatStore::resolve(self.session_sync.rua_root(), address.clone());
        let configuration = self
            .load_chat_configuration_locked(address, &header.agent_id, scope.root())
            .await?
            .ok_or("Chat configuration unavailable")?;
        let manifest = load_conversation_manifest(scope.root(), address)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("Chat history unavailable")?;
        if manifest
            .latest_turn_status
            .as_deref()
            .is_some_and(|status| {
                !rua_session::TurnLifecycleStatus::parse(status).is_syncable_conversation_state()
            })
        {
            return Err("latest conversation turn is not terminal".into());
        }
        let next = ConversationAuthorityPlacement {
            device_id: selection.device_id.clone(),
            authority_epoch: ConversationAuthorityEpoch::next_for_device(
                &selection.device_id,
                Some(expected.authority_epoch),
            ),
            base_manifest_hash: manifest.hash().map_err(|e| e.to_string())?,
        };
        let store = self.handoff_store(address);
        let requirements = Requirements {
            agent_id: header.agent_id.clone(),
            selection: selection.clone(),
            manifest,
            history: None,
            queue_paused: false,
            settings: handoff_settings::capture(
                self.session_sync.rua_root(),
                address,
                &header.agent_id,
            )
            .map_err(|e| e.to_string())?,
            task_refs: task_ref_identities(&store, address.chat_id().as_str())?,
            settings_published: false,
            extra: BTreeMap::new(),
        };
        let record = ChatHandoffRecord {
            handoff_id: format!("handoff-{}", uuid::Uuid::new_v4()),
            chat_id: address.chat_id().to_string(),
            old_placement: serde_json::json!(expected),
            new_placement: serde_json::json!(next),
            configuration: Some(
                serde_json::to_value(&configuration).map_err(|error| error.to_string())?,
            ),
            candidate_configuration: None,
            requirements: serde_json::json!(requirements),
            phase: ChatHandoffPhase::Prepared,
            extra: Default::default(),
        };
        store
            .prepare_chat_handoff(&record, None)
            .await
            .map_err(|e| e.to_string())
    }

    async fn ready_handoff_target(
        &self,
        address: &ChatAddress,
        record: &mut ChatHandoffRecord,
    ) -> Result<Value, String> {
        let store = self.handoff_store(address);
        let scope = ChatStore::resolve(self.session_sync.rua_root(), address.clone());
        let mut requirements: Requirements = decode(&record.requirements)?;
        let approvals = self
            .approval_authority
            .as_ref()
            .ok_or("Approval Authority unavailable")?;
        // Prepared already fences history writers, including delayed checkpoint
        // publication. Capture after that fence, before exporting the records.
        requirements.history =
            Some(handoff_history::capture(scope.root(), address.chat_id().as_str()).await?);
        self.session_sync
            .emit_evergreen_chat_snapshot(scope.root(), address)
            .await;
        requirements.settings_published = self
            .session_sync
            .publish_handoff_settings(scope.root(), address, &requirements.agent_id, approvals)
            .await?;
        // Bidirectional synchronization preserves legitimate newer shared settings
        // on C. Read the resulting persisted projections, not Agent defaults.
        self.sync_handoff_state(&requirements.selection.device_id)
            .await?;
        requirements.settings = handoff_settings::capture(
            self.session_sync.rua_root(),
            address,
            &requirements.agent_id,
        )
        .map_err(|e| e.to_string())?;
        {
            let authority = self
                .conversation_authority
                .as_ref()
                .ok_or("Conversation Authority unavailable")?;
            let _gate = authority.transition_gate(address).lock_owned().await;
            requirements.queue_paused = authority.is_queue_drain_paused(address).await;
            *record = store
                .refresh_chat_handoff_requirements(
                    address.chat_id().as_str(),
                    &record.handoff_id,
                    &serde_json::json!(requirements),
                )
                .await
                .map_err(|e| e.to_string())?;
        }
        let configuration = store
            .load_chat_header(address.chat_id().as_str())
            .await
            .map_err(|e| e.to_string())?
            .and_then(|header| header.extra.get("hook_configuration").cloned())
            .ok_or("Chat configuration unavailable")?;
        let response = match self
            .handoff_rpc(
                address,
                &requirements.selection.device_id,
                ChatHandoffAction::Prepare {
                    record: Box::new(record.clone()),
                    configuration: Box::new(configuration.clone()),
                },
            )
            .await
        {
            Ok(response) => response,
            Err(error) => match self
                .handoff_rpc(
                    address,
                    &requirements.selection.device_id,
                    ChatHandoffAction::Status {
                        handoff_id: record.handoff_id.clone(),
                    },
                )
                .await
            {
                Ok(response)
                    if response.record.phase == ChatHandoffPhase::Prepared
                        && response.candidate.is_some() =>
                {
                    response
                }
                _ => return Err(error),
            },
        };
        // The target resolves the candidate this intent left open once it knows
        // its own directory, so compare the frozen facts without it.
        let mut prepared = response.record.clone();
        prepared.candidate_configuration = record.candidate_configuration.clone();
        if prepared != *record {
            return Err("Target prepared different Chat handoff facts".into());
        }
        let candidate = response
            .candidate
            .ok_or("Target did not prepare the Chat configuration")?;
        let mut expected: ChatHookConfiguration = decode(&configuration)?;
        let actual: ChatHookConfiguration = decode(&candidate)?;
        let directory = actual
            .capability_configuration
            .as_ref()
            .and_then(|cap| cap.extra.get(rua_session::EXECUTION_DIRECTORY_KEY))
            .ok_or("Target execution directory missing")?;
        let directory: rua_session::ChatExecutionDirectory = decode(directory)?;
        if directory.device_id != requirements.selection.device_id
            || directory.managed != requirements.selection.path.is_none()
        {
            return Err("Target prepared a different execution location".into());
        }
        expected
            .capability_configuration
            .as_mut()
            .ok_or("Chat capabilities unavailable")?
            .extra
            .insert(
                rua_session::EXECUTION_DIRECTORY_KEY.into(),
                serde_json::json!(directory),
            );
        if serde_json::json!(expected) != candidate {
            return Err("Handoff must preserve the existing Chat configuration".into());
        }
        Ok(candidate)
    }

    fn validate_handoff_participant(
        &self,
        address: &ChatAddress,
        source: &str,
        record: &ChatHandoffRecord,
        id: &str,
    ) -> Result<(), String> {
        let old: ConversationAuthorityPlacement = decode(&record.old_placement)?;
        let new: ConversationAuthorityPlacement = decode(&record.new_placement)?;
        let local =
            current_sync_device_id(&self.sync_device).ok_or("Device identity unavailable")?;
        if record.chat_id != address.chat_id().as_str()
            || record.handoff_id != id
            || source != old.device_id
            || local != new.device_id
            || new.authority_epoch <= old.authority_epoch
        {
            return Err("Handoff does not match its current participants and epoch".into());
        }
        Ok(())
    }

    async fn prepare_handoff_target(
        &self,
        address: &ChatAddress,
        source: &str,
        record: ChatHandoffRecord,
        configuration: Value,
    ) -> Result<ChatHandoffResponse, String> {
        self.validate_handoff_participant(address, source, &record, &record.handoff_id)?;
        if record.phase != ChatHandoffPhase::Prepared {
            return Err("Handoff preparation requires Prepared state".into());
        }
        let requirements: Requirements = decode(&record.requirements)?;
        let old: ConversationAuthorityPlacement = decode(&record.old_placement)?;
        let new: ConversationAuthorityPlacement = decode(&record.new_placement)?;
        if requirements.manifest.address != *address
            || requirements.manifest.hash().map_err(|e| e.to_string())? != new.base_manifest_hash
            || requirements.selection.device_id != new.device_id
        {
            return Err("Invalid handoff history or execution location".into());
        }
        let manager = crate::services::device_session_access::current_session_manager(
            self.remote_input_delivery.device_network(),
        )
        .await?;
        ensure_conversation_history_ready_via_device_session(ConversationHistoryGapRequest {
            data_dir: self.session_sync.rua_root(),
            agent_id: &requirements.agent_id,
            chat_id: address.chat_id().as_str(),
            source_device_id: source,
            target_device_id: &new.device_id,
            expected_authority_epoch: old.authority_epoch,
            requirement: &requirements.manifest,
            session_manager: Some(&manager),
        })
        .await?;
        self.sync_handoff_state(source).await?;
        let _project = self.project_work_admission(address).await?;
        // Bringing the selected Agent up is slow and publishes Agent state; it must
        // happen before this Chat's writer gate is taken, never while holding it.
        self.registry
            .get_or_create(&requirements.agent_id)
            .await
            .map_err(|error| error.to_string())?;
        let authority = self
            .conversation_authority
            .as_ref()
            .ok_or("Conversation Authority unavailable")?;
        let _gate = authority.transition_gate(address).lock_owned().await;
        self.check_handoff_idle(address).await?;
        let header = self.chat_header(address).await?;
        if parse_conversation_authority_placement(&Value::Object(header.extra))
            .map_err(|e| e.to_string())?
            != old
        {
            return Err("Target has a different Chat authority".into());
        }
        let store = self.handoff_store(address);
        let scope = ChatStore::resolve(self.session_sync.rua_root(), address.clone());
        let persisted = self
            .load_chat_configuration_locked(address, &requirements.agent_id, scope.root())
            .await?
            .ok_or("Target Chat configuration unavailable")?;
        if serde_json::json!(persisted) != configuration {
            return Err("Target Chat authorization snapshot is not synchronized".into());
        }
        // The target may take over only once the Chat permission rules the Authority
        // published are actually applied here, so the first Turn reuses the same
        // grants. Device-local Agent/root security is re-evaluated on this Device.
        if !handoff_settings::chat_rules_ready(
            self.session_sync.rua_root(),
            address,
            &requirements.settings,
            requirements.settings_published,
        )
        .map_err(|e| e.to_string())?
        {
            return Err("Target Chat permissions have not been applied".into());
        }
        if task_ref_identities(&store, address.chat_id().as_str())? != requirements.task_refs {
            return Err("Target Task references are not synchronized".into());
        }
        let history = requirements
            .history
            .as_ref()
            .ok_or("Handoff history proof missing")?;
        if !handoff_history::capture(scope.root(), address.chat_id().as_str())
            .await?
            .contains(history)
        {
            return Err("Target Chat history has not been applied".into());
        }
        self.validate_authority_adoption_model(&requirements.agent_id, address.chat_id().as_str())
            .await?;
        let directory = crate::services::chat_context::select_execution_directory(
            self.session_sync.rua_root(),
            address,
            &new.device_id,
            requirements.selection.path.as_deref(),
        )
        .await?;
        crate::services::chat_context::create_managed_directory(&directory).await?;
        crate::services::execution_directory::validate(
            &directory.path,
            self.session_sync.rua_root(),
        )
        .await?;
        let mut candidate: ChatHookConfiguration = decode(&configuration)?;
        candidate
            .capability_configuration
            .as_mut()
            .ok_or("Chat capabilities unavailable")?
            .extra
            .insert(
                rua_session::EXECUTION_DIRECTORY_KEY.into(),
                serde_json::json!(directory),
            );
        candidate.validate()?;
        if let Some(previous) = store
            .load_chat_handoff(address.chat_id().as_str())
            .await
            .map_err(|e| e.to_string())?
        {
            if previous.handoff_id == record.handoff_id
                && previous.phase == ChatHandoffPhase::Prepared
                && previous.old_placement == record.old_placement
                && previous.new_placement == record.new_placement
                && previous.requirements != record.requirements
            {
                store
                    .refresh_chat_handoff_requirements(
                        address.chat_id().as_str(),
                        &record.handoff_id,
                        &record.requirements,
                    )
                    .await
                    .map_err(|error| error.to_string())?;
            }
            if previous.handoff_id != record.handoff_id
                && previous.phase == ChatHandoffPhase::Prepared
                && previous.old_placement == record.old_placement
            {
                store
                    .abort_chat_handoff(address.chat_id().as_str(), &previous.handoff_id)
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }
        let record = store
            .prepare_chat_handoff(&record, Some(&serde_json::json!(candidate)))
            .await
            .map_err(|e| e.to_string())?;
        self.handoff_response(address, record).await
    }

    async fn activate_handoff_target(
        &self,
        address: &ChatAddress,
        source: &str,
        id: &str,
    ) -> Result<ChatHandoffResponse, String> {
        let store = self.handoff_store(address);
        let authority = self
            .conversation_authority
            .as_ref()
            .ok_or("Conversation Authority unavailable")?;
        let _gate = authority.transition_gate(address).lock_owned().await;
        let record = store
            .load_chat_handoff(address.chat_id().as_str())
            .await
            .map_err(|e| e.to_string())?
            .ok_or("Handoff unavailable")?;
        self.validate_handoff_participant(address, source, &record, id)?;
        if record.phase == ChatHandoffPhase::Prepared {
            // This authenticated message is sent only after A has durably
            // released. Persist the same commit intent before changing either file.
            store
                .release_chat_handoff(address.chat_id().as_str(), id)
                .map_err(|e| e.to_string())?;
        }
        if matches!(
            record.phase,
            ChatHandoffPhase::Prepared | ChatHandoffPhase::Released
        ) {
            let requirements: Requirements = decode(&record.requirements)?;
            let history = requirements
                .history
                .ok_or("Handoff history proof missing")?;
            store
                .save_collaboration_wake_state(address.chat_id().as_str(), &history.wake_state)
                .await
                .map_err(|error| error.to_string())?;
            authority
                .restore_handoff_queue_pause_locked(address, requirements.queue_paused)
                .await;
        }
        let record = store
            .activate_chat_handoff(address.chat_id().as_str(), id)
            .await
            .map_err(|e| e.to_string())?;
        self.repair_chat_projection(address).await?;
        self.publish_handoff_installed(address, &record).await?;
        self.handoff_response(address, record).await
    }

    async fn publish_handoff_installed(
        &self,
        address: &ChatAddress,
        record: &ChatHandoffRecord,
    ) -> Result<(), String> {
        let scope = ChatStore::resolve(self.session_sync.rua_root(), address.clone());
        let value = record
            .candidate_configuration
            .clone()
            .ok_or("Handoff configuration missing")?;
        let candidate: ChatHookConfiguration = decode(&value)?;
        super::configuration_update::publish_handoff_configuration_locked(
            self,
            address,
            scope.root(),
            &candidate,
        )
        .await
    }

    async fn handoff_response(
        &self,
        address: &ChatAddress,
        record: ChatHandoffRecord,
    ) -> Result<ChatHandoffResponse, String> {
        let candidate = record.candidate_configuration.clone();
        let scope = ChatStore::resolve(self.session_sync.rua_root(), address.clone());
        let chat = if record.phase == ChatHandoffPhase::Activated {
            load_chat_record(scope.root(), address.chat_id().as_str())
                .await
                .map_err(|e| e.to_string())?
        } else {
            None
        };
        Ok(ChatHandoffResponse {
            record,
            candidate,
            chat,
            extra: BTreeMap::new(),
        })
    }
}
