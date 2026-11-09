use std::path::PathBuf;

use rua_core::{ChatAddress, Result, RuaError};
use serde::{Deserialize, Serialize};

use crate::WorkspaceSessionStore;

pub const COLLABORATION_META_KEY: &str = "collaboration";
pub const COLLABORATION_REQUEST_KEY: &str = "collaboration_request_id";
const COLLABORATION_WAKE_STATE_FILE: &str = "collaboration_wake_state.json";
const COLLABORATION_TARGETS_FILE: &str = "collaboration_targets.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollaborationWakeState {
    #[serde(default)]
    pub suppressed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_at: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CollaborationTargets {
    #[serde(default)]
    pub targets: Vec<ChatAddress>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CollaborationDescriptor {
    pub source: ChatAddress,
    #[serde(default)]
    pub executor: CollaborationExecutor,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub executor_agent_id: String,
    pub authority_device_id: String,
    pub working_dir: PathBuf,
    pub creation_id: String,
    #[serde(default)]
    pub configuration: CollaborationConfigurationState,
    #[serde(default, flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CollaborationExecutor {
    #[default]
    Agent,
    Acp {
        runner: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollaborationConfigurationState {
    #[serde(default)]
    pub desired_source_epoch: String,
    #[serde(default)]
    pub applied_source_epoch: String,
    #[serde(default)]
    pub status: CollaborationConfigurationStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollaborationConfigurationView {
    pub source: ChatAddress,
    pub desired_source_epoch: String,
    pub applied_source_epoch: String,
    pub status: CollaborationConfigurationStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl CollaborationDescriptor {
    pub fn configuration_view(&self) -> CollaborationConfigurationView {
        CollaborationConfigurationView {
            source: self.source.clone(),
            desired_source_epoch: self.configuration.desired_source_epoch.clone(),
            applied_source_epoch: self.configuration.applied_source_epoch.clone(),
            status: self.configuration.status.clone(),
            error: self.configuration.error.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationConfigurationStatus {
    #[default]
    Ready,
    Applying,
    Failed,
}

impl CollaborationConfigurationState {
    pub fn ready(source_epoch: impl Into<String>) -> Self {
        let source_epoch = source_epoch.into();
        Self {
            desired_source_epoch: source_epoch.clone(),
            applied_source_epoch: source_epoch,
            status: CollaborationConfigurationStatus::Ready,
            error: None,
        }
    }

    pub fn is_ready(&self) -> bool {
        self.status == CollaborationConfigurationStatus::Ready
            && !self.desired_source_epoch.is_empty()
            && self.desired_source_epoch == self.applied_source_epoch
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CollaborationRequest {
    pub request_id: String,
    pub source: ChatAddress,
    pub target: ChatAddress,
    pub origin_turn_id: String,
    pub origin_tool_call_id: String,
    pub input_digest: String,
    pub created_at: String,
    pub input_id: Option<String>,
    pub turn_id: Option<String>,
    pub outcome: Option<CollaborationOutcome>,
    #[serde(default)]
    pub delivery: CollaborationDeliveryState,
    #[serde(default, flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CollaborationDeliveryState {
    #[default]
    Undecided,
    Direct,
    Deferred,
    Automatic {
        observation_id: String,
    },
    Read {
        read_id: String,
    },
    DeliveryFailed {
        observation_id: String,
        error: String,
    },
    Delivered {
        observation_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_turn_id: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollaborationDeliveryClaim {
    Direct,
    Deferred,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollaborationDeferredClaim {
    Automatic { observation_id: String },
    Read { read_id: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CollaborationOutcome {
    pub result_id: String,
    pub status: String,
    pub output: String,
    pub partial: bool,
    #[serde(default, flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollaborationResultCursor {
    pub request_id: String,
    pub result_id: String,
    pub offset: usize,
    #[serde(default, flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

pub fn collaboration_result_page(
    request_id: &str,
    result_id: &str,
    status: &str,
    output: &str,
    partial: bool,
    offset: usize,
) -> std::result::Result<serde_json::Value, String> {
    if offset > output.len() || !output.is_char_boundary(offset) {
        return Err("Result cursor is not a valid UTF-8 byte boundary".into());
    }
    let mut end = offset.saturating_add(8000).min(output.len());
    while !output.is_char_boundary(end) {
        end -= 1;
    }
    let mut page = serde_json::json!({
        "result_id": result_id, "status": status, "output": &output[offset..end],
        "partial": partial, "has_more": end < output.len(),
    });
    if end < output.len() {
        page["next"] =
            serde_json::json!({"request_id": request_id, "result_id": result_id, "offset": end});
    }
    Ok(page)
}

impl WorkspaceSessionStore {
    fn collaboration_targets_path(&self, chat_id: &str) -> PathBuf {
        self.chat_dir(chat_id).join(COLLABORATION_TARGETS_FILE)
    }

    pub async fn load_collaboration_targets(&self, chat_id: &str) -> Result<Vec<ChatAddress>> {
        match tokio::fs::read(self.collaboration_targets_path(chat_id)).await {
            Ok(bytes) => {
                let mut targets: CollaborationTargets =
                    serde_json::from_slice(&bytes).map_err(|error| RuaError::Validation {
                        message: format!("Invalid collaboration targets: {error}"),
                    })?;
                targets.targets.sort_by_key(|target| target.to_string());
                targets.targets.dedup();
                Ok(targets.targets)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(RuaError::Io {
                message: error.to_string(),
            }),
        }
    }

    pub async fn add_collaboration_target(
        &self,
        source_chat_id: &str,
        target: ChatAddress,
    ) -> Result<Vec<ChatAddress>> {
        let path = self.collaboration_targets_path(source_chat_id);
        let _guard = self.lock_chat_storage(source_chat_id).await;
        let mut targets = match tokio::fs::read(&path).await {
            Ok(bytes) => {
                serde_json::from_slice::<CollaborationTargets>(&bytes)
                    .map_err(|error| RuaError::Validation {
                        message: format!("Invalid collaboration targets: {error}"),
                    })?
                    .targets
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => {
                return Err(RuaError::Io {
                    message: error.to_string(),
                })
            }
        };
        targets.push(target);
        targets.sort_by_key(|target| target.to_string());
        targets.dedup();
        let value = CollaborationTargets {
            targets: targets.clone(),
        };
        tokio::task::spawn_blocking(move || {
            rua_config::atomic_write_json(&path, &serde_json::to_value(value)?)
        })
        .await
        .map_err(|error| RuaError::Runtime {
            message: error.to_string(),
        })?
        .map_err(|error| RuaError::Io {
            message: error.to_string(),
        })?;
        Ok(targets)
    }

    fn collaboration_wake_state_path(&self, chat_id: &str) -> PathBuf {
        self.chat_dir(chat_id).join(COLLABORATION_WAKE_STATE_FILE)
    }

    pub async fn load_collaboration_wake_state(
        &self,
        chat_id: &str,
    ) -> Result<CollaborationWakeState> {
        match tokio::fs::read(self.collaboration_wake_state_path(chat_id)).await {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| RuaError::Validation {
                message: format!("Invalid collaboration wake state: {error}"),
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(CollaborationWakeState::default())
            }
            Err(error) => Err(RuaError::Io {
                message: error.to_string(),
            }),
        }
    }

    pub async fn set_collaboration_wake_suppressed(
        &self,
        chat_id: &str,
        suppressed: bool,
    ) -> Result<CollaborationWakeState> {
        let path = self.collaboration_wake_state_path(chat_id);
        let _guard = self.lock_chat_storage(chat_id).await;
        let state = CollaborationWakeState {
            suppressed,
            stopped_at: suppressed.then(|| chrono::Utc::now().to_rfc3339()),
        };
        let value = state.clone();
        tokio::task::spawn_blocking(move || {
            rua_config::atomic_write_json(&path, &serde_json::to_value(value)?)
        })
        .await
        .map_err(|error| RuaError::Runtime {
            message: error.to_string(),
        })?
        .map_err(|error| RuaError::Io {
            message: error.to_string(),
        })?;
        Ok(state)
    }

    fn collaboration_request_path(&self, chat_id: &str, request_id: &str) -> Result<PathBuf> {
        uuid::Uuid::parse_str(request_id).map_err(|_| RuaError::Validation {
            message: "Invalid collaboration request ID".to_string(),
        })?;
        Ok(self
            .chat_dir(chat_id)
            .join("collaboration_requests")
            .join(format!("{request_id}.json")))
    }

    pub async fn save_collaboration_request(&self, record: &CollaborationRequest) -> Result<()> {
        let path =
            self.collaboration_request_path(record.target.chat_id().as_str(), &record.request_id)?;
        let _guard = self
            .lock_chat_storage(record.target.chat_id().as_str())
            .await;
        let mut value = record.clone();
        if let Some(existing) = self
            .load_collaboration_request(record.target.chat_id().as_str(), &record.request_id)
            .await?
        {
            if existing.source != record.source
                || existing.target != record.target
                || existing.input_digest != record.input_digest
                || existing.origin_turn_id != record.origin_turn_id
                || existing.origin_tool_call_id != record.origin_tool_call_id
            {
                return Err(RuaError::Validation {
                    message: "Collaboration request identity conflict".into(),
                });
            }
            if let (Some(old), Some(new)) = (&existing.outcome, &record.outcome) {
                if old != new {
                    return Err(RuaError::Validation {
                        message: "Collaboration outcome is immutable".into(),
                    });
                }
            }
            if let (Some(old), Some(new)) = (&existing.turn_id, &record.turn_id) {
                if old != new {
                    return Err(RuaError::Validation {
                        message: "Collaboration request cannot change its Turn".into(),
                    });
                }
            }
            value.outcome = existing.outcome.or(value.outcome);
            value.turn_id = existing.turn_id.or(value.turn_id);
            value.input_id = existing.input_id.or(value.input_id);
            value.delivery = merge_delivery_state(existing.delivery, value.delivery)?;
        }
        tokio::task::spawn_blocking(move || {
            rua_config::atomic_write_json(&path, &serde_json::to_value(value)?)
        })
        .await
        .map_err(|error| RuaError::Runtime {
            message: error.to_string(),
        })?
        .map_err(|error| RuaError::Io {
            message: error.to_string(),
        })
    }

    pub async fn load_collaboration_request(
        &self,
        chat_id: &str,
        request_id: &str,
    ) -> Result<Option<CollaborationRequest>> {
        let path = self.collaboration_request_path(chat_id, request_id)?;
        match tokio::fs::read(&path).await {
            Ok(bytes) => {
                serde_json::from_slice(&bytes)
                    .map(Some)
                    .map_err(|error| RuaError::Validation {
                        message: error.to_string(),
                    })
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(RuaError::Io {
                message: error.to_string(),
            }),
        }
    }

    pub async fn list_collaboration_requests(
        &self,
        chat_id: &str,
    ) -> Result<Vec<CollaborationRequest>> {
        let dir = self.chat_dir(chat_id).join("collaboration_requests");
        let mut entries = match tokio::fs::read_dir(dir).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(RuaError::Io {
                    message: error.to_string(),
                })
            }
        };
        let mut records = Vec::new();
        while let Some(entry) = entries.next_entry().await.map_err(|error| RuaError::Io {
            message: error.to_string(),
        })? {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let bytes = tokio::fs::read(&path).await.map_err(|error| RuaError::Io {
                message: error.to_string(),
            })?;
            let record: CollaborationRequest =
                serde_json::from_slice(&bytes).map_err(|error| RuaError::Validation {
                    message: format!("Invalid collaboration request {}: {error}", path.display()),
                })?;
            if record.target.chat_id().as_str() != chat_id {
                return Err(RuaError::Validation {
                    message: "Collaboration request target does not match its Chat storage".into(),
                });
            }
            records.push(record);
        }
        records.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.request_id.cmp(&right.request_id))
        });
        Ok(records)
    }

    pub async fn claim_collaboration_delivery(
        &self,
        chat_id: &str,
        request_id: &str,
        claim: CollaborationDeliveryClaim,
    ) -> Result<CollaborationRequest> {
        let path = self.collaboration_request_path(chat_id, request_id)?;
        let _guard = self.lock_chat_storage(chat_id).await;
        let bytes = tokio::fs::read(&path).await.map_err(|error| RuaError::Io {
            message: error.to_string(),
        })?;
        let mut record: CollaborationRequest =
            serde_json::from_slice(&bytes).map_err(|error| RuaError::Validation {
                message: error.to_string(),
            })?;
        if record.delivery == CollaborationDeliveryState::Undecided {
            record.delivery = match claim {
                CollaborationDeliveryClaim::Direct => CollaborationDeliveryState::Direct,
                CollaborationDeliveryClaim::Deferred => CollaborationDeliveryState::Deferred,
            };
            let value = record.clone();
            tokio::task::spawn_blocking(move || {
                rua_config::atomic_write_json(&path, &serde_json::to_value(value)?)
            })
            .await
            .map_err(|error| RuaError::Runtime {
                message: error.to_string(),
            })?
            .map_err(|error| RuaError::Io {
                message: error.to_string(),
            })?;
        }
        Ok(record)
    }

    pub async fn mark_collaboration_delivered(
        &self,
        chat_id: &str,
        request_id: &str,
        observation_id: String,
        source_turn_id: Option<String>,
    ) -> Result<CollaborationRequest> {
        let path = self.collaboration_request_path(chat_id, request_id)?;
        let _guard = self.lock_chat_storage(chat_id).await;
        let bytes = tokio::fs::read(&path).await.map_err(|error| RuaError::Io {
            message: error.to_string(),
        })?;
        let mut record: CollaborationRequest =
            serde_json::from_slice(&bytes).map_err(|error| RuaError::Validation {
                message: error.to_string(),
            })?;
        match &record.delivery {
            CollaborationDeliveryState::Automatic {
                observation_id: claimed,
            } if claimed == &observation_id => {
                record.delivery = CollaborationDeliveryState::Delivered {
                    observation_id,
                    source_turn_id,
                };
            }
            CollaborationDeliveryState::Delivered {
                observation_id: existing,
                ..
            } if existing == &observation_id => return Ok(record),
            CollaborationDeliveryState::Undecided
            | CollaborationDeliveryState::Direct
            | CollaborationDeliveryState::Deferred
            | CollaborationDeliveryState::Read { .. }
            | CollaborationDeliveryState::DeliveryFailed { .. }
            | CollaborationDeliveryState::Automatic { .. } => {
                return Err(RuaError::Validation {
                    message: "Collaboration result is not eligible for automatic delivery".into(),
                });
            }
            CollaborationDeliveryState::Delivered { .. } => {
                return Err(RuaError::Validation {
                    message: "Collaboration result delivery identity conflict".into(),
                });
            }
        }
        let value = record.clone();
        tokio::task::spawn_blocking(move || {
            rua_config::atomic_write_json(&path, &serde_json::to_value(value)?)
        })
        .await
        .map_err(|error| RuaError::Runtime {
            message: error.to_string(),
        })?
        .map_err(|error| RuaError::Io {
            message: error.to_string(),
        })?;
        Ok(record)
    }

    pub async fn claim_deferred_collaboration_delivery(
        &self,
        chat_id: &str,
        request_id: &str,
        claim: CollaborationDeferredClaim,
    ) -> Result<CollaborationRequest> {
        let path = self.collaboration_request_path(chat_id, request_id)?;
        let _guard = self.lock_chat_storage(chat_id).await;
        let bytes = tokio::fs::read(&path).await.map_err(|error| RuaError::Io {
            message: error.to_string(),
        })?;
        let mut record: CollaborationRequest =
            serde_json::from_slice(&bytes).map_err(|error| RuaError::Validation {
                message: error.to_string(),
            })?;
        if record.delivery == CollaborationDeliveryState::Deferred {
            record.delivery = match claim {
                CollaborationDeferredClaim::Automatic { observation_id } => {
                    CollaborationDeliveryState::Automatic { observation_id }
                }
                CollaborationDeferredClaim::Read { read_id } => {
                    CollaborationDeliveryState::Read { read_id }
                }
            };
            let value = record.clone();
            tokio::task::spawn_blocking(move || {
                rua_config::atomic_write_json(&path, &serde_json::to_value(value)?)
            })
            .await
            .map_err(|error| RuaError::Runtime {
                message: error.to_string(),
            })?
            .map_err(|error| RuaError::Io {
                message: error.to_string(),
            })?;
        }
        Ok(record)
    }

    pub async fn mark_collaboration_delivery_failed(
        &self,
        chat_id: &str,
        request_id: &str,
        observation_id: String,
        error: String,
    ) -> Result<CollaborationRequest> {
        let path = self.collaboration_request_path(chat_id, request_id)?;
        let _guard = self.lock_chat_storage(chat_id).await;
        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|read_error| RuaError::Io {
                message: read_error.to_string(),
            })?;
        let mut record: CollaborationRequest =
            serde_json::from_slice(&bytes).map_err(|parse_error| RuaError::Validation {
                message: parse_error.to_string(),
            })?;
        match &record.delivery {
            CollaborationDeliveryState::Automatic {
                observation_id: claimed,
            } if claimed == &observation_id => {
                record.delivery = CollaborationDeliveryState::DeliveryFailed {
                    observation_id,
                    error,
                };
            }
            CollaborationDeliveryState::DeliveryFailed {
                observation_id: existing,
                ..
            } if existing == &observation_id => return Ok(record),
            _ => {
                return Err(RuaError::Validation {
                    message: "Collaboration result is not eligible for delivery failure".into(),
                })
            }
        }
        let value = record.clone();
        tokio::task::spawn_blocking(move || {
            rua_config::atomic_write_json(&path, &serde_json::to_value(value)?)
        })
        .await
        .map_err(|join_error| RuaError::Runtime {
            message: join_error.to_string(),
        })?
        .map_err(|write_error| RuaError::Io {
            message: write_error.to_string(),
        })?;
        Ok(record)
    }

    pub async fn retry_failed_collaboration_delivery(
        &self,
        chat_id: &str,
        request_id: &str,
    ) -> Result<CollaborationRequest> {
        let path = self.collaboration_request_path(chat_id, request_id)?;
        let _guard = self.lock_chat_storage(chat_id).await;
        let bytes = tokio::fs::read(&path).await.map_err(|error| RuaError::Io {
            message: error.to_string(),
        })?;
        let mut record: CollaborationRequest =
            serde_json::from_slice(&bytes).map_err(|error| RuaError::Validation {
                message: error.to_string(),
            })?;
        if let CollaborationDeliveryState::DeliveryFailed { observation_id, .. } = &record.delivery
        {
            record.delivery = CollaborationDeliveryState::Automatic {
                observation_id: observation_id.clone(),
            };
            let value = record.clone();
            tokio::task::spawn_blocking(move || {
                rua_config::atomic_write_json(&path, &serde_json::to_value(value)?)
            })
            .await
            .map_err(|error| RuaError::Runtime {
                message: error.to_string(),
            })?
            .map_err(|error| RuaError::Io {
                message: error.to_string(),
            })?;
        }
        Ok(record)
    }
}

fn merge_delivery_state(
    existing: CollaborationDeliveryState,
    incoming: CollaborationDeliveryState,
) -> Result<CollaborationDeliveryState> {
    use CollaborationDeliveryState::{
        Automatic, Deferred, Delivered, DeliveryFailed, Direct, Read, Undecided,
    };
    match (existing, incoming) {
        (Undecided, incoming) => Ok(incoming),
        (existing, Undecided) => Ok(existing),
        (Direct, Direct) => Ok(Direct),
        (Deferred, Deferred) => Ok(Deferred),
        (Deferred, automatic @ Automatic { .. }) => Ok(automatic),
        (Deferred, read @ Read { .. }) => Ok(read),
        (existing @ Automatic { .. }, Deferred) => Ok(existing),
        (existing @ Read { .. }, Deferred) => Ok(existing),
        (Automatic { observation_id }, delivered @ Delivered { .. })
            if matches!(
                &delivered,
                Delivered {
                    observation_id: delivered_id,
                    ..
                } if &observation_id == delivered_id
            ) =>
        {
            Ok(delivered)
        }
        (existing @ Delivered { .. }, Deferred | Automatic { .. }) => Ok(existing),
        (existing @ Automatic { .. }, incoming @ Automatic { .. }) if existing == incoming => {
            Ok(existing)
        }
        (Automatic { observation_id }, failed @ DeliveryFailed { .. })
            if matches!(
                &failed,
                DeliveryFailed {
                    observation_id: failed_id,
                    ..
                } if &observation_id == failed_id
            ) =>
        {
            Ok(failed)
        }
        (existing @ DeliveryFailed { .. }, Automatic { .. }) => Ok(existing),
        (existing @ DeliveryFailed { .. }, incoming @ DeliveryFailed { .. })
            if existing == incoming =>
        {
            Ok(existing)
        }
        (existing @ Read { .. }, incoming @ Read { .. }) if existing == incoming => Ok(existing),
        (existing @ Delivered { .. }, incoming @ Delivered { .. }) if existing == incoming => {
            Ok(existing)
        }
        _ => Err(RuaError::Validation {
            message: "Collaboration result delivery state conflict".into(),
        }),
    }
}
