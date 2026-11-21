use std::path::{Path, PathBuf};
use std::sync::Arc;

use rua_config::types::tools::ToolIdentity;
use rua_contract::Tool;
use rua_core::tool::{ToolContext, ToolResult};
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
use super::device_read_check::DeviceFileReadReply;
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

    pub async fn discover_agents_instructions(
        &self,
        context: &TargetChatRead<'_>,
        discovery: &rua_core::agents_instruction_discovery::AgentsInstructionDiscoveryRequest,
        cancel: tokio_util::sync::CancellationToken,
    ) -> rua_core::Result<rua_core::agents_instruction_discovery::AgentsInstructionDiscovery> {
        if discovery.device_id != self.local_device_id {
            return Err(rua_core::RuaError::Validation {
                message: "AGENTS.md discovery was addressed to another Device".into(),
            });
        }
        let cancel = cancel.child_token();
        let _cancel_on_drop = cancel.clone().drop_guard();
        let discover = async {
            self.checked_chat_configuration(context)
                .await
                .map_err(|error| rua_core::RuaError::Runtime {
                    message: error.to_string(),
                })?;
            let fs = rua_tool::WorkspaceFs::for_execution(
                &self.rua_root,
                rua_core::runtime_layout::RuntimeLayout::from_current_process(None),
            )?;
            rua_tool::agents_instructions::discover_agents_instructions(
                &fs,
                discovery,
                &cancel,
                |requested, resolved| async move {
                    let (Some(requested), Some(resolved)) = (requested.to_str(), resolved.to_str())
                    else {
                        return false;
                    };
                    let args =
                        serde_json::json!({"device_id":self.local_device_id,"file_path":requested});
                    let read = TargetChatRead {
                        address: context.address,
                        agent_id: context.agent_id,
                        authority_device_id: context.authority_device_id,
                        authority_epoch: context.authority_epoch,
                        configuration_epoch: context.configuration_epoch,
                        turn_id: context.turn_id,
                        tool_call_id: context.tool_call_id,
                        tool_name: "read",
                        args: &args,
                    };
                    // Discovery only uses existing permission; it never opens an approval.
                    self.inspect_policy(&read, &[requested, resolved], false)
                        .await
                        .is_ok_and(|admission| admission == FileReadAdmission::Allowed)
                },
            )
            .await
        };
        tokio::time::timeout(std::time::Duration::from_secs(20), discover)
            .await
            .map_err(|_| rua_core::RuaError::Timeout {
                operation: "AGENTS.md discovery".into(),
            })?
    }

    /// Verify the local replica's authority and captured configuration before contacting the
    /// origin. A stale or unknown authority cannot consume a one-time decision at the origin.
    pub async fn check_authority(
        &self,
        request: &TargetChatRead<'_>,
    ) -> Result<(), TargetReadRejection> {
        self.requested_paths(request)
            .map_err(|_| TargetReadRejection::Denied)?;
        self.checked_chat_configuration(request)
            .await
            .map(|_| ())
            .map_err(Into::into)
    }

    /// Classify the target policy from the requested path spelling without probing the file or
    /// creating an approval. File execution rechecks policy against the opened file.
    pub async fn check_metadata(
        &self,
        request: &TargetChatRead<'_>,
    ) -> Result<FileReadAdmission, TargetReadRejection> {
        let paths = self
            .requested_paths(request)
            .map_err(|_| TargetReadRejection::Denied)?;
        for path in &paths {
            rua_tool::fs::WorkspaceFs::check_secret_dir(Path::new(path))
                .map_err(|_| TargetReadRejection::Denied)?;
        }
        let names = paths.iter().map(String::as_str).collect::<Vec<_>>();
        self.inspect_policy(request, &names, false)
            .await
            .map_err(Into::into)
    }

    /// Run the existing local file tool after evaluating the requested and resolved paths.
    pub async fn execute_authorized(
        &self,
        request: &TargetChatRead<'_>,
        max_text_bytes: usize,
        confirmed_invocation: bool,
        cancel: Option<tokio::sync::watch::Receiver<bool>>,
    ) -> Result<DeviceFileReadReply, TargetReadRejection> {
        let paths = self
            .requested_paths(request)
            .map_err(|_| TargetReadRejection::Denied)?;
        let fs = rua_tool::WorkspaceFs::for_execution(
            &self.rua_root,
            rua_core::runtime_layout::RuntimeLayout::from_current_process(None),
        )
        .map_err(|_| TargetReadRejection::Unavailable)?;
        let requested_names = paths.iter().map(String::as_str).collect::<Vec<_>>();
        if self
            .inspect_policy(request, &requested_names, confirmed_invocation)
            .await?
            != FileReadAdmission::Allowed
        {
            return Err(TargetReadRejection::Denied);
        }
        let mut names = paths.clone();
        let mut resolved_paths = Vec::with_capacity(paths.len());
        for path in &paths {
            let resolved = if request.tool_name == "edit" {
                fs.resolve_for_create(path)
            } else {
                fs.resolve(path)
            }
            .map_err(|error| match error {
                rua_tool::ToolError::SecretDirDenied { .. }
                | rua_tool::ToolError::PathSecurity { .. } => TargetReadRejection::Denied,
                _ => TargetReadRejection::FileUnavailable,
            })?;
            let name = resolved
                .to_str()
                .ok_or(TargetReadRejection::Unavailable)?
                .to_owned();
            if !names.contains(&name) {
                names.push(name);
            }
            resolved_paths.push(resolved);
        }
        let names = names.iter().map(String::as_str).collect::<Vec<_>>();
        if self
            .inspect_policy(request, &names, confirmed_invocation)
            .await?
            != FileReadAdmission::Allowed
        {
            return Err(TargetReadRejection::Denied);
        }
        if request.tool_name == "read"
            && tokio::fs::metadata(&resolved_paths[0])
                .await
                .map_err(|_| TargetReadRejection::FileUnavailable)?
                .is_file()
        {
            return self
                .read_authorized(request, max_text_bytes, confirmed_invocation, cancel)
                .await
                .map(DeviceFileReadReply::Page);
        }

        let store = rua_core::ChatStore::resolve(&self.rua_root, request.address.clone());
        let context = ToolContext {
            agent_id: request.agent_id.to_owned(),
            session_id: request.address.chat_id().to_string(),
            local_device_id: Some(self.local_device_id.clone()),
            working_dir: self.rua_root.clone(),
            conversation_store_dir: Some(store.root().to_owned()),
            tool_result_store_dir: Some(store.root().join("tool_call_logs")),
            raw_capture_max_bytes: max_text_bytes,
            metadata: self.tool_call_context(request).metadata,
            cancel,
            ..Default::default()
        };
        let mut args = request.args.clone();
        let (limit_key, default_limit, max_limit) = if request.tool_name == "read" {
            ("limit", 100, 200)
        } else {
            ("max_results", 200, 2000)
        };
        let mut page_limit = args[limit_key]
            .as_u64()
            .unwrap_or(default_limit)
            .clamp(1, max_limit);
        let result = loop {
            if context
                .cancel
                .as_ref()
                .is_some_and(|cancel| *cancel.borrow())
            {
                break Ok(ToolResult::error(
                    request.tool_call_id,
                    "File operation cancelled",
                ));
            }
            let result = match request.tool_name {
                "read" => {
                    rua_tool::ReadFileTool
                        .call(args.clone(), context.clone())
                        .await
                }
                "edit" => {
                    rua_tool::EditFileTool
                        .call(args.clone(), context.clone())
                        .await
                }
                "grep" => {
                    rua_tool::extended::search::GrepSearchTool
                        .call(args.clone(), context.clone())
                        .await
                }
                "glob" => {
                    rua_tool::extended::search::GlobSearchTool
                        .call(args.clone(), context.clone())
                        .await
                }
                _ => return Err(TargetReadRejection::Denied),
            };
            if request.tool_name == "edit"
                || result.as_ref().map_or(true, |result| {
                    (result.is_error && !result.metadata.contains_key("omitted_entries"))
                        || result.text_content().len() <= max_text_bytes
                })
            {
                break result;
            }
            // Regenerate a smaller complete page from the same starting point. Trimming
            // its text would leave a continuation that skips entries never delivered.
            if page_limit == 1 {
                let mut error = ToolResult::error(
                    request.tool_call_id,
                    "One result exceeds the output byte budget. Narrow the query or increase the output limit.",
                );
                error
                    .metadata
                    .insert("output_error".into(), "page_budget_too_small".into());
                break Ok(error);
            }
            page_limit /= 2;
            args[limit_key] = page_limit.into();
        };
        let mut result = match result {
            Ok(result) => result,
            Err(error) => ToolResult::error(request.tool_call_id, error.to_string()),
        };
        // A completed edit may already have effects, even if permissions change meanwhile.
        if request.tool_name != "edit"
            && self
                .inspect_policy(request, &names, confirmed_invocation)
                .await?
                != FileReadAdmission::Allowed
        {
            return Err(TargetReadRejection::Denied);
        }
        if let Some(meta) = result.truncation.as_mut() {
            meta.recall_key = None;
        }
        let mut remaining = max_text_bytes;
        let mut truncated = false;
        for part in &mut result.content {
            if let agent_types::ContentPart::Text { text } = part {
                let mut end = text.len().min(remaining);
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                truncated |= end < text.len();
                text.truncate(end);
                remaining -= end;
            }
        }
        let is_error = result.is_error;
        let location = result.metadata.entry("file_location").or_insert_with(|| {
            serde_json::json!({
                "device_id": self.local_device_id,
                "paths": paths,
                "path": paths.first(),
                "coverage": if is_error { "unavailable" } else { "complete" },
            })
        });
        location["dispatched"] = true.into();
        if truncated {
            if location["coverage"] != "unavailable" {
                location["coverage"] = "partial".into();
            }
            result
                .metadata
                .insert("output_truncated".into(), true.into());
        }
        Ok(DeviceFileReadReply::Result(Box::new(result)))
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

    /// Apply the shared user policy and any verified confirmation for this invocation,
    /// then read the file on its named Device through the checked handle.
    pub(super) async fn read_authorized(
        &self,
        request: &TargetChatRead<'_>,
        max_text_bytes: usize,
        confirmed_invocation: bool,
        cancel: Option<tokio::sync::watch::Receiver<bool>>,
    ) -> Result<rua_core::tool::SharedReadPage, TargetReadRejection> {
        let ctx = self.tool_call_context(request);
        let path = self
            .requested_path(request)
            .map_err(|_| TargetReadRejection::Denied)?;
        let approved = Path::new(path);
        rua_tool::fs::WorkspaceFs::check_secret_dir(approved)
            .map_err(|_| TargetReadRejection::Denied)?;
        if self
            .inspect_policy(request, &[path], confirmed_invocation)
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
        let mut names = vec![path];
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
            cancel,
            ..Default::default()
        };
        let page = tool
            .read_opened_page(file, request.args.clone(), local)
            .await
            .map_err(|_| TargetReadRejection::FileUnavailable)?;
        if permitted().await? != FileReadAdmission::Allowed {
            return Err(TargetReadRejection::Denied);
        }
        Ok(page)
    }

    fn requested_paths(&self, request: &TargetChatRead<'_>) -> Result<Vec<String>, String> {
        if self.local_device_id.is_empty()
            || request.turn_id.is_empty()
            || request.tool_call_id.is_empty()
            || request.args["device_id"].as_str() != Some(&self.local_device_id)
        {
            return Err("Expected a Chat file operation on this Device".into());
        }
        let paths = match request.tool_name {
            "read" => vec![self.requested_path(request)?.to_owned()],
            "grep" | "glob" => vec![request.args["path"]
                .as_str()
                .ok_or("A remote search requires an absolute path")?
                .to_owned()],
            "edit" => rua_tool::EditFileTool::affected_paths(
                request.args["patch"]
                    .as_str()
                    .ok_or("An edit patch is required")?,
            )?,
            _ => return Err("Unsupported file operation".into()),
        };
        if paths.iter().any(|path| {
            !Path::new(path).is_absolute()
                || !rua_core::execution_directory::is_device_absolute_path(path)
                || path.starts_with("//")
                || path.starts_with(r"\")
                || path.chars().any(char::is_control)
        }) {
            return Err(
                "File operations require actual absolute paths on the target Device".into(),
            );
        }
        Ok(paths)
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
                    "device_id" | "file_path" | "start_line" | "limit" | "byte_offset" | "after"
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
        let read = ToolIdentity::builtin(request.tool_name);
        let definition = self.tool_catalog.get(request.tool_name).ok_or_else(|| {
            Unavailable("The requested file tool is unavailable on this Device".into())
        })?;
        let capability = definition.get("capability").and_then(Value::as_str);
        let selected = self
            .tool_config_store
            .selection_policy(Some(request.agent_id))
            .await
            .map_err(|e| Unavailable(e.to_string()))?;
        if captured
            .agent_assignment
            .as_ref()
            .is_some_and(|assignment| {
                assignment
                    .disabled_tools
                    .iter()
                    .any(|name| name == request.tool_name)
            })
            || !capabilities.selection_policy.selects(&read, capability)
            || !selected.selects(&read, capability)
        {
            return Err(Denied(
                "The requested file tool is disabled for this Chat or target Agent".into(),
            ));
        }
        let ctx = self.tool_call_context(request);
        if request.tool_name == "read" {
            return self
                .security
                .inspect_confirmed_chat_file_read(
                    &ctx,
                    &captured.security_config,
                    local_names,
                    confirmed_invocation,
                )
                .await;
        }
        let operations = local_names
            .iter()
            .map(|path| {
                let mut operation = ctx.clone();
                if request.tool_name == "edit" {
                    operation.tool_input = serde_json::json!({
                        "device_id": self.local_device_id,
                        "path": path,
                    });
                } else {
                    operation.tool_input["path"] = (*path).into();
                }
                operation
            })
            .collect::<Vec<_>>();
        self.security
            .inspect_chat_file_operation(
                &ctx,
                &captured.security_config,
                &operations,
                confirmed_invocation,
            )
            .await
    }

    fn tool_call_context(&self, request: &TargetChatRead<'_>) -> ToolCallContext {
        let mut ctx = ToolCallContext::new(
            request.agent_id,
            request.address.chat_id().as_str(),
            request.tool_name,
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
