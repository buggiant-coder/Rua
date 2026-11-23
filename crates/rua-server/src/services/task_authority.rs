use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use rua_core::ChatStore;
use rua_task::{
    TaskAttemptAuthorization, TaskAuthorityError, TaskAuthorityResolver, TaskDeviceContext,
    TaskOwner, TaskRunnerSpec, TaskSpec,
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
        let scope_store = ChatStore::resolve(&self.data_dir, owner.chat_address.clone());
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
    async fn accept_attempt(
        &self,
        spec: &TaskSpec,
        authorization: &TaskAttemptAuthorization,
        prior: Option<&TaskDeviceContext>,
    ) -> Result<TaskDeviceContext, TaskAuthorityError> {
        let (device, local_device_id) = self.local_device()?;
        let owner = &spec.owner;
        let scope = ChatStore::resolve(&self.data_dir, owner.chat_address.clone());
        let store = rua_session::WorkspaceSessionStore::new(scope.root());
        let header = store
            .load_chat_header(owner.chat_id())
            .await
            .map_err(|error| TaskAuthorityError::Unavailable(error.to_string()))?
            .ok_or_else(|| {
                TaskAuthorityError::Rejected("Task owning Chat does not exist".into())
            })?;
        if header.metadata.archived {
            return Err(TaskAuthorityError::Rejected(
                "Task owning Chat or selected Agent changed".into(),
            ));
        }
        if let Some(value) = header
            .extra
            .get(rua_session::collaboration::COLLABORATION_META_KEY)
        {
            let descriptor: rua_session::collaboration::CollaborationDescriptor =
                serde_json::from_value(value.clone()).map_err(|error| {
                    TaskAuthorityError::Rejected(format!("Invalid collaboration state: {error}"))
                })?;
            if !descriptor.configuration.is_ready() {
                return Err(TaskAuthorityError::Rejected(
                    "Collaboration configuration is not ready".into(),
                ));
            }
        }
        let configuration = header.extra.get("hook_configuration").ok_or_else(|| {
            TaskAuthorityError::Unavailable("Task Chat configuration is unavailable".into())
        })?;
        // An authorization that names a configuration epoch must still be the
        // Chat's current one. Without a claimed epoch the Attempt relies on the
        // captured directory alone, which is all a direct Task creation has.
        if authorization
            .configuration_epoch
            .as_deref()
            .is_some_and(|claimed| {
                Some(claimed)
                    != configuration
                        .get("epoch")
                        .and_then(serde_json::Value::as_str)
            })
        {
            return Err(TaskAuthorityError::Rejected(
                "Task Attempt authorization configuration is stale".into(),
            ));
        }
        let captured: rua_session::ChatExecutionDirectory = serde_json::from_value(
            configuration
                .get("capability_configuration")
                .and_then(|value| value.get(rua_session::EXECUTION_DIRECTORY_KEY))
                .cloned()
                .ok_or_else(|| {
                    TaskAuthorityError::Unavailable(
                        "Task captured execution directory is unavailable".into(),
                    )
                })?,
        )
        .map_err(|error| TaskAuthorityError::Rejected(error.to_string()))?;
        let layout = rua_core::runtime_layout::RuntimeLayout::from_current_process(None);
        let requested = if let Some(prior) = prior {
            if prior.device_id.as_deref() != Some(&local_device_id) {
                return Err(TaskAuthorityError::Rejected(
                    "Retry belongs to the original execution Device".into(),
                ));
            }
            PathBuf::from(prior.public_cwd.as_deref().ok_or_else(|| {
                TaskAuthorityError::Unavailable(
                    "Original Task execution directory is unavailable".into(),
                )
            })?)
        } else {
            let arguments = match &spec.runner {
                TaskRunnerSpec::Tool {
                    tool_name,
                    arguments,
                } if tool_name == "bash" => Some(arguments),
                _ => None,
            };
            let selected = arguments
                .and_then(|args| args["device_id"].as_str())
                .unwrap_or(&captured.device_id);
            if selected != local_device_id {
                return Err(TaskAuthorityError::Rejected(
                    "Task must be accepted on its selected execution Device".into(),
                ));
            }
            match arguments.and_then(|args| args["cwd"].as_str()) {
                Some(cwd) => {
                    if captured.device_id != local_device_id
                        && !rua_core::execution_directory::is_device_absolute_path(cwd)
                    {
                        return Err(TaskAuthorityError::Rejected(
                            "Remote Task cwd must be an absolute public path".into(),
                        ));
                    }
                    PathBuf::from(cwd)
                }
                None if captured.device_id == local_device_id => captured.path.clone(),
                None => match &layout.linux_compat {
                    Some(guest) => guest.guest_home.clone(),
                    None => dirs::home_dir().ok_or_else(|| {
                        TaskAuthorityError::Unavailable("Target home is unavailable".into())
                    })?,
                },
            }
        };
        let base =
            if prior.is_none() && !requested.is_absolute() && captured.device_id == local_device_id
            {
                captured
                    .host_path()
                    .map_err(|error| TaskAuthorityError::Unavailable(error.to_string()))?
            } else {
                self.data_dir.clone()
            };
        let fs = rua_tool::WorkspaceFs::for_execution(&base, layout)
            .map_err(|error| TaskAuthorityError::Unavailable(error.to_string()))?;
        let host = fs
            .resolve(&requested)
            .map_err(|error| TaskAuthorityError::Unavailable(error.to_string()))?;
        if !host.is_dir() {
            return Err(TaskAuthorityError::Unavailable(
                "Task execution directory is not available".into(),
            ));
        }
        // Report the directory the Chat captured verbatim when that is what was
        // accepted: resolving it again would rewrite the user's own spelling
        // (for example through a `/var` -> `/private/var` symlink) and lose the
        // fact that Rua manages the directory.
        let captured_verbatim = prior.is_none()
            && captured.device_id == local_device_id
            && requested == captured.path;
        let public_cwd = if captured_verbatim {
            captured
                .path
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| {
                    TaskAuthorityError::Unavailable("Task directory has no public path".into())
                })?
        } else {
            // An Attempt that already recorded its directory keeps that exact
            // spelling as long as it still names this directory: retrying a Task
            // must not rewrite the original path into a different form.
            let prior_cwd = prior.and_then(|prior| prior.public_cwd.clone());
            match prior_cwd {
                Some(prior_cwd)
                    if fs
                        .resolve(std::path::Path::new(&prior_cwd))
                        .is_ok_and(|prior_host| prior_host == host) =>
                {
                    prior_cwd
                }
                _ => fs
                    .public_path_from_host(&host)
                    .and_then(|path| path.to_str().map(str::to_owned))
                    .ok_or_else(|| {
                        TaskAuthorityError::Unavailable("Task directory has no public path".into())
                    })?,
            }
        };
        if prior.is_some_and(|prior| prior.public_cwd.as_deref() != Some(public_cwd.as_str())) {
            return Err(TaskAuthorityError::Rejected(
                "Original Task directory now resolves elsewhere".into(),
            ));
        }
        let placement = self.placement(owner).await?;
        let device_name = device_name_snapshot(&self.data_dir, &local_device_id).or_else(|| {
            Some(rua_sync::device_list::default_friendly_name(
                &self.host_profile,
                &device.public_key().to_bytes(),
            ))
        });
        Ok(TaskDeviceContext {
            device_id: Some(local_device_id),
            public_cwd: Some(public_cwd),
            // A fresh Attempt reports what the Chat captured; a retry reports what
            // the original Attempt recorded, so the managed flag survives Retry.
            cwd_managed: Some(
                prior
                    .and_then(|prior| prior.cwd_managed)
                    .unwrap_or(captured_verbatim && captured.managed),
            ),
            device_name,
            host_profile: Some(self.host_profile.clone()),
            authority_epoch: Some(placement.authority_epoch.to_string()),
        })
    }

    async fn is_current(
        &self,
        _owner: &TaskOwner,
        accepted: &TaskDeviceContext,
    ) -> Result<bool, TaskAuthorityError> {
        let local = current_sync_device_id(&self.sync_device).ok_or_else(|| {
            TaskAuthorityError::Unavailable("local Device identity is unavailable".into())
        })?;
        // Chat placement can change while this Device still owns the same Attempt.
        Ok(accepted.device_id.as_deref() == Some(local.as_str()))
    }

    async fn local_device_id(&self) -> Result<Option<String>, TaskAuthorityError> {
        self.local_device().map(|(_, device_id)| Some(device_id))
    }
}
