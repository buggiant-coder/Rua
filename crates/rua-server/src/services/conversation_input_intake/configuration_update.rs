//! Authority-serialized Chat configuration updates. Persistence precedes runtime
//! adoption, collaboration propagation, Control View publication and Sync. The
//! guard covers preview validation as well as commit, so Turn claim cannot see
//! an intermediate configuration. Resource capture remains side-effect free.

use std::path::Path;

use rua_config::ChatRunningConfig;
use rua_core::{ChatAddress, RuaError};
use rua_session::WorkspaceSessionStore;
use serde_json::Value;
use tokio::sync::OwnedMutexGuard;

use super::ConversationInputIntake;
use crate::services::conversation_authority_placement::CHAT_MODEL_META_KEY;
use crate::services::conversation_authority_target::load_chat_control_meta;
use crate::services::product_hooks::{ChatHookConfiguration, CHAT_HOOK_CONFIGURATION_META_KEY};

pub(super) struct ChatConfigurationUpdate<'a> {
    intake: &'a ConversationInputIntake,
    address: &'a ChatAddress,
    scope_root: &'a Path,
    current: Option<ChatHookConfiguration>,
    _transition: Option<OwnedMutexGuard<()>>,
}

enum Publication<'a> {
    Model(&'a Value),
    RunningConfig(&'a ChatRunningConfig),
    Complete(&'a ChatHookConfiguration),
}

impl<'a> ChatConfigurationUpdate<'a> {
    pub(super) async fn begin(
        intake: &'a ConversationInputIntake,
        address: &'a ChatAddress,
        agent_id: &str,
        scope_root: &'a Path,
    ) -> Result<Self, String> {
        let transition = match intake.conversation_authority.as_ref() {
            Some(authority) => Some(authority.transition_gate(address).lock_owned().await),
            None => None,
        };
        let current = intake
            .load_chat_configuration_locked(address, agent_id, scope_root)
            .await?;
        Ok(Self {
            intake,
            address,
            scope_root,
            current,
            _transition: transition,
        })
    }

    pub(super) fn current(&self) -> Option<&ChatHookConfiguration> {
        self.current.as_ref()
    }

    pub(super) async fn update_model(&self, model: Value) -> Result<Value, String> {
        let chat = rua_chat::ChatService::for_resolved_scope(self.scope_root, self.address.clone())
            .update_setting(self.address, rua_chat::ChatSetting::Model(model))
            .await
            .map_err(|error| error.to_string())?;
        let model = chat
            .model()
            .cloned()
            .ok_or("updated Chat has no selected model")?;
        self.publish(Publication::Model(&model))
            .await
            .map_err(|error| error.to_string())?;
        Ok(model)
    }

    pub(super) async fn update_running_config(
        &self,
        running: ChatRunningConfig,
    ) -> rua_core::Result<ChatRunningConfig> {
        if !WorkspaceSessionStore::new(self.scope_root)
            .update_chat_running_config(self.address.chat_id().as_str(), running.clone())
            .await?
        {
            return Err(RuaError::NotFound {
                resource: "chat".into(),
                id: self.address.chat_id().to_string(),
            });
        }
        self.publish(Publication::RunningConfig(&running)).await?;
        Ok(running)
    }

    pub(super) async fn replace(&self, candidate: &ChatHookConfiguration) -> Result<(), String> {
        let current = self.current().ok_or("Chat not found")?;
        // A retry after the durable commit resumes propagation/publication without
        // creating a second epoch or re-adopting the already applied candidate.
        if current.epoch != candidate.epoch {
            WorkspaceSessionStore::new(self.scope_root)
                .replace_chat_configuration(
                    self.address.chat_id().as_str(),
                    &current.epoch,
                    serde_json::to_value(candidate).map_err(|error| error.to_string())?,
                )
                .await
                .map_err(|error| error.to_string())?;
        }
        self.publish(Publication::Complete(candidate))
            .await
            .map_err(|error| error.to_string())
    }

    async fn publish(&self, publication: Publication<'_>) -> rua_core::Result<()> {
        if let Some(authority) = self.intake.conversation_authority.as_ref() {
            if let Some(hooks) = self.intake.product_hooks.as_ref() {
                let configuration = match &publication {
                    Publication::Complete(candidate) => self
                        .current()
                        .filter(|current| current.epoch != candidate.epoch)
                        .map(|_| (*candidate).clone()),
                    _ => WorkspaceSessionStore::new(self.scope_root)
                        .load_chat_header(self.address.chat_id().as_str())
                        .await?
                        .and_then(|header| {
                            header.extra.get(CHAT_HOOK_CONFIGURATION_META_KEY).cloned()
                        })
                        .map(serde_json::from_value)
                        .transpose()
                        .map_err(|error| RuaError::Runtime {
                            message: format!("invalid persisted Chat configuration: {error}"),
                        })?,
                };
                if let Some(configuration) = configuration {
                    hooks
                        .adopt_chat_configuration(self.address, configuration)
                        .await;
                }
            }
            if let Publication::Complete(candidate) = &publication {
                if let Some(service) = self
                    .intake
                    .collaboration
                    .as_ref()
                    .and_then(|binding| binding.get())
                    .and_then(std::sync::Weak::upgrade)
                {
                    service
                        .propagate_source_configuration(self.address.clone(), (*candidate).clone())
                        .await
                        .map_err(|message| RuaError::Runtime { message })?;
                }
            }
            let mut meta = load_chat_control_meta(self.scope_root, self.address.chat_id().as_str())
                .await
                .map_err(|message| RuaError::Runtime { message })?
                .unwrap_or_else(|| serde_json::json!({}));
            match publication {
                Publication::Model(model) => {
                    if !meta.is_object() {
                        meta = serde_json::json!({});
                    }
                    meta.as_object_mut()
                        .expect("model control metadata is an object")
                        .insert(CHAT_MODEL_META_KEY.into(), model.clone());
                    authority
                        .publish_control_snapshot(self.address, "model_updated", &meta)
                        .await;
                }
                Publication::RunningConfig(running) => {
                    let running =
                        serde_json::to_value(running).map_err(|error| RuaError::Runtime {
                            message: format!("serialize Chat Running Config: {error}"),
                        })?;
                    authority
                        .publish_running_config_snapshot(self.address, &meta, running)
                        .await;
                }
                Publication::Complete(_) => {
                    authority
                        .publish_control_snapshot(self.address, "configuration_updated", &meta)
                        .await;
                }
            }
        }
        self.intake.session_sync.spawn_chat_state_publication_for(
            self.scope_root,
            self.address,
            None,
        );
        Ok(())
    }
}
