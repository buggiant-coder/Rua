use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use rua_core::ChatScopeStore;
use rua_task::{
    TaskAttemptAuthorization, TaskAuthorityError, TaskAuthorityResolver, TaskDeviceContext,
    TaskOwner,
};

use crate::services::device_context::{current_sync_device_id, device_name_snapshot};

pub struct ServerTaskAuthorityResolver {
    data_dir: PathBuf,
    host_profile: String,
    sync_device: Arc<std::sync::RwLock<Option<Arc<rua_sync::identity::DeviceIdentity>>>>,
}

impl ServerTaskAuthorityResolver {
    pub fn new(
        data_dir: PathBuf,
        host_profile: impl Into<String>,
        sync_device: Arc<std::sync::RwLock<Option<Arc<rua_sync::identity::DeviceIdentity>>>>,
    ) -> Self {
        Self {
            data_dir,
            host_profile: host_profile.into(),
            sync_device,
        }
    }

    fn local_device(
        &self,
    ) -> Result<(Arc<rua_sync::identity::DeviceIdentity>, String), TaskAuthorityError> {
        let device = self
            .sync_device
            .read()
            .map_err(|_| {
                TaskAuthorityError::Unavailable(
                    "local Device identity lock is unavailable".to_string(),
                )
            })?
            .clone()
            .ok_or_else(|| {
                TaskAuthorityError::Unavailable("local Device identity is unavailable".to_string())
            })?;
        let device_id = hex::encode(device.device_id_short());
        Ok((device, device_id))
    }

    async fn placement(
        &self,
        owner: &TaskOwner,
    ) -> Result<
        crate::services::conversation_authority_placement::ConversationAuthorityPlacement,
        TaskAuthorityError,
    > {
        let scope_store = ChatScopeStore::resolve(&self.data_dir, owner.chat_address.clone());
        crate::services::conversation_authority_target::load_optional_chat_authority_placement(
            scope_store.root(),
            owner.chat_id(),
        )
        .await
        .map_err(TaskAuthorityError::Unavailable)?
        .ok_or_else(|| {
            TaskAuthorityError::Rejected(format!(
                "owning Chat '{}' does not exist",
                owner.chat_address
            ))
        })
    }
}

#[async_trait]
impl TaskAuthorityResolver for ServerTaskAuthorityResolver {
    async fn accept(&self, owner: &TaskOwner) -> Result<TaskDeviceContext, TaskAuthorityError> {
        let (device, local_device_id) = self.local_device()?;
        let placement = self.placement(owner).await?;
        if placement.device_id != local_device_id {
            return Err(TaskAuthorityError::Rejected(format!(
                "Chat authority is Device '{}', not local Device '{}'",
                placement.device_id, local_device_id
            )));
        }
        let device_name = device_name_snapshot(&self.data_dir, &local_device_id).or_else(|| {
            Some(rua_sync::device_list::default_friendly_name(
                &self.host_profile,
                &device.public_key().to_bytes(),
            ))
        });
        Ok(TaskDeviceContext {
            device_id: Some(local_device_id),
            device_name,
            host_profile: Some(self.host_profile.clone()),
            authority_epoch: Some(placement.authority_epoch.to_string()),
        })
    }

    async fn accept_attempt(
        &self,
        owner: &TaskOwner,
        authorization: &TaskAttemptAuthorization,
    ) -> Result<TaskDeviceContext, TaskAuthorityError> {
        let scope = ChatScopeStore::resolve(&self.data_dir, owner.chat_address.clone());
        let header = rua_session::WorkspaceSessionStore::new(scope.root())
            .load_chat_header(owner.chat_id())
            .await
            .map_err(|error| TaskAuthorityError::Unavailable(error.to_string()))?;
        if let Some(header) = header {
            if let Some(value) = header
                .extra
                .get(rua_session::collaboration::COLLABORATION_META_KEY)
            {
                let descriptor: rua_session::collaboration::CollaborationDescriptor =
                    serde_json::from_value(value.clone()).map_err(|error| {
                        TaskAuthorityError::Rejected(format!(
                            "Invalid collaboration configuration state: {error}"
                        ))
                    })?;
                if !descriptor.configuration.is_ready() {
                    return Err(TaskAuthorityError::Rejected(format!(
                        "Collaboration Chat configuration is not ready for source epoch {}",
                        descriptor.configuration.desired_source_epoch
                    )));
                }
                let current_epoch = header
                    .extra
                    .get("hook_configuration")
                    .and_then(|configuration| configuration.get("epoch"))
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        TaskAuthorityError::Rejected(
                            "Collaboration Chat configuration head is unavailable".to_string(),
                        )
                    })?;
                if authorization.configuration_epoch.as_deref() != Some(current_epoch) {
                    return Err(TaskAuthorityError::Rejected(format!(
                        "Task configuration epoch is stale; current child epoch is {current_epoch}"
                    )));
                }
            }
        }
        self.accept(owner).await
    }

    async fn is_current(
        &self,
        owner: &TaskOwner,
        accepted: &TaskDeviceContext,
    ) -> Result<bool, TaskAuthorityError> {
        let local_device_id = current_sync_device_id(&self.sync_device).ok_or_else(|| {
            TaskAuthorityError::Unavailable("local Device identity is unavailable".to_string())
        })?;
        let placement = self.placement(owner).await?;
        Ok(
            accepted.device_id.as_deref() == Some(local_device_id.as_str())
                && accepted.authority_epoch.as_deref()
                    == Some(placement.authority_epoch.to_string().as_str())
                && placement.device_id == local_device_id,
        )
    }

    async fn local_device_id(&self) -> Result<Option<String>, TaskAuthorityError> {
        self.local_device().map(|(_, device_id)| Some(device_id))
    }
}
