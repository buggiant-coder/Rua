use std::path::{Path, PathBuf};
use std::sync::Arc;

use rua_config::types::tools::ToolIdentity;
use rua_core::ChatAddress;
pub use rua_security::security_module::FileReadAdmission;
use rua_security::security_module::FileReadPolicyError;
use rua_security::security_module::FileReadPolicyError::{Denied, Unavailable};
use rua_security::ToolCallContext;
use rua_tool::core::read_file::OpenedRegularFile;
use serde_json::Value;

use super::agent_manager::AgentManager;
use super::conversation_authority_placement::{
    parse_conversation_authority_placement, ConversationAuthorityEpoch,
};
use super::product_hooks::ChatHookConfiguration;

pub struct TargetChatRead<'a> {
    pub address: &'a ChatAddress,
    pub agent_id: &'a str,
    pub authority_device_id: &'a str,
    pub authority_epoch: ConversationAuthorityEpoch,
    pub configuration_epoch: &'a str,
    pub turn_id: &'a str,
    pub tool_call_id: &'a str,
    pub tool_name: &'a str,
    pub args: &'a Value,
}

/// Safe to report to another Device without disclosing local paths or policy details.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetReadRejection {
    Denied,
    Unavailable,
    FileUnavailable,
}

impl From<FileReadPolicyError> for TargetReadRejection {
    fn from(error: FileReadPolicyError) -> Self {
        match error {
            Denied(_) => Self::Denied,
            Unavailable(_) => Self::Unavailable,
        }
    }
}

#[derive(Clone)]
pub struct TargetFileReadContext {
    rua_root: PathBuf,
    local_device_id: String,
    project_catalog: Arc<rua_project::ProjectCatalog>,
    agent_service: Arc<AgentManager>,
    tool_catalog: Arc<serde_json::Map<String, Value>>,
    tool_config_store: Arc<rua_tool::ToolConfigStore>,
    security: Arc<rua_security::SecurityModule>,
}

impl TargetFileReadContext {
    pub fn new(
        rua_root: PathBuf,
        local_device_id: String,
        project_catalog: Arc<rua_project::ProjectCatalog>,
        agent_service: Arc<AgentManager>,
        tool_catalog: Arc<serde_json::Map<String, Value>>,
        tool_config_store: Arc<rua_tool::ToolConfigStore>,
        security: Arc<rua_security::SecurityModule>,
    ) -> Self {
        Self {
            rua_root,
            local_device_id,
            project_catalog,
            agent_service,
            tool_catalog,
            tool_config_store,
            security,
        }
    }

    /// Verify the local replica's authority and captured configuration before contacting the
    /// origin. A stale or unknown authority cannot consume a one-time decision at the origin.
    pub async fn check_authority(
        &self,
        request: &TargetChatRead<'_>,
    ) -> Result<(), TargetReadRejection> {
        self.requested_path(request)
            .map_err(|_| TargetReadRejection::Denied)?;
        self.checked_chat_configuration(request)
            .await
            .map(|_| ())
            .map_err(Into::into)
    }

    /// Classify the target policy from the requested path spelling without probing the file or
    /// creating an approval. Both outcomes still require independent target file-access
    /// authorization and verification of the opened file before any contents may be read.
    pub async fn check_metadata(
        &self,
        request: &TargetChatRead<'_>,
    ) -> Result<FileReadAdmission, TargetReadRejection> {
        let path = self
            .requested_path(request)
            .map_err(|_| TargetReadRejection::Denied)?;
        rua_tool::fs::WorkspaceFs::check_secret_dir(Path::new(path))
            .map_err(|_| TargetReadRejection::Denied)?;
        self.inspect_policy(request, &[path], false)
            .await
            .map_err(Into::into)
    }

    /// Inspect a file already opened under this Device's own file-access authorization.
    /// The caller authenticates the Chat authority and independently verifies any required
    /// one-time confirmation for this exact invocation before reading from the same handle.
    pub async fn inspect_opened(
        &self,
        request: &TargetChatRead<'_>,
        file: &OpenedRegularFile,
    ) -> Result<FileReadAdmission, String> {
        let path = self.requested_path(request)?;
        if file.public_path().to_str() != Some(path) {
            return Err("File read arguments do not match the opened regular file".into());
        }
        let mut local_names = vec![path];
        for name in [file.resolved_public_path(), file.backing_path()] {
            let name = name.to_str().ok_or("Unrepresentable file path")?;
            if !local_names.contains(&name) {
                local_names.push(name);
            }
        }
        self.inspect_policy(request, &local_names, false)
            .await
            .map_err(|e| e.to_string())
    }

    /// Read the exact file previously approved by a signed-in user on this Device.
    /// A missing or unrelated rule leaves the policy-only result intact.
    pub(super) async fn read_previously_allowed(
        &self,
        request: &TargetChatRead<'_>,
        max_text_bytes: usize,
        confirmed_invocation: bool,
    ) -> Result<Option<rua_core::tool::SharedReadPage>, TargetReadRejection> {
        let ctx = self.tool_call_context(request);
        let local_approval = || {
            self.security
                .approval_authority()
                .locally_approved_chat_read(&ctx, &self.local_device_id)
        };
        let Some(path) = local_approval()
            .await
            .map_err(|_| TargetReadRejection::Unavailable)?
        else {
            return Ok(None);
        };
        if self.requested_path(request).ok() != Some(path.as_str()) {
            return Err(TargetReadRejection::Denied);
        }
        let approved = Path::new(&path);
        rua_tool::fs::WorkspaceFs::check_secret_dir(approved)
            .map_err(|_| TargetReadRejection::Denied)?;
        if self
            .inspect_policy(request, &[path.as_str()], confirmed_invocation)
            .await?
            != FileReadAdmission::Allowed
        {
            return Err(TargetReadRejection::Denied);
        }
        let tool = rua_tool::ReadFileTool;
        let file = tool
            .open_regular_file(approved, &self.rua_root)
            .await
            .map_err(|error| match error {
                rua_core::RuaError::Security { .. } => TargetReadRejection::Denied,
                _ => TargetReadRejection::FileUnavailable,
            })?;
        if !file
            .matches_approved_path(approved)
            .await
            .map_err(|_| TargetReadRejection::Unavailable)?
        {
            return Err(TargetReadRejection::Denied);
        }
        let resolved = file.resolved_public_path().to_owned();
        let backing = file.backing_path().to_owned();
        let mut names = vec![path.as_str()];
        for p in [&resolved, &backing] {
            let name = p.to_str().ok_or(TargetReadRejection::Unavailable)?;
            if !names.contains(&name) {
                names.push(name);
            }
        }
        let permitted = || self.inspect_policy(request, &names, confirmed_invocation);
        if permitted().await? != FileReadAdmission::Allowed {
            return Err(TargetReadRejection::Denied);
        }
        let store = rua_core::ChatStore::resolve(&self.rua_root, request.address.clone());
        let local = rua_core::tool::ToolContext {
            agent_id: request.agent_id.to_owned(),
            session_id: request.address.chat_id().to_string(),
            local_device_id: Some(self.local_device_id.clone()),
            working_dir: self.rua_root.clone(),
            conversation_store_dir: Some(store.root().to_owned()),
            tool_result_store_dir: Some(store.root().join("tool_call_logs")),
            raw_capture_max_bytes: max_text_bytes,
            metadata: ctx.metadata.clone(),
            ..Default::default()
        };
        let page = tool
            .read_opened_page(file, request.args.clone(), local)
            .await
            .map_err(|_| TargetReadRejection::FileUnavailable)?;
        if local_approval()
            .await
            .map_err(|_| TargetReadRejection::Unavailable)?
            .as_ref()
            != Some(&path)
            || permitted().await? != FileReadAdmission::Allowed
        {
            return Err(TargetReadRejection::Denied);
        }
        Ok(Some(page))
    }

    fn requested_path<'a>(&self, request: &TargetChatRead<'a>) -> Result<&'a str, String> {
        let args = request
            .args
            .as_object()
            .ok_or("Invalid file read arguments")?;
        if self.local_device_id.trim().is_empty()
            || request.tool_name != "read"
            || request.turn_id.trim().is_empty()
            || request.tool_call_id.trim().is_empty()
            || args.get("device_id").and_then(Value::as_str) != Some(self.local_device_id.as_str())
        {
            return Err("Expected a Chat read invocation for this Device".into());
        }
        let path = args
            .get("file_path")
            .and_then(Value::as_str)
            .ok_or("A file path is required")?;
        if !Path::new(path).is_absolute()
            || !rua_core::execution_directory::is_device_absolute_path(path)
            || path.starts_with("//")
            || (args.contains_key("byte_offset")
                && (args.contains_key("start_line") || args.contains_key("limit")))
            || path.starts_with(r"\\")
            || args.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "device_id" | "file_path" | "start_line" | "limit" | "byte_offset"
                )
            })
            || ["start_line", "limit", "byte_offset"].iter().any(|&key| {
                args.get(key).is_some_and(|value| {
                    value
                        .as_u64()
                        .is_none_or(|n| n == 0 && key != "byte_offset")
                })
            })
        {
            return Err("Invalid single-file read arguments for this Device".into());
        }
        Ok(path)
    }

    async fn checked_chat_configuration(
        &self,
        request: &TargetChatRead<'_>,
    ) -> Result<ChatHookConfiguration, FileReadPolicyError> {
        let chat_id = request.address.chat_id().as_str();
        super::console_chat::resolve_chat_id(chat_id).map_err(Denied)?;
        let mut header = super::chat_context::load_live_chat_header(&self.rua_root, chat_id)
            .await
            .map_err(|e| Unavailable(e.to_string()))?
            .ok_or_else(|| Unavailable("Chat is unavailable on this Device".into()))?;
        if header.metadata.archived {
            return Err(Denied("Chat is archived".into()));
        }
        if header.chat_id != chat_id || header.agent_id != request.agent_id {
            return Err(Unavailable("Chat identity or Agent has changed".into()));
        }
        if let Some(project_id) = header.project_id.as_deref() {
            let project_id = rua_core::ProjectId::new(project_id);
            if self
                .project_catalog
                .lifecycle_transition_in_progress(&project_id)
            {
                return Err(Unavailable("Chat Project is transitioning".into()));
            }
            let project = self
                .project_catalog
                .get(&project_id)
                .map_err(|e| Unavailable(e.to_string()))?
                .ok_or_else(|| Unavailable("Chat Project is unavailable".into()))?;
            if project.lifecycle != rua_project::ProjectLifecycle::Active {
                return Err(Denied("Chat Project is archived".into()));
            }
        }
        let configuration = header
            .extra
            .remove("hook_configuration")
            .ok_or_else(|| Unavailable("Captured Chat configuration is unavailable".into()))?;
        let placement = parse_conversation_authority_placement(&Value::Object(header.extra))
            .map_err(|e| Unavailable(e.to_string()))?;
        if placement.device_id != request.authority_device_id
            || placement.authority_epoch != request.authority_epoch
        {
            return Err(Unavailable(
                "Chat authority Device or epoch has changed".into(),
            ));
        }
        let captured = ChatHookConfiguration::from_persisted(configuration).map_err(Unavailable)?;
        if captured.selected_agent_id != request.agent_id
            || captured.epoch != request.configuration_epoch
        {
            return Err(Unavailable(
                "Captured Chat Agent or configuration epoch has changed".into(),
            ));
        }
        let agent_enabled = self
            .agent_service
            .registered_agent_enabled(request.agent_id)
            .map_err(|e| Unavailable(e.to_string()))?
            .ok_or_else(|| Unavailable("Chat Agent is unavailable on this Device".into()))?;
        if !agent_enabled {
            return Err(Denied("Chat Agent is disabled on this Device".into()));
        }
        Ok(captured)
    }

    async fn inspect_policy(
        &self,
        request: &TargetChatRead<'_>,
        local_names: &[&str],
        confirmed_invocation: bool,
    ) -> Result<FileReadAdmission, FileReadPolicyError> {
        let captured = self.checked_chat_configuration(request).await?;
        let capabilities = captured
            .capability_configuration
            .ok_or_else(|| Unavailable("Captured Chat tool selection is unavailable".into()))?;
        let read = ToolIdentity::builtin("read");
        let definition = self
            .tool_catalog
            .get("read")
            .ok_or_else(|| Unavailable("The read tool is unavailable on this Device".into()))?;
        let capability = definition.get("capability").and_then(Value::as_str);
        let selected = self
            .tool_config_store
            .selection_policy(Some(request.agent_id))
            .await
            .map_err(|e| Unavailable(e.to_string()))?;
        if captured
            .agent_assignment
            .as_ref()
            .is_some_and(|assignment| assignment.disabled_tools.iter().any(|name| name == "read"))
            || !capabilities.selection_policy.selects(&read, capability)
            || !selected.selects(&read, capability)
        {
            return Err(Denied(
                "The read tool is disabled for this Chat or target Agent".into(),
            ));
        }
        let ctx = self.tool_call_context(request);
        if confirmed_invocation {
            self.security
                .inspect_preapproved_chat_file_read(
                    &ctx,
                    &captured.security_config,
                    local_names,
                    true,
                )
                .await
        } else {
            self.security
                .inspect_chat_file_read(&ctx, &captured.security_config, local_names)
                .await
        }
    }

    fn tool_call_context(&self, request: &TargetChatRead<'_>) -> ToolCallContext {
        let mut ctx = ToolCallContext::new(
            request.agent_id,
            request.address.chat_id().as_str(),
            "read",
            request.args.clone(),
        )
        .with_chat_address(request.address.clone());
        ctx.local_device_id = Some(self.local_device_id.clone());
        ctx.metadata
            .insert("turn_id".into(), request.turn_id.into());
        ctx.metadata
            .insert("tool_call_id".into(), request.tool_call_id.into());
        ctx
    }
}
