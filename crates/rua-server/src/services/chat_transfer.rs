use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::chat_file_read_admission::TargetReadRejection;
use super::chat_file_read_admission::{FileReadAdmission, TargetFileReadContext};
use super::conversation_authority::{
    ConversationActiveTurnPhase, ConversationAuthority, ConversationDriverKind,
};
use super::device_read_check::DeviceReadCheckReply;
use super::device_read_check::DeviceReadCheckRequest;
use super::sync_transfer::SyncTransferPlanError;
use super::sync_transfer::{
    build_transfer_manifest_metadata, capture_transfer_digests_with_cancel,
    execute_local_chat_transfer_plan, plan_receive, send_transfer_stream,
    LocalTransferAuthorization, ReceiveConflictPolicy, SourceTransferOperationStore,
    SyncTransferBulkHandler, TransferExecutionOptions, TransferExecutionOutcome,
    TransferExecutionProgress, TransferStreamSendRequest,
};
use super::sync_transfer::{SourceTransferOperationSnapshot, TransferManifest};
use rua_device_network::DeviceSessionManager;

/// The original Chat invocation, distinct from the Device sending file bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatTransferContext {
    pub operation_id: String,
    pub call: DeviceReadCheckRequest,
    #[serde(default, flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatTransferCommand {
    pub context: ChatTransferContext,
    pub action: ChatTransferAction,
    #[serde(default, flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ChatTransferAction {
    SourcePlan,
    PrepareReceive {
        manifest: TransferManifest,
    },
    Start {
        manifest: TransferManifest,
        destination: Box<PreparedChatReceive>,
    },
    Status,
    Cancel,
    CancelReceive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedChatReceive {
    pub target_dir: PathBuf,
    pub paths: Vec<String>,
    pub context: ChatTransferContext,
    #[serde(default, flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatTransferResponse {
    Policy {
        response: super::device_read_check::DeviceReadCheckReply,
    },
    SourcePlan {
        manifest: TransferManifest,
        paths: Vec<String>,
    },
    Prepared {
        prepared: Box<PreparedChatReceive>,
    },
    Operation {
        operation: SourceTransferOperationSnapshot,
    },
    ReceiveCancelled,
}

#[async_trait::async_trait]
pub trait ChatTransferAuthorization: Send + Sync {
    async fn check_receive(
        &self,
        context: &ChatTransferContext,
        target_path: &Path,
        confirmed_invocation: bool,
    ) -> Result<(), SyncTransferPlanError>;
}

pub struct RuntimeChatTransferAuthorization {
    pub rua_root: PathBuf,
    pub target: Arc<TargetFileReadContext>,
}

#[derive(Clone)]
pub struct ChatTransferService {
    rua_root: PathBuf,
    local_device_id: String,
    target: Arc<TargetFileReadContext>,
    bulk: Arc<SyncTransferBulkHandler>,
    operations: Arc<SourceTransferOperationStore>,
    sessions: Arc<RwLock<Option<DeviceSessionManager>>>,
    authority: Arc<ConversationAuthority>,
    approvals: Arc<rua_security::ApprovalAuthority>,
    invocations: Arc<Mutex<HashMap<String, ParticipantInvocation>>>,
}

struct ParticipantInvocation {
    context: ChatTransferContext,
    prepared: Option<(PreparedChatReceive, bool)>,
    last_used: Instant,
}

impl ChatTransferService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        rua_root: PathBuf,
        local_device_id: String,
        target: Arc<TargetFileReadContext>,
        bulk: Arc<SyncTransferBulkHandler>,
        operations: Arc<SourceTransferOperationStore>,
        sessions: Arc<RwLock<Option<DeviceSessionManager>>>,
        authority: Arc<ConversationAuthority>,
        approvals: Arc<rua_security::ApprovalAuthority>,
    ) -> Self {
        Self {
            rua_root,
            local_device_id,
            target,
            bulk,
            operations,
            sessions,
            authority,
            approvals,
            invocations: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn handle(
        &self,
        command: ChatTransferCommand,
    ) -> Result<ChatTransferResponse, String> {
        let context = command.context;
        self.validate(&context, &command.action)?;
        let known = {
            let mut invocations = self
                .invocations
                .lock()
                .expect("Chat transfer invocations lock");
            invocations.retain(|id, invocation| {
                invocation.last_used.elapsed() < Duration::from_secs(600)
                    || self.operations.is_running(id)
            });
            if let Some(previous) = invocations.get_mut(&context.operation_id) {
                if !same_invocation(&previous.context, &context) {
                    return Err("Transfer operation belongs to a different Chat invocation".into());
                }
                previous.last_used = Instant::now();
                true
            } else {
                false
            }
        };
        // An authenticated stop/status for an existing invocation remains available after
        // configuration revocation, so revoking permission cannot strand ongoing IO.
        let requires_current_configuration = !known
            || !matches!(
                command.action,
                ChatTransferAction::Status
                    | ChatTransferAction::Cancel
                    | ChatTransferAction::CancelReceive
            );
        if requires_current_configuration
            && self
                .target
                .checked_chat_configuration(&context.call.as_target_read())
                .await
                .is_err()
        {
            return Ok(policy(DeviceReadCheckReply::Unavailable));
        }
        {
            let mut invocations = self
                .invocations
                .lock()
                .expect("Chat transfer invocations lock");
            match invocations.entry(context.operation_id.clone()) {
                std::collections::hash_map::Entry::Occupied(entry) => {
                    if !same_invocation(&entry.get().context, &context) {
                        return Err("Transfer operation invocation changed".into());
                    }
                }
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(ParticipantInvocation {
                        context: context.clone(),
                        prepared: None,
                        last_used: Instant::now(),
                    });
                }
            }
        }
        match command.action {
            ChatTransferAction::SourcePlan => self.source_plan(context).await,
            ChatTransferAction::PrepareReceive { manifest } => {
                self.prepare(context, manifest).await
            }
            ChatTransferAction::Start {
                manifest,
                destination,
            } => self.start(context, manifest, *destination).await,
            ChatTransferAction::Status => self
                .operations
                .get(&context.operation_id)
                .map(|operation| ChatTransferResponse::Operation { operation })
                .ok_or_else(|| "Transfer operation has not started".into()),
            ChatTransferAction::Cancel => self
                .operations
                .cancel(&context.operation_id)
                .map(|operation| ChatTransferResponse::Operation { operation })
                .ok_or_else(|| "Transfer operation is unavailable".into()),
            ChatTransferAction::CancelReceive => {
                let prepared = self
                    .invocations
                    .lock()
                    .expect("Chat transfer invocations lock")
                    .get(&context.operation_id)
                    .and_then(|entry| entry.prepared.as_ref())
                    .map(|(prepared, _)| prepared.context.clone());
                self.bulk
                    .cancel_chat_receive(prepared.as_ref().unwrap_or(&context))
                    .await
                    .map_err(|error| error.to_string())?;
                Ok(ChatTransferResponse::ReceiveCancelled)
            }
        }
    }

    fn validate(
        &self,
        context: &ChatTransferContext,
        action: &ChatTransferAction,
    ) -> Result<(), String> {
        let call = &context.call;
        let args = &call.arguments;
        let source = args["source_device_id"]
            .as_str()
            .filter(|value| !value.trim().is_empty());
        let target = args["target_device_id"]
            .as_str()
            .filter(|value| !value.trim().is_empty());
        let source_action = matches!(
            action,
            ChatTransferAction::SourcePlan
                | ChatTransferAction::Start { .. }
                | ChatTransferAction::Status
                | ChatTransferAction::Cancel
        );
        if context.operation_id.trim().is_empty()
            || call.tool_name != "sync_send"
            || call.target_device_id != self.local_device_id
            || call.source_device_id.trim().is_empty()
            || call.turn_id.trim().is_empty()
            || call.tool_call_id.trim().is_empty()
            || source.is_none()
            || target.is_none()
            || (if source_action { source } else { target }) != Some(self.local_device_id.as_str())
            || !args["source_paths"].as_array().is_some_and(|paths| {
                !paths.is_empty()
                    && paths.iter().all(|path| {
                        path.as_str()
                            .is_some_and(rua_core::execution_directory::is_device_absolute_path)
                    })
            })
            || !args["target_dir"]
                .as_str()
                .is_some_and(rua_core::execution_directory::is_device_absolute_path)
        {
            return Err("Invalid explicit Chat transfer invocation for this Device".into());
        }
        conflict(&call.arguments)?;
        Ok(())
    }

    fn filesystem(&self) -> Result<rua_tool::WorkspaceFs, String> {
        rua_tool::WorkspaceFs::for_execution(
            &self.rua_root,
            rua_core::runtime_layout::RuntimeLayout::from_current_process(None),
        )
        .map_err(|error| error.to_string())
    }

    fn source_roots(
        &self,
        context: &ChatTransferContext,
    ) -> Result<Vec<(String, PathBuf)>, String> {
        let fs = self.filesystem()?;
        context.call.arguments["source_paths"]
            .as_array()
            .ok_or("Missing source paths")?
            .iter()
            .map(|path| {
                let public = path.as_str().ok_or("Invalid source path")?;
                let native = fs.resolve(public).map_err(|error| error.to_string())?;
                Ok((public.to_owned(), native))
            })
            .collect()
    }

    async fn confirmation(
        &self,
        context: &ChatTransferContext,
    ) -> Result<bool, DeviceReadCheckReply> {
        let call = &context.call;
        let Some(confirmation) = call.confirmation.as_ref() else {
            return Ok(false);
        };
        if call.source_device_id != self.local_device_id {
            let manager = self
                .sessions
                .read()
                .expect("Device sessions lock")
                .clone()
                .ok_or(DeviceReadCheckReply::Unavailable)?;
            return if call.verify_at_origin(&manager, confirmation).await {
                Ok(true)
            } else {
                Err(DeviceReadCheckReply::Unavailable)
            };
        }
        let active = self.authority.active_turn(&call.address).await;
        if !active.is_some_and(|active| {
            active.authority_device_id == call.source_device_id
                && active.authority_epoch == call.authority_epoch
                && active.turn_id == call.turn_id
                && active.driver == ConversationDriverKind::Rua
                && matches!(
                    active.phase,
                    ConversationActiveTurnPhase::RunningTool
                        | ConversationActiveTurnPhase::Streaming
                        | ConversationActiveTurnPhase::WaitingApproval
                )
        }) {
            return Err(DeviceReadCheckReply::Unavailable);
        }
        let mut check = rua_security::ToolCallContext::new(
            &call.agent_id,
            call.address.chat_id().as_str(),
            "sync_send",
            call.arguments.clone(),
        )
        .with_chat_address(call.address.clone());
        check.local_device_id = Some(self.local_device_id.clone());
        check
            .metadata
            .insert("turn_id".into(), call.turn_id.clone().into());
        check
            .metadata
            .insert("tool_call_id".into(), call.tool_call_id.clone().into());
        let claimed = self
            .approvals
            .consume_authenticated_transfer_confirmation_if(
                &check,
                &self.local_device_id,
                |approval_id| {
                    approval_id == confirmation.approval_id
                        && self.authority.claim_read_dispatch(
                            confirmation.dispatch_token,
                            approval_id,
                            &self.local_device_id,
                        )
                },
            )
            .await;
        if claimed.as_deref() == Some(confirmation.approval_id.as_str()) {
            Ok(true)
        } else {
            Err(DeviceReadCheckReply::Unavailable)
        }
    }

    async fn inspect_sources(
        &self,
        context: &ChatTransferContext,
        roots: &[(String, PathBuf)],
        manifest: Option<&TransferManifest>,
        confirmed: bool,
    ) -> Result<Vec<String>, DeviceReadCheckReply> {
        let fs = self
            .filesystem()
            .map_err(|_| DeviceReadCheckReply::Unavailable)?;
        let native_paths: Vec<&Path> = match manifest {
            Some(manifest) => manifest
                .entries
                .iter()
                .map(|entry| entry.source_path.as_path())
                .collect(),
            None => roots.iter().map(|(_, native)| native.as_path()).collect(),
        };
        let mut paths = Vec::with_capacity(native_paths.len());
        let mut needs_confirmation = false;
        for native in native_paths {
            let requested = roots
                .iter()
                .find_map(|(public, root)| {
                    native
                        .strip_prefix(root)
                        .ok()
                        .map(|suffix| Path::new(public).join(suffix))
                })
                .ok_or(DeviceReadCheckReply::Denied)?;
            let resolved =
                public_path(&fs, native).map_err(|_| DeviceReadCheckReply::Unavailable)?;
            match self
                .target
                .inspect_transfer_source(
                    &context.call.as_target_read(),
                    requested
                        .to_str()
                        .ok_or(DeviceReadCheckReply::Unavailable)?,
                    &resolved,
                    confirmed,
                )
                .await
            {
                Ok(FileReadAdmission::Allowed) => {}
                Ok(FileReadAdmission::RequiresUserConfirmation) => needs_confirmation = true,
                Err(rejection) => return Err(rejection_reply(rejection)),
            }
            paths.push(resolved);
        }
        if needs_confirmation {
            Err(DeviceReadCheckReply::ConfirmationRequired)
        } else {
            Ok(paths)
        }
    }

    async fn metadata(&self, roots: &[(String, PathBuf)]) -> Result<TransferManifest, String> {
        let paths = roots
            .iter()
            .map(|(_, path)| path.clone())
            .collect::<Vec<_>>();
        let manifest =
            tokio::task::spawn_blocking(move || build_transfer_manifest_metadata(&paths))
                .await
                .map_err(|error| error.to_string())?
                .map_err(|error| error.to_string())?;
        // Control messages carry metadata only; leave room for context, public paths and receipts.
        if manifest.entries.is_empty()
            || manifest.entries.len() > 512
            || serde_json::to_vec(&manifest)
                .map_err(|error| error.to_string())?
                .len()
                > 256 * 1024
        {
            return Err("Select a nonempty, smaller batch of files for this transfer".into());
        }
        Ok(manifest)
    }

    async fn source_plan(
        &self,
        context: ChatTransferContext,
    ) -> Result<ChatTransferResponse, String> {
        let roots = self.source_roots(&context)?;
        let confirmed = match self.confirmation(&context).await {
            Ok(value) => value,
            Err(reply) => return Ok(policy(reply)),
        };
        if let Err(reply) = self
            .inspect_sources(&context, &roots, None, confirmed)
            .await
        {
            return Ok(policy(reply));
        }
        let manifest = self.metadata(&roots).await?;
        match self
            .inspect_sources(&context, &roots, Some(&manifest), confirmed)
            .await
        {
            Ok(paths) => {
                bounded_control(&(&context, &manifest, &paths))?;
                Ok(ChatTransferResponse::SourcePlan { manifest, paths })
            }
            Err(reply) => Ok(policy(reply)),
        }
    }

    async fn prepare(
        &self,
        context: ChatTransferContext,
        manifest: TransferManifest,
    ) -> Result<ChatTransferResponse, String> {
        if manifest.entries.is_empty()
            || manifest.entries.len() > 512
            || manifest
                .entries
                .iter()
                .any(|entry| !entry.sha256.is_empty())
        {
            return Err(
                "Transfer preparation requires bounded metadata without file content".into(),
            );
        }
        let fs = self.filesystem()?;
        let requested_dir = context.call.arguments["target_dir"]
            .as_str()
            .ok_or("Missing target directory")?;
        let target_dir = fs
            .resolve(requested_dir)
            .map_err(|error| error.to_string())?;
        let conflict_policy = conflict(&context.call.arguments)?;
        let plan = plan_receive(&target_dir, &manifest, conflict_policy)
            .map_err(|error| error.to_string())?;
        bounded_control(&(
            &context,
            &manifest,
            plan.entries
                .iter()
                .map(|entry| &entry.target_path)
                .collect::<Vec<_>>(),
        ))?;
        let confirmed = match self.confirmation(&context).await {
            Ok(value) => value,
            Err(reply) => return Ok(policy(reply)),
        };
        for entry in &plan.entries {
            let requested = Path::new(requested_dir).join(&entry.final_relative_path);
            let resolved = public_path(&fs, &entry.target_path)?;
            match self
                .target
                .inspect_transfer_target(
                    &context.call.as_target_read(),
                    requested.to_str().ok_or("Invalid target path")?,
                    &resolved,
                    confirmed,
                )
                .await
            {
                Ok(FileReadAdmission::Allowed) => {}
                Ok(FileReadAdmission::RequiresUserConfirmation) => {
                    return Ok(policy(DeviceReadCheckReply::ConfirmationRequired))
                }
                Err(rejection) => return Ok(policy(rejection_reply(rejection))),
            }
        }
        let plan = self
            .bulk
            .prepare_chat_receive(
                TransferStreamSendRequest {
                    operation_id: context.operation_id.clone(),
                    source_device_id: context.call.arguments["source_device_id"]
                        .as_str()
                        .unwrap()
                        .into(),
                    target_device_id: self.local_device_id.clone(),
                    manifest,
                    target_dir,
                    conflict_policy,
                    options: TransferExecutionOptions::default(),
                    chat_context: Some(context.clone()),
                },
                confirmed,
            )
            .await
            .map_err(|error| error.to_string())?;
        let prepared = PreparedChatReceive {
            target_dir: plan.target_dir,
            paths: plan
                .entries
                .iter()
                .map(|entry| public_path(&fs, &entry.target_path))
                .collect::<Result<_, _>>()?,
            context: context.clone(),
            extra: Default::default(),
        };
        bounded_control(&prepared)?;
        self.invocations
            .lock()
            .expect("Chat transfer invocations lock")
            .get_mut(&context.operation_id)
            .expect("bound Chat invocation")
            .prepared = Some((prepared.clone(), confirmed));
        Ok(ChatTransferResponse::Prepared {
            prepared: Box::new(prepared),
        })
    }

    async fn start(
        &self,
        context: ChatTransferContext,
        manifest: TransferManifest,
        destination: PreparedChatReceive,
    ) -> Result<ChatTransferResponse, String> {
        let mut target_context = destination.context.clone();
        target_context.call.target_device_id = self.local_device_id.clone();
        if !same_invocation(&context, &target_context)
            || destination.context.call.target_device_id
                != context.call.arguments["target_device_id"].as_str().unwrap()
            || destination.paths.len() != manifest.entries.len()
            || !destination
                .target_dir
                .to_str()
                .is_some_and(rua_core::execution_directory::is_device_absolute_path)
        {
            return Err("Transfer destination does not match the original Chat invocation".into());
        }
        if let Some(operation) = self.operations.get(&context.operation_id) {
            return Ok(ChatTransferResponse::Operation { operation });
        }
        let roots = self.source_roots(&context)?;
        let confirmed = match self.confirmation(&context).await {
            Ok(value) => value,
            Err(reply) => return Ok(policy(reply)),
        };
        if let Err(reply) = self
            .inspect_sources(&context, &roots, None, confirmed)
            .await
        {
            return Ok(policy(reply));
        }
        if self.metadata(&roots).await? != manifest {
            return Err("The selected source files changed; reconsider the transfer".into());
        }
        if let Err(reply) = self
            .inspect_sources(&context, &roots, Some(&manifest), confirmed)
            .await
        {
            return Ok(policy(reply));
        }
        let (operation, cancel) = self.operations.start(
            context.operation_id.clone(),
            manifest.total_bytes,
            self.local_device_id.clone(),
            destination.context.call.target_device_id.clone(),
        )?;
        if let Some(cancel) = cancel {
            let service = self.clone();
            let operation_id = context.operation_id.clone();
            let operations = self.operations.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    if !operations.is_running(&operation_id)
                        || operations.cancel_if_control_expired(&operation_id)
                    {
                        break;
                    }
                }
            });
            tokio::spawn(async move {
                let result = service
                    .execute(&context, manifest, destination, &roots, confirmed, cancel)
                    .await;
                match result {
                    Ok(outcome) => service.operations.succeed(&context.operation_id, outcome),
                    Err(SyncTransferPlanError::Cancelled) => {
                        service.operations.mark_cancelled(&context.operation_id)
                    }
                    Err(error @ SyncTransferPlanError::OutcomeUnknown { .. }) => service
                        .operations
                        .mark_unknown(&context.operation_id, error.to_string()),
                    Err(error) => service
                        .operations
                        .fail(&context.operation_id, error.to_string()),
                }
            });
        }
        Ok(ChatTransferResponse::Operation { operation })
    }

    async fn execute(
        &self,
        context: &ChatTransferContext,
        mut manifest: TransferManifest,
        destination: PreparedChatReceive,
        roots: &[(String, PathBuf)],
        confirmed: bool,
        cancel: tokio::sync::watch::Receiver<rua_core::RunControl>,
    ) -> Result<TransferExecutionOutcome, SyncTransferPlanError> {
        let hash_cancel = cancel.clone();
        manifest = tokio::task::spawn_blocking(move || {
            capture_transfer_digests_with_cancel(&mut manifest, &hash_cancel)?;
            Ok::<_, SyncTransferPlanError>(manifest)
        })
        .await
        .map_err(|error| SyncTransferPlanError::Remote(error.to_string()))??;
        if cancel.borrow().is_terminal() {
            return Err(SyncTransferPlanError::Cancelled);
        }
        self.inspect_sources(context, roots, Some(&manifest), confirmed)
            .await
            .map_err(|_| {
                SyncTransferPlanError::Remote("Source permission changed before sending".into())
            })?;
        let mut receipt_paths = HashMap::<PathBuf, PathBuf>::new();
        let public_paths = &destination.paths;
        let mut on_progress = |mut progress: TransferExecutionProgress| {
            for receipt in &mut progress.completed_files {
                let next = receipt_paths.len();
                if let Some(public) = receipt_paths.get(&receipt.target_path) {
                    receipt.target_path = public.clone();
                } else if let Some(public) = public_paths.get(next) {
                    let public = PathBuf::from(public);
                    receipt_paths.insert(receipt.target_path.clone(), public.clone());
                    receipt.target_path = public;
                }
            }
            self.operations
                .update_progress_with_files(&context.operation_id, &progress);
        };
        let mut outcome = if destination.context.call.target_device_id == self.local_device_id {
            let (prepared, target_confirmed) = self
                .invocations
                .lock()
                .expect("Chat transfer invocations lock")
                .get(&context.operation_id)
                .and_then(|entry| entry.prepared.clone())
                .ok_or_else(|| {
                    SyncTransferPlanError::Remote("Local target preparation is unavailable".into())
                })?;
            if prepared != destination {
                return Err(SyncTransferPlanError::Remote(
                    "Local target preparation changed".into(),
                ));
            }
            let plan = self.bulk.prepared_chat_plan(&destination.context)?;
            let authorization = RuntimeChatTransferAuthorization {
                rua_root: self.rua_root.clone(),
                target: self.target.clone(),
            };
            execute_local_chat_transfer_plan(
                &manifest,
                &plan,
                TransferExecutionOptions::default(),
                cancel,
                &mut on_progress,
                &LocalTransferAuthorization {
                    context: &destination.context,
                    confirmed_invocation: target_confirmed,
                    authorization: &authorization,
                },
            )
            .await?
        } else {
            let manager = self
                .sessions
                .read()
                .expect("Device sessions lock")
                .clone()
                .ok_or_else(|| {
                    SyncTransferPlanError::Remote("Device network is unavailable".into())
                })?;
            send_transfer_stream(
                &manager,
                TransferStreamSendRequest {
                    operation_id: context.operation_id.clone(),
                    source_device_id: self.local_device_id.clone(),
                    target_device_id: destination.context.call.target_device_id.clone(),
                    manifest,
                    target_dir: destination.target_dir.clone(),
                    conflict_policy: conflict(&context.call.arguments)
                        .map_err(SyncTransferPlanError::Remote)?,
                    options: TransferExecutionOptions::default(),
                    chat_context: Some(destination.context.clone()),
                },
                cancel,
                &mut on_progress,
            )
            .await?
        };
        if outcome.files.len() != destination.paths.len() {
            return Err(SyncTransferPlanError::OutcomeUnknown {
                operation_id: context.operation_id.clone(),
                detail: "Completion receipt did not cover the prepared file collection".into(),
            });
        }
        for (receipt, public) in outcome.files.iter_mut().zip(&destination.paths) {
            receipt.target_path = PathBuf::from(public);
        }
        Ok(outcome)
    }
}

fn same_invocation(left: &ChatTransferContext, right: &ChatTransferContext) -> bool {
    let mut left = left.clone();
    let mut right = right.clone();
    left.call.confirmation = None;
    right.call.confirmation = None;
    left == right
}

fn public_path(fs: &rua_tool::WorkspaceFs, native: &Path) -> Result<String, String> {
    fs.public_path_from_host(native)
        .and_then(|path| path.to_str().map(str::to_owned))
        .ok_or_else(|| "Transfer path has no public filesystem representation".into())
}

fn conflict(args: &serde_json::Value) -> Result<ReceiveConflictPolicy, String> {
    match args["conflict"].as_str().unwrap_or("rename") {
        "rename" => Ok(ReceiveConflictPolicy::Rename),
        "fail" => Ok(ReceiveConflictPolicy::Fail),
        "overwrite" => Ok(ReceiveConflictPolicy::Overwrite),
        _ => Err("Invalid transfer conflict policy".into()),
    }
}

fn policy(response: DeviceReadCheckReply) -> ChatTransferResponse {
    ChatTransferResponse::Policy { response }
}

fn bounded_control(value: &impl Serialize) -> Result<(), String> {
    if serde_json::to_vec(value)
        .map_err(|error| error.to_string())?
        .len()
        > 128 * 1024
    {
        return Err(
            "Transfer metadata exceeds the control message budget; select fewer files".into(),
        );
    }
    Ok(())
}

fn rejection_reply(rejection: TargetReadRejection) -> DeviceReadCheckReply {
    match rejection {
        TargetReadRejection::Denied => DeviceReadCheckReply::Denied,
        TargetReadRejection::Unavailable | TargetReadRejection::FileUnavailable => {
            DeviceReadCheckReply::Unavailable
        }
    }
}

#[async_trait::async_trait]
impl ChatTransferAuthorization for RuntimeChatTransferAuthorization {
    async fn check_receive(
        &self,
        context: &ChatTransferContext,
        target_path: &Path,
        confirmed_invocation: bool,
    ) -> Result<(), SyncTransferPlanError> {
        let fs = rua_tool::WorkspaceFs::for_execution(
            &self.rua_root,
            rua_core::runtime_layout::RuntimeLayout::from_current_process(None),
        )
        .map_err(|error| SyncTransferPlanError::Remote(error.to_string()))?;
        let path = fs
            .public_path_from_host(target_path)
            .and_then(|path| path.to_str().map(str::to_owned))
            .ok_or_else(|| {
                SyncTransferPlanError::Remote("The target has no public filesystem path".into())
            })?;
        let requested_dir = context.call.arguments["target_dir"]
            .as_str()
            .ok_or_else(|| {
                SyncTransferPlanError::Remote("Missing original transfer destination".into())
            })?;
        let native_root = fs
            .resolve(requested_dir)
            .map_err(|error| SyncTransferPlanError::Remote(error.to_string()))?;
        let suffix = target_path.strip_prefix(native_root).map_err(|_| {
            SyncTransferPlanError::Remote(
                "Transfer destination escaped its selected directory".into(),
            )
        })?;
        let requested = Path::new(requested_dir).join(suffix);
        let requested = requested.to_str().ok_or_else(|| {
            SyncTransferPlanError::Remote("Invalid public destination path".into())
        })?;
        match self
            .target
            .inspect_transfer_target(
                &context.call.as_target_read(),
                requested,
                &path,
                confirmed_invocation,
            )
            .await
        {
            Ok(FileReadAdmission::Allowed) => Ok(()),
            _ => Err(SyncTransferPlanError::Remote(
                "The target path is no longer authorized for this transfer".into(),
            )),
        }
    }
}
