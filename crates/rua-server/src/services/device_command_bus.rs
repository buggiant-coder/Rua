use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rua_acp::permissions::{
    AcpPermissionRelay, AcpPermissionRelayError, AcpPermissionRequest, AcpPermissionSnapshot,
};
use rua_config::types::acp::{default_builtin_runners, AcpAgentConfig, ToolParseMode};
use rua_core::{ChatAddress, ChatScopeStore};
use rua_security::approval_authority::ApprovalReuseMatcher;

use crate::services::chat_metadata_store::{AcpBindingMetadata, ChatMetadataRecord};
use crate::services::console_chat::CONSOLE_CHANNEL_ID;
use crate::services::conversation_authority::{
    ConversationAuthority, ConversationControlView, ConversationInputAcceptanceMode,
    ConversationInputEntrySnapshot,
};
use crate::services::conversation_authority_placement::{
    chat_acp_binding_active, ConversationAuthorityEpoch, ConversationAuthorityPlacement,
};
use crate::services::conversation_history_readiness::{
    ensure_conversation_history_ready, ensure_conversation_history_ready_via_device_session,
    ConversationHistoryGapRequest, ConversationHistoryReadinessRequest,
};
use crate::services::conversation_input_intake::ConversationInputIntake;
use crate::services::conversation_manifest::{load_conversation_manifest, ConversationManifest};
use crate::services::terminal::{
    OpenTerminalSessionRequest, TerminalActivityFrame, TerminalError, TerminalProfileList,
    TerminalPtyGeometry, TerminalSessionAttachment, TerminalSessionAuthority,
    TerminalSessionSnapshot, TerminalSnapshot, TerminalStreamCursor, TerminalViewerIdentity,
};
use crate::services::user_notification::{UserNotificationService, UserNotificationWireEnvelope};
use crate::streaming::ActiveStreamRegistry;

pub use crate::services::conversation_history_readiness::ConversationHistorySyncRequest;

const COMMAND_FRAME_LIMIT: usize = 1024 * 1024;
const TERMINAL_ATTACHMENT_CHUNK_BYTES: usize = 512 * 1024;
const TERMINAL_ATTACHMENT_TRANSFER_TTL: Duration = Duration::from_secs(60);
const TERMINAL_ATTACHMENT_TRANSFER_LIMIT: usize = 8;
const COMMAND_RESULT_CACHE_LIMIT: usize = 1024;
const REMOTE_CHAT_CONTROL_CONFIRM_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteConversationInputRequest {
    pub address: ChatAddress,
    pub agent_id: String,
    pub source_device_id: String,
    pub target_device_id: String,
    pub authority_epoch: ConversationAuthorityEpoch,
    pub base_manifest_hash: String,
    pub submission_id: String,
    pub device_delivery_id: String,
    pub device_delivery_sequence: u64,
    pub text: String,
    #[serde(default)]
    pub input: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    pub media_requirements: Vec<RemoteConversationInputMediaRequirement>,
    pub created_at: String,
    pub delivery_deadline_at: String,
    #[serde(default)]
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteConversationInputMediaRequirement {
    pub mime_type: String,
    pub byte_size: u64,
    pub sha256: String,
    pub agent_id: String,
    pub media_ref: String,
    pub filename: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpChatDeviceCommand {
    pub command_id: String,
    pub agent_id: String,
    pub address: ChatAddress,
    pub source_device_id: String,
    pub target_device_id: String,
    pub authority_epoch: ConversationAuthorityEpoch,
    pub action: AcpChatDeviceAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum AcpChatDeviceAction {
    ListRunners,
    Enter {
        runner: String,
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default)]
        resume_session_id: Option<String>,
    },
    Exit,
    Status,
    ListSessions {
        runner: String,
        #[serde(default)]
        cwd: Option<String>,
    },
    PermissionSnapshot,
    DecidePermission {
        request_id: String,
        option_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteChatControlCommand {
    pub command_id: String,
    pub agent_id: String,
    pub address: ChatAddress,
    pub source_device_id: String,
    pub target_device_id: String,
    pub authority_epoch: ConversationAuthorityEpoch,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_delivery_id: Option<String>,
    #[serde(default)]
    pub await_terminal: bool,
    pub action: RemoteChatControlAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteChatControlAction {
    StopCurrentTurn,
    InterjectCurrentTurn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteChatControlResponse {
    pub command_id: String,
    pub target_device_id: String,
    pub address: ChatAddress,
    pub action: RemoteChatControlAction,
    pub applied: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationControlDeviceCommand {
    pub command_id: String,
    pub agent_id: String,
    pub address: ChatAddress,
    pub source_device_id: String,
    pub target_device_id: String,
    pub authority_epoch: ConversationAuthorityEpoch,
    pub action: ConversationControlDeviceAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ConversationControlDeviceAction {
    DeliverBackgroundResult {
        note: serde_json::Value,
    },
    PauseQueuedInputs {
        authority_epoch: ConversationAuthorityEpoch,
    },
    StopTurn {
        turn_id: String,
        authority_epoch: ConversationAuthorityEpoch,
    },
    Discover,
    View,
    ViewRunningConfig,
    ViewConfiguration,
    UpdateConfiguration {
        expected_epoch: String,
        candidate_epoch: String,
    },
    ViewProjectionExclusions,
    ViewResourcePolicy,
    UpdateModel {
        model: serde_json::Value,
    },
    UpdateRunningConfig {
        running: serde_json::Value,
    },
    UpdateProjectionExclusions {
        message_ids: Vec<String>,
    },
    SelectVariant {
        turn_id: String,
        selected_variant_id: String,
    },
    RollbackBeforeTurn {
        before_turn_id: String,
    },
    BranchBeforeTurn {
        before_turn_id: String,
    },
    EditQueuedInput {
        input_id: String,
        message: String,
    },
    DeleteQueuedInput {
        input_id: String,
    },
    SendNow {
        input_id: String,
        control_command_id: String,
    },
    ApprovalSnapshot,
    DecideApproval {
        approval_id: String,
        decision: ConversationApprovalDecision,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum ConversationApprovalDecision {
    RunOnce {
        decided_by: String,
    },
    AllowChat {
        decided_by: String,
        matcher: ApprovalReuseMatcher,
    },
    AllowAgent {
        decided_by: String,
        matcher: ApprovalReuseMatcher,
    },
    Reject {
        decided_by: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConversationControlDeviceResponse {
    BackgroundResultDelivered {
        observation_id: String,
    },
    StopRequested {
        applied: bool,
    },
    View {
        control: ConversationControlView,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        execution_agent_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        authority_placement: Option<ConversationAuthorityPlacement>,
    },
    RunningConfig {
        running: serde_json::Value,
    },
    Configuration {
        configuration: serde_json::Value,
    },
    ProjectionExclusions {
        message_ids: Vec<String>,
    },
    ResourcePolicy {
        workspace: ProjectResourcePolicyView,
    },
    ModelUpdated {
        model: serde_json::Value,
        control: ConversationControlView,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        chat: Option<serde_json::Value>,
    },
    RunningConfigUpdated {
        running: serde_json::Value,
        control: ConversationControlView,
    },
    ProjectionExclusionsUpdated {
        message_ids: Vec<String>,
    },
    VariantSelected {
        turn_id: String,
        selected_variant_id: String,
    },
    RolledBack {
        chat: serde_json::Value,
        cancelled_tasks: usize,
    },
    Branched {
        chat: serde_json::Value,
    },
    Edited {
        entry: ConversationInputEntrySnapshot,
        control: ConversationControlView,
    },
    Deleted {
        deleted_input_id: String,
        control: ConversationControlView,
    },
    SendNow {
        input_id: String,
        applied: bool,
        control: ConversationControlView,
    },
    ApprovalSnapshot {
        snapshot: serde_json::Value,
    },
    ApprovalDecided {
        request: serde_json::Value,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectResourcePolicyView {
    pub policy: rua_security::EffectiveProjectResourcePolicy,
    pub pending_approval_count: usize,
    pub chat_rule_count: usize,
    pub agent_rule_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationManifestDeviceCommand {
    pub command_id: String,
    pub agent_id: String,
    pub address: ChatAddress,
    pub source_device_id: String,
    pub target_device_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_manifest: Option<ConversationManifest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationManifestDeviceResponse {
    pub command_id: String,
    pub target_device_id: String,
    pub address: ChatAddress,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<ConversationManifest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationAuthorityAdoptDeviceCommand {
    pub command_id: String,
    pub agent_id: String,
    pub address: ChatAddress,
    pub source_device_id: String,
    pub authority_source_device_id: String,
    pub target_device_id: String,
    pub expected_authority_epoch: ConversationAuthorityEpoch,
    pub required_manifest: ConversationManifest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationAuthorityAdoptDeviceResponse {
    pub command_id: String,
    pub target_device_id: String,
    pub address: ChatAddress,
    pub placement: ConversationAuthorityPlacement,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationAuthorityRedirect {
    pub address: ChatAddress,
    pub placement: ConversationAuthorityPlacement,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationAuthoritySupersedeDeviceCommand {
    pub command_id: String,
    pub agent_id: String,
    pub address: ChatAddress,
    pub source_device_id: String,
    pub target_device_id: String,
    pub previous_authority_epoch: ConversationAuthorityEpoch,
    pub replacement: ConversationAuthorityPlacement,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationAuthoritySupersedeDeviceResponse {
    pub command_id: String,
    pub target_device_id: String,
    pub address: ChatAddress,
    pub placement: ConversationAuthorityPlacement,
    #[serde(default)]
    pub superseded_input_ids: Vec<String>,
}

macro_rules! impl_chat_address_accessors {
    ($($type:ty),+ $(,)?) => {
        $(
            impl $type {
                pub fn chat_id(&self) -> &str {
                    self.address.chat_id().as_str()
                }
            }
        )+
    };
}

impl_chat_address_accessors!(
    RemoteConversationInputRequest,
    AcpChatDeviceCommand,
    RemoteChatControlCommand,
    RemoteChatControlResponse,
    ConversationControlDeviceCommand,
    ConversationManifestDeviceCommand,
    ConversationManifestDeviceResponse,
    ConversationAuthorityAdoptDeviceCommand,
    ConversationAuthorityAdoptDeviceResponse,
    ConversationAuthorityRedirect,
    ConversationAuthoritySupersedeDeviceCommand,
    ConversationAuthoritySupersedeDeviceResponse,
);

impl ConversationAuthorityRedirect {
    pub fn agent_id(&self) -> &str {
        self.address
            .agent_id()
            .expect("Agent authority redirect requires Agent Scope")
            .as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalDeviceCommand {
    pub command_id: String,
    pub source_device_id: String,
    pub target_device_id: String,
    pub action: TerminalDeviceAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum TerminalDeviceAction {
    ListSessions,
    ListProfiles,
    Open {
        browser_viewer_id: String,
        scrollback_lines: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        terminal_profile_id: Option<String>,
    },
    Attach {
        session_id: String,
        browser_viewer_id: String,
        scrollback_lines: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cursor: Option<TerminalStreamCursor>,
    },
    AttachmentChunk {
        transfer_id: String,
        offset_bytes: usize,
    },
    SetScrollback {
        session_id: String,
        scrollback_lines: usize,
    },
    Rename {
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display_name: Option<String>,
    },
    TakeInputLease {
        session_id: String,
        browser_viewer_id: String,
    },
    SendInput {
        session_id: String,
        browser_viewer_id: String,
        data: Vec<u8>,
    },
    AdaptPtyGeometry {
        session_id: String,
        browser_viewer_id: String,
        cols: u16,
        rows: u16,
    },
    UpdatePalette {
        session_id: String,
        browser_viewer_id: String,
        palette: crate::services::terminal::TerminalPalette,
    },
    DetachViewer {
        session_id: String,
        browser_viewer_id: String,
    },
    KillSession {
        session_id: String,
        browser_viewer_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TerminalDeviceResponse {
    Sessions {
        sessions: Vec<TerminalSessionSnapshot>,
    },
    Profiles {
        profiles: TerminalProfileList,
    },
    Opened {
        session: TerminalSessionSnapshot,
    },
    Attached {
        session: TerminalSessionSnapshot,
        snapshot: TerminalSnapshot,
        tail: Vec<TerminalActivityFrame>,
        live_cursor: TerminalStreamCursor,
    },
    AttachedTransfer {
        session: TerminalSessionSnapshot,
        snapshot_cursor: TerminalStreamCursor,
        live_cursor: TerminalStreamCursor,
        transfer_id: String,
        total_bytes: usize,
    },
    AttachmentChunk {
        transfer_id: String,
        offset_bytes: usize,
        next_offset_bytes: usize,
        total_bytes: usize,
        data_base64: String,
        done: bool,
    },
    ScrollbackUpdated {
        session_id: String,
    },
    Renamed {
        session: TerminalSessionSnapshot,
    },
    InputLeaseTaken {
        session: TerminalSessionSnapshot,
    },
    PtyGeometryAdapted {
        session: TerminalSessionSnapshot,
    },
    PaletteUpdated {
        session_id: String,
    },
    InputAccepted {
        session_id: String,
    },
    ViewerDetached {
        session_id: String,
    },
    SessionKilled {
        session_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncTransferDeviceCommand {
    pub command_id: String,
    pub source_device_id: String,
    pub target_device_id: String,
    pub action: SyncTransferDeviceAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectDeviceCommand {
    pub command_id: String,
    pub source_device_id: String,
    pub target_device_id: String,
    pub action: ProjectDeviceAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum ProjectResourceOperation {
    Files { path: String },
    GitStatus,
    GitHistory { offset: usize, limit: usize },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ProjectDeviceAction {
    ArchiveIdleChats {
        project_id: rua_core::ProjectId,
        chat_ids: Option<Vec<String>>,
    },
    InspectResource {
        project_id: rua_core::ProjectId,
        directory_id: String,
        operation: ProjectResourceOperation,
    },
    CreateDirectory {
        parent: String,
        name: String,
    },
    BrowseDirectory {
        path: String,
    },
    Create {
        resource_root: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display_name: Option<String>,
    },
    CreateResources {
        resource_roots: Vec<String>,
        primary_index: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display_name: Option<String>,
    },
    EditResources {
        project_id: rua_core::ProjectId,
        expected_revision: u64,
        resource_roots: Vec<PathBuf>,
        primary_index: usize,
        #[serde(default)]
        directory_ids: Vec<Option<String>>,
    },
    SetPrimaryDirectory {
        project_id: rua_core::ProjectId,
        expected_revision: u64,
        directory_id: String,
    },
    InspectGit {
        project_id: rua_core::ProjectId,
    },
    InspectGitHistory {
        project_id: rua_core::ProjectId,
        offset: usize,
        limit: usize,
    },
    InspectLifecycle {
        project_id: rua_core::ProjectId,
    },
    Archive {
        project_id: rua_core::ProjectId,
    },
    Reopen {
        project_id: rua_core::ProjectId,
    },
    SwitchChatAgent {
        project_id: rua_core::ProjectId,
        chat_id: String,
        expected_agent_id: String,
        agent_id: String,
    },
    UpdateChat {
        project_id: rua_core::ProjectId,
        chat_id: String,
        update: crate::services::project_chat::ProjectChatUpdate,
    },
    DeleteChat {
        project_id: rua_core::ProjectId,
        chat_id: String,
        force: bool,
    },
    ListTasks {
        project_id: rua_core::ProjectId,
        chat_id: String,
        filter: rua_task::TaskListFilter,
    },
    GetTask {
        project_id: rua_core::ProjectId,
        chat_id: String,
        task_id: String,
    },
    SendTaskInput {
        project_id: rua_core::ProjectId,
        chat_id: String,
        task_id: String,
        submission: rua_task::TaskInputSubmission,
    },
    WatchTask {
        project_id: rua_core::ProjectId,
        chat_id: String,
        task_id: String,
    },
    UnwatchTask {
        project_id: rua_core::ProjectId,
        chat_id: String,
        task_id: String,
    },
    CancelTask {
        project_id: rua_core::ProjectId,
        chat_id: String,
        task_id: String,
    },
    RetryTask {
        project_id: rua_core::ProjectId,
        chat_id: String,
        task_id: String,
        authorization_agent_id: String,
    },
    GetTaskResult {
        project_id: rua_core::ProjectId,
        chat_id: String,
        task_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProjectDeviceResponse {
    ChatsArchived {
        result: crate::services::project_chat::ProjectBulkArchiveResult,
    },
    Directory {
        device_id: String,
        path: PathBuf,
        entries: Vec<rua_project::ProjectDirectoryEntry>,
        roots: Vec<rua_project::ProjectDirectoryEntry>,
    },
    Created {
        outcome: rua_project::ProjectCreationOutcome,
    },
    Duplicate {
        owner: rua_project::ProjectDescriptor,
    },
    ResourcesEdited {
        outcome: rua_project::ProjectCreationOutcome,
    },
    ResourcesUpdated {
        project: rua_project::ProjectDescriptor,
    },
    GitStatus {
        status: rua_project::ProjectGitStatus,
    },
    GitHistory {
        history: rua_project::ProjectGitHistoryPage,
    },
    LifecyclePreview {
        preview: crate::services::project_lifecycle::ProjectLifecyclePreview,
    },
    Archived {
        result: crate::services::project_lifecycle::ProjectArchiveResult,
    },
    Reopened {
        result: crate::services::project_lifecycle::ProjectReopenResult,
    },
    ChatAgentSelection {
        state: crate::services::project_chat::ProjectChatAgentState,
    },
    ChatUpdated {
        chat: serde_json::Value,
    },
    ChatDeleted {
        result: Result<
            crate::services::project_chat::ProjectChatDeleteResult,
            crate::services::project_chat::ProjectChatDeleteError,
        >,
    },
    TaskList {
        tasks: serde_json::Value,
    },
    Task {
        task: serde_json::Value,
    },
    TaskInput {
        acceptance: rua_task::TaskInputAcceptance,
    },
    TaskWatch {
        status: rua_task::TaskWatchStatus,
    },
    TaskResult {
        result: serde_json::Value,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum SyncTransferDeviceAction {
    ListDir {
        path: String,
    },
    StartSend {
        operation_id: String,
        source_paths: Vec<String>,
        transfer_target_device_id: String,
        target_dir: String,
        conflict: SyncTransferConflictPolicy,
    },
    StartChatGapSend {
        operation_id: String,
        agent_id: String,
        chat_id: String,
        expected_authority_epoch: ConversationAuthorityEpoch,
        required_manifest: Box<ConversationManifest>,
        transfer_target_device_id: String,
        target_dir: String,
        target_inventory: Vec<SyncTransferFileFingerprint>,
    },
    GetSendStatus {
        operation_id: String,
    },
    CancelSend {
        operation_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SyncTransferDeviceResponse {
    DirEntries {
        device_id: String,
        path: PathBuf,
        entries: Vec<SyncTransferDeviceEntry>,
    },
    TransferOperation {
        operation: crate::services::sync_transfer::SourceTransferOperationSnapshot,
    },
    ChatGapTransferOperation {
        operation: crate::services::sync_transfer::SourceTransferOperationSnapshot,
        source_chat: Box<ChatMetadataRecord>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncTransferFileFingerprint {
    pub relative_path: PathBuf,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncTransferConflictPolicy {
    Rename,
    Fail,
    Overwrite,
}

impl From<SyncTransferConflictPolicy> for crate::services::sync_transfer::ReceiveConflictPolicy {
    fn from(value: SyncTransferConflictPolicy) -> Self {
        match value {
            SyncTransferConflictPolicy::Rename => Self::Rename,
            SyncTransferConflictPolicy::Fail => Self::Fail,
            SyncTransferConflictPolicy::Overwrite => Self::Overwrite,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncTransferDirectoryListing {
    pub path: PathBuf,
    pub entries: Vec<SyncTransferDeviceEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncTransferDeviceEntry {
    pub name: String,
    pub path: PathBuf,
    pub kind: SyncTransferDeviceEntryKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncTransferDeviceEntryKind {
    Directory,
    File,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpChatDeviceStatus {
    pub active: bool,
    #[serde(default)]
    pub runner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    pub chat_id: String,
    pub workspace_dir: String,
    pub target_device_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpChatDeviceSessionEntry {
    pub session_id: String,
    #[serde(default)]
    pub title: Option<String>,
    pub cwd: String,
    #[serde(default)]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpChatRunnerInfo {
    pub key: String,
    pub enabled: bool,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    pub trusted: bool,
    pub tool_parse_mode: ToolParseMode,
    #[serde(default)]
    pub stdio_buffer_limit_bytes: Option<u64>,
    pub is_builtin: bool,
    #[serde(default, flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AcpChatDeviceResponse {
    Runners {
        runners: Vec<AcpChatRunnerInfo>,
    },
    Entered {
        ok: bool,
        message: String,
        status: AcpChatDeviceStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        history_preview: Option<String>,
    },
    Exited {
        ok: bool,
        message: String,
    },
    Status {
        status: AcpChatDeviceStatus,
    },
    Sessions {
        sessions: Vec<AcpChatDeviceSessionEntry>,
    },
    PermissionSnapshot {
        snapshot: AcpPermissionSnapshot,
    },
    PermissionDecided {
        request: AcpPermissionRequest,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteConversationInputAck {
    pub submission_id: String,
    pub input_id: String,
    pub mode: ConversationInputAcceptanceMode,
    pub device_delivery_id: String,
    pub target_device_id: String,
    pub accepted_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control: Option<Box<ConversationControlView>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMediaPrepareCommand {
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub first_submission: bool,
    pub command_id: String,
    pub operation_id: String,
    pub address: ChatAddress,
    pub source_device_id: String,
    pub target_device_id: String,
    pub authority_epoch: ConversationAuthorityEpoch,
    pub media_id: String,
    pub byte_size: u64,
    pub mime_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMediaPreparedResponse {
    pub already_present: bool,
    pub operation_id: String,
    pub media_id: String,
    pub media_ref: String,
    pub authority_epoch: ConversationAuthorityEpoch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CollaborationDeviceCommand {
    pub command_id: String,
    pub source_device_id: String,
    pub target_device_id: String,
    pub route_address: ChatAddress,
    pub authority_epoch: ConversationAuthorityEpoch,
    pub action: CollaborationDeviceAction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum CollaborationDeviceAction {
    CreateAgent {
        caller: crate::services::collaboration::CollaborationCaller,
        agent_id: String,
        message: Option<String>,
        yield_seconds: u64,
    },
    CreateAcp {
        caller: crate::services::collaboration::CollaborationCaller,
        runner: String,
        message: Option<String>,
        yield_seconds: u64,
    },
    Send {
        caller: crate::services::collaboration::CollaborationCaller,
        target: ChatAddress,
        message: String,
        yield_seconds: u64,
    },
    Read {
        caller: crate::services::collaboration::CollaborationCaller,
        target: ChatAddress,
        yield_seconds: u64,
        #[serde(default)]
        result: Option<rua_session::collaboration::CollaborationResultCursor>,
    },
    StopTurn {
        caller: crate::services::collaboration::CollaborationCaller,
        target: ChatAddress,
        turn_id: String,
        turn_authority_epoch: ConversationAuthorityEpoch,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum CollaborationDeviceResponse {
    Reply {
        reply: Box<crate::services::collaboration::CollaborationReply>,
        authority_device_id: String,
        authority_epoch: ConversationAuthorityEpoch,
    },
    Stopped {
        reply: crate::services::collaboration::CollaborationStopReply,
        authority_device_id: String,
        authority_epoch: ConversationAuthorityEpoch,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", content = "payload", rename_all = "snake_case")]
pub(crate) enum DeviceCommandEnvelope {
    ConversationInput(Box<RemoteConversationInputRequest>),
    AcpChat(AcpChatDeviceCommand),
    RemoteChatControl(RemoteChatControlCommand),
    ConversationControl(ConversationControlDeviceCommand),
    ConversationManifest(ConversationManifestDeviceCommand),
    ConversationAuthorityAdopt(ConversationAuthorityAdoptDeviceCommand),
    ConversationAuthoritySupersede(ConversationAuthoritySupersedeDeviceCommand),
    Terminal(TerminalDeviceCommand),
    SyncTransfer(SyncTransferDeviceCommand),
    ChatMediaPrepare(ChatMediaPrepareCommand),
    Project(ProjectDeviceCommand),
    Collaboration(CollaborationDeviceCommand),
}

impl DeviceCommandEnvelope {
    fn idempotency_key(&self) -> Option<String> {
        let (kind, id) = match self {
            Self::ConversationInput(request) => {
                ("conversation_input", request.submission_id.as_str())
            }
            Self::AcpChat(command) => ("acp_chat", command.command_id.as_str()),
            Self::RemoteChatControl(command) => {
                ("remote_chat_control", command.command_id.as_str())
            }
            Self::ConversationControl(command) => {
                ("conversation_control", command.command_id.as_str())
            }
            Self::ConversationManifest(command) => {
                ("conversation_manifest", command.command_id.as_str())
            }
            Self::ConversationAuthorityAdopt(command) => {
                ("conversation_authority_adopt", command.command_id.as_str())
            }
            Self::ConversationAuthoritySupersede(command) => (
                "conversation_authority_supersede",
                command.command_id.as_str(),
            ),
            Self::Terminal(command) => ("terminal", command.command_id.as_str()),
            Self::SyncTransfer(command) => ("sync_transfer", command.command_id.as_str()),
            Self::ChatMediaPrepare(command) => ("chat_media_prepare", command.command_id.as_str()),
            Self::Project(command) => ("project", command.command_id.as_str()),
            Self::Collaboration(command) => ("collaboration", command.command_id.as_str()),
        };
        (!id.trim().is_empty()).then(|| format!("{kind}:{id}"))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DeviceCommandResponse {
    Accepted(RemoteConversationInputAck),
    AcpChat {
        response: AcpChatDeviceResponse,
    },
    RemoteChatControl {
        response: RemoteChatControlResponse,
    },
    ConversationControl {
        response: Box<ConversationControlDeviceResponse>,
    },
    ConversationManifest {
        response: ConversationManifestDeviceResponse,
    },
    ConversationAuthorityAdopt {
        response: ConversationAuthorityAdoptDeviceResponse,
    },
    ConversationAuthoritySuperseded {
        response: ConversationAuthoritySupersedeDeviceResponse,
    },
    AuthorityRedirect {
        redirect: ConversationAuthorityRedirect,
    },
    Terminal {
        response: TerminalDeviceResponse,
    },
    SyncTransfer {
        response: SyncTransferDeviceResponse,
    },
    ChatMediaPrepared {
        response: ChatMediaPreparedResponse,
    },
    Project {
        response: ProjectDeviceResponse,
    },
    Collaboration {
        response: CollaborationDeviceResponse,
    },
    Rejected {
        reason: String,
        detail: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TerminalAttachmentTransferPayload {
    snapshot: TerminalSnapshot,
    tail: Vec<TerminalActivityFrame>,
}

#[derive(Debug)]
struct PendingTerminalAttachmentTransfer {
    payload: Vec<u8>,
    created_at: Instant,
}

#[derive(Debug, Default)]
struct CommandResultCache {
    responses: HashMap<String, CachedCommandResult>,
    order: VecDeque<String>,
}

#[derive(Debug)]
struct CachedCommandResult {
    request: Vec<u8>,
    response: DeviceCommandResponse,
}

impl CommandResultCache {
    fn get(&self, key: &str, request: &[u8]) -> Result<Option<DeviceCommandResponse>, ()> {
        let Some(cached) = self.responses.get(key) else {
            return Ok(None);
        };
        if cached.request != request {
            return Err(());
        }
        Ok(Some(cached.response.clone()))
    }

    fn insert(&mut self, key: String, request: Vec<u8>, response: DeviceCommandResponse) {
        if let Some(existing) = self.responses.get_mut(&key) {
            *existing = CachedCommandResult { request, response };
            return;
        }
        while self.responses.len() >= COMMAND_RESULT_CACHE_LIMIT {
            let Some(evicted) = self.order.pop_front() else {
                self.responses.clear();
                break;
            };
            self.responses.remove(&evicted);
        }
        self.order.push_back(key.clone());
        self.responses
            .insert(key, CachedCommandResult { request, response });
    }
}

#[derive(Debug)]
pub enum DeviceCommandBusError {
    InvalidId(String),
    InvalidSequence,
    WrongTarget { target: String, local: String },
    Io(std::io::Error),
}

#[derive(Debug)]
pub enum DeviceCommandDeliveryError {
    Timeout,
    Connect(String),
    Json(serde_json::Error),
    Session(rua_device_network::DeviceSessionError),
    AuthorityRedirect(Box<ConversationAuthorityRedirect>),
    Rejected { reason: String, detail: String },
}

impl DeviceCommandDeliveryError {
    pub fn is_session_version_incompatible(&self) -> bool {
        matches!(
            self,
            Self::Session(error) if error.is_version_incompatible()
        )
    }

    pub fn failure_kind(&self) -> &'static str {
        match self {
            Self::Rejected { .. } => "business",
            Self::AuthorityRedirect(_) => "authority_redirect",
            Self::Session(error) if error.is_version_incompatible() => "version_incompatible",
            Self::Json(_) => "protocol_error",
            Self::Timeout | Self::Connect(_) | Self::Session(_) => "device_offline",
        }
    }
}

impl std::fmt::Display for DeviceCommandDeliveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => write!(f, "device command delivery timed out"),
            Self::Connect(error) => write!(f, "connect: {error}"),
            Self::Json(error) => write!(f, "json: {error}"),
            Self::Session(error) => write!(f, "{error}"),
            Self::AuthorityRedirect(redirect) => write!(
                f,
                "conversation authority redirected to {} at Epoch {}",
                redirect.placement.device_id, redirect.placement.authority_epoch
            ),
            Self::Rejected { reason, detail } => write!(f, "{reason}: {detail}"),
        }
    }
}

impl std::error::Error for DeviceCommandDeliveryError {}

impl From<serde_json::Error> for DeviceCommandDeliveryError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<rua_device_network::DeviceSessionError> for DeviceCommandDeliveryError {
    fn from(error: rua_device_network::DeviceSessionError) -> Self {
        Self::Session(error)
    }
}

impl std::fmt::Display for DeviceCommandBusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidId(id) => write!(f, "invalid device command id: {id}"),
            Self::InvalidSequence => {
                write!(f, "device delivery sequence must be greater than zero")
            }
            Self::WrongTarget { target, local } => write!(
                f,
                "device command target {target} does not match this device {local}"
            ),
            Self::Io(error) => write!(f, "io: {error}"),
        }
    }
}

impl std::error::Error for DeviceCommandBusError {}

impl From<std::io::Error> for DeviceCommandBusError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Clone)]
pub struct DeviceCommandHandler {
    local_device_id: String,
    data_dir: Option<PathBuf>,
    remote_history_sync_tx:
        Option<tokio::sync::mpsc::UnboundedSender<ConversationHistorySyncRequest>>,
    acp_chat_registry: Option<Arc<crate::services::acp_chat_registry::AcpChatRegistry>>,
    acp_permission_relay: Option<Arc<AcpPermissionRelay>>,
    workspace_registry: Option<Arc<dyn rua_workspace::WorkspaceRegistry>>,
    remote_chat_control_streams: Option<Arc<ActiveStreamRegistry>>,
    conversation_input_intake: Option<ConversationInputIntake>,
    conversation_authority: Option<Arc<ConversationAuthority>>,
    collaboration: Option<
        Arc<
            std::sync::OnceLock<
                std::sync::Weak<crate::services::collaboration::CollaborationService>,
            >,
        >,
    >,
    terminal_authority: Option<Arc<TerminalSessionAuthority>>,
    terminal_attachment_transfers: Arc<
        tokio::sync::Mutex<std::collections::HashMap<String, PendingTerminalAttachmentTransfer>>,
    >,
    command_results: Arc<tokio::sync::Mutex<CommandResultCache>>,
    command_locks: Arc<tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
    sync_transfer_session_manager:
        Option<Arc<std::sync::RwLock<Option<rua_device_network::DeviceSessionManager>>>>,
    chat_media_transfers:
        Option<Arc<crate::services::chat_media_transfer::ChatMediaTransferRegistry>>,
    sync_transfer_operations: Arc<crate::services::sync_transfer::SourceTransferOperationStore>,
    project_catalog: Option<Arc<rua_project::ProjectCatalog>>,
    project_bindings: Option<Arc<rua_project::ProjectResourceBindingStore>>,
    project_publication: Option<crate::sync_publication::SyncPublicationRuntime>,
    project_resource_editor: Option<crate::services::project_lifecycle::ProjectResourceEditor>,
    task_runtime: Option<Arc<rua_task::TaskRuntime>>,
    user_notifications: Option<Arc<UserNotificationService>>,
}

impl std::fmt::Debug for DeviceCommandHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceCommandHandler")
            .field("local_device_id", &self.local_device_id)
            .field("data_dir", &self.data_dir)
            .field(
                "has_remote_history_sync",
                &self.remote_history_sync_tx.is_some(),
            )
            .field("has_acp_chat_registry", &self.acp_chat_registry.is_some())
            .field(
                "has_acp_permission_relay",
                &self.acp_permission_relay.is_some(),
            )
            .field("has_workspace_registry", &self.workspace_registry.is_some())
            .field(
                "has_remote_chat_control_streams",
                &self.remote_chat_control_streams.is_some(),
            )
            .field(
                "has_conversation_input_intake",
                &self.conversation_input_intake.is_some(),
            )
            .field(
                "has_conversation_authority",
                &self.conversation_authority.is_some(),
            )
            .field("has_terminal_authority", &self.terminal_authority.is_some())
            .field(
                "terminal_attachment_transfer_count",
                &self
                    .terminal_attachment_transfers
                    .try_lock()
                    .map(|transfers| transfers.len())
                    .unwrap_or_default(),
            )
            .field(
                "command_result_count",
                &self
                    .command_results
                    .try_lock()
                    .map(|cache| cache.responses.len())
                    .unwrap_or_default(),
            )
            .field(
                "has_sync_transfer_session_manager",
                &self.sync_transfer_session_manager.is_some(),
            )
            .finish()
    }
}

impl DeviceCommandHandler {
    pub fn new(command_root: PathBuf, local_device_id: String) -> Self {
        let data_dir = command_root.parent().map(Path::to_path_buf);
        Self {
            local_device_id,
            data_dir,
            remote_history_sync_tx: None,
            acp_chat_registry: None,
            acp_permission_relay: None,
            workspace_registry: None,
            remote_chat_control_streams: None,
            conversation_input_intake: None,
            conversation_authority: None,
            collaboration: None,
            terminal_authority: None,
            terminal_attachment_transfers: Arc::new(tokio::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            command_results: Arc::new(tokio::sync::Mutex::new(CommandResultCache::default())),
            command_locks: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            sync_transfer_session_manager: None,
            chat_media_transfers: None,
            sync_transfer_operations: Arc::new(
                crate::services::sync_transfer::SourceTransferOperationStore::new(),
            ),
            project_catalog: None,
            project_bindings: None,
            project_publication: None,
            project_resource_editor: None,
            task_runtime: None,
            user_notifications: None,
        }
    }

    pub fn with_remote_history_sync(
        mut self,
        tx: tokio::sync::mpsc::UnboundedSender<ConversationHistorySyncRequest>,
    ) -> Self {
        self.remote_history_sync_tx = Some(tx);
        self
    }

    pub fn with_acp_chat_registry(
        mut self,
        registry: Arc<crate::services::acp_chat_registry::AcpChatRegistry>,
        workspace_registry: Arc<dyn rua_workspace::WorkspaceRegistry>,
    ) -> Self {
        self.acp_chat_registry = Some(registry);
        self.workspace_registry = Some(workspace_registry);
        self
    }

    pub fn with_acp_permission_relay(mut self, relay: Arc<AcpPermissionRelay>) -> Self {
        self.acp_permission_relay = Some(relay);
        self
    }

    pub fn with_remote_chat_control_streams(mut self, streams: Arc<ActiveStreamRegistry>) -> Self {
        self.remote_chat_control_streams = Some(streams);
        self
    }

    pub fn with_conversation_input_intake(mut self, intake: ConversationInputIntake) -> Self {
        self.conversation_input_intake = Some(intake);
        self
    }

    pub fn with_collaboration_binding(
        mut self,
        collaboration: Arc<
            std::sync::OnceLock<
                std::sync::Weak<crate::services::collaboration::CollaborationService>,
            >,
        >,
    ) -> Self {
        self.collaboration = Some(collaboration);
        self
    }

    pub fn with_conversation_authority(mut self, authority: Arc<ConversationAuthority>) -> Self {
        self.conversation_authority = Some(authority);
        self
    }

    pub fn with_terminal_authority(mut self, authority: Arc<TerminalSessionAuthority>) -> Self {
        self.terminal_authority = Some(authority);
        self
    }

    pub fn with_sync_transfer_session_manager(
        mut self,
        session_manager: Arc<std::sync::RwLock<Option<rua_device_network::DeviceSessionManager>>>,
    ) -> Self {
        self.sync_transfer_session_manager = Some(session_manager);
        self
    }

    pub fn with_chat_media_transfers(
        mut self,
        chat_media_transfers: Arc<crate::services::chat_media_transfer::ChatMediaTransferRegistry>,
    ) -> Self {
        self.chat_media_transfers = Some(chat_media_transfers);
        self
    }

    pub fn with_foreground_operations(
        mut self,
        foreground_operations: Arc<
            crate::services::foreground_operation::ForegroundOperationRegistry,
        >,
    ) -> Self {
        self.sync_transfer_operations = Arc::new(
            crate::services::sync_transfer::SourceTransferOperationStore::with_foreground_operations(
                foreground_operations,
            ),
        );
        self
    }

    pub fn with_project_resource_editor(
        mut self,
        editor: crate::services::project_lifecycle::ProjectResourceEditor,
    ) -> Self {
        self.project_resource_editor = Some(editor);
        self
    }

    pub fn with_project_runtime(
        mut self,
        catalog: Arc<rua_project::ProjectCatalog>,
        bindings: Arc<rua_project::ProjectResourceBindingStore>,
        publication: crate::sync_publication::SyncPublicationRuntime,
    ) -> Self {
        self.project_catalog = Some(catalog);
        self.project_bindings = Some(bindings);
        self.project_publication = Some(publication);
        self
    }

    pub fn with_task_runtime(mut self, task_runtime: Arc<rua_task::TaskRuntime>) -> Self {
        self.task_runtime = Some(task_runtime);
        self
    }

    pub fn with_user_notifications(
        mut self,
        user_notifications: Arc<UserNotificationService>,
    ) -> Self {
        self.user_notifications = Some(user_notifications);
        self
    }
}

#[async_trait::async_trait]
impl rua_device_network::DeviceSessionCommandHandler for DeviceCommandHandler {
    async fn handle_command(&self, payload: &[u8]) -> Result<Vec<u8>, String> {
        if let Ok(envelope) = serde_json::from_slice::<UserNotificationWireEnvelope>(payload) {
            if let (Some(service), Some(request)) = (
                self.user_notifications.as_ref(),
                envelope.into_current_request(),
            ) {
                service.receive(request).await;
            }
            return Ok(Vec::new());
        }
        serde_json::to_vec(&self.handle_frame(payload).await).map_err(|error| error.to_string())
    }
}

impl DeviceCommandHandler {
    async fn handle_frame(&self, bytes: &[u8]) -> DeviceCommandResponse {
        let envelope = match serde_json::from_slice::<DeviceCommandEnvelope>(bytes) {
            Ok(envelope) => envelope,
            Err(error) => {
                return DeviceCommandResponse::Rejected {
                    reason: "invalid_request".to_string(),
                    detail: error.to_string(),
                };
            }
        };
        let Some(key) = envelope.idempotency_key() else {
            return self.dispatch_envelope(envelope).await;
        };
        let command_lock = {
            let mut locks = self.command_locks.lock().await;
            Arc::clone(
                locks
                    .entry(key.clone())
                    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
            )
        };
        let command_guard = command_lock.lock().await;
        match self.command_results.lock().await.get(&key, bytes) {
            Ok(Some(response)) => {
                drop(command_guard);
                self.command_locks.lock().await.remove(&key);
                return response;
            }
            Ok(None) => {}
            Err(()) => {
                drop(command_guard);
                self.command_locks.lock().await.remove(&key);
                return DeviceCommandResponse::Rejected {
                    reason: "duplicate_command_conflict".to_string(),
                    detail: "command id was already used with a different payload".to_string(),
                };
            }
        }
        let dispatch = self.dispatch_envelope(envelope);
        let response = dispatch.await;
        self.command_results
            .lock()
            .await
            .insert(key.clone(), bytes.to_vec(), response.clone());
        drop(command_guard);
        self.command_locks.lock().await.remove(&key);
        response
    }

    async fn dispatch_envelope(&self, envelope: DeviceCommandEnvelope) -> DeviceCommandResponse {
        match envelope {
            DeviceCommandEnvelope::ConversationInput(request) => {
                self.handle_remote_conversation_input(*request).await
            }
            DeviceCommandEnvelope::AcpChat(command) => self.handle_acp_chat(command).await,
            DeviceCommandEnvelope::RemoteChatControl(command) => {
                self.handle_remote_chat_control(command).await
            }
            DeviceCommandEnvelope::ConversationControl(command) => {
                self.handle_conversation_control(command).await
            }
            DeviceCommandEnvelope::ConversationManifest(command) => {
                self.handle_conversation_manifest(command).await
            }
            DeviceCommandEnvelope::ConversationAuthorityAdopt(command) => {
                let future = self.handle_conversation_authority_adopt(command);
                future.await
            }
            DeviceCommandEnvelope::ConversationAuthoritySupersede(command) => {
                self.handle_conversation_authority_supersede(command).await
            }
            DeviceCommandEnvelope::Terminal(command) => self.handle_terminal(command).await,
            DeviceCommandEnvelope::SyncTransfer(command) => {
                self.handle_sync_transfer(command).await
            }
            DeviceCommandEnvelope::ChatMediaPrepare(command) => {
                self.handle_chat_media_prepare(command).await
            }
            DeviceCommandEnvelope::Project(command) => self.handle_project(command).await,
            DeviceCommandEnvelope::Collaboration(command) => {
                self.handle_collaboration(command).await
            }
        }
    }

    async fn handle_collaboration(
        &self,
        mut command: CollaborationDeviceCommand,
    ) -> DeviceCommandResponse {
        if command.target_device_id != self.local_device_id
            || command.command_id.trim().is_empty()
            || command.source_device_id.trim().is_empty()
        {
            return DeviceCommandResponse::Rejected {
                reason: "invalid_collaboration_command".into(),
                detail: "Collaboration command identity fields are invalid".into(),
            };
        }
        let Some(rua_root) = self.data_dir.as_deref() else {
            return DeviceCommandResponse::Rejected {
                reason: "collaboration_unavailable".into(),
                detail: "target Device has no Rua Root".into(),
            };
        };
        let scope = rua_core::ChatScopeStore::resolve(rua_root, command.route_address.clone());
        let placement = match crate::services::conversation_authority_target::load_optional_chat_authority_placement(
            scope.root(),
            command.route_address.chat_id().as_str(),
        )
        .await
        {
            Ok(Some(placement)) => placement,
            Ok(None) => return DeviceCommandResponse::Rejected {
                reason: "conversation_authority_missing".into(),
                detail: "Conversation Authority placement is not materialized".into(),
            },
            Err(error) => return DeviceCommandResponse::Rejected {
                reason: "conversation_authority_unavailable".into(),
                detail: error,
            },
        };
        if placement.device_id != self.local_device_id
            || placement.authority_epoch != command.authority_epoch
        {
            return DeviceCommandResponse::Rejected {
                reason: "conversation_authority_redirect".into(),
                detail: format!(
                    "Conversation Authority is on Device '{}' at epoch '{}'",
                    placement.device_id, placement.authority_epoch
                ),
            };
        }
        let Some(service) = self
            .collaboration
            .as_ref()
            .and_then(|binding| binding.get())
            .and_then(std::sync::Weak::upgrade)
        else {
            return DeviceCommandResponse::Rejected {
                reason: "collaboration_unavailable".into(),
                detail: "Collaboration service is unavailable".into(),
            };
        };
        let Some(intake) = self.conversation_input_intake.as_ref() else {
            return DeviceCommandResponse::Rejected {
                reason: "collaboration_unavailable".into(),
                detail: "Conversation input intake is unavailable".into(),
            };
        };
        let caller = match &mut command.action {
            CollaborationDeviceAction::CreateAgent { caller, .. }
            | CollaborationDeviceAction::CreateAcp { caller, .. }
            | CollaborationDeviceAction::Send { caller, .. }
            | CollaborationDeviceAction::Read { caller, .. }
            | CollaborationDeviceAction::StopTurn { caller, .. } => caller,
        };
        caller.working_dir = match intake
            .resolve_collaboration_working_dir(&caller.address, &self.local_device_id)
            .await
        {
            Ok(working_dir) => working_dir,
            Err(detail) => {
                return DeviceCommandResponse::Rejected {
                    reason: "collaboration_resource_unavailable".into(),
                    detail,
                }
            }
        };
        let response = match command.action {
            CollaborationDeviceAction::CreateAgent {
                caller,
                agent_id,
                message,
                yield_seconds,
            } => service
                .create(caller, agent_id, message, yield_seconds)
                .await
                .map(|reply| CollaborationDeviceResponse::Reply {
                    reply: Box::new(reply),
                    authority_device_id: self.local_device_id.clone(),
                    authority_epoch: placement.authority_epoch,
                }),
            CollaborationDeviceAction::CreateAcp {
                caller,
                runner,
                message,
                yield_seconds,
            } => service
                .create_acp(caller, runner, message, yield_seconds)
                .await
                .map(|reply| CollaborationDeviceResponse::Reply {
                    reply: Box::new(reply),
                    authority_device_id: self.local_device_id.clone(),
                    authority_epoch: placement.authority_epoch,
                }),
            CollaborationDeviceAction::Send {
                caller,
                target,
                message,
                yield_seconds,
            } => service
                .send(caller, target, message, yield_seconds)
                .await
                .map(|reply| CollaborationDeviceResponse::Reply {
                    reply: Box::new(reply),
                    authority_device_id: self.local_device_id.clone(),
                    authority_epoch: placement.authority_epoch,
                }),
            CollaborationDeviceAction::Read {
                caller,
                target,
                yield_seconds,
                result,
            } => {
                let reply = match result {
                    Some(cursor) => service.read_result(caller, target, cursor).await,
                    None => service.read(caller, target, yield_seconds).await,
                };
                reply.map(|reply| CollaborationDeviceResponse::Reply {
                    reply: Box::new(reply),
                    authority_device_id: self.local_device_id.clone(),
                    authority_epoch: placement.authority_epoch,
                })
            }
            CollaborationDeviceAction::StopTurn {
                caller,
                target,
                turn_id,
                turn_authority_epoch,
            } => service
                .stop_turn(caller, target, turn_id, turn_authority_epoch)
                .await
                .map(|reply| CollaborationDeviceResponse::Stopped {
                    reply,
                    authority_device_id: self.local_device_id.clone(),
                    authority_epoch: placement.authority_epoch,
                }),
        };
        match response {
            Ok(response) => DeviceCommandResponse::Collaboration { response },
            Err(detail) => DeviceCommandResponse::Rejected {
                reason: "collaboration_rejected".into(),
                detail,
            },
        }
    }

    async fn handle_chat_media_prepare(
        &self,
        command: ChatMediaPrepareCommand,
    ) -> DeviceCommandResponse {
        if command.target_device_id != self.local_device_id
            || command.operation_id.trim().is_empty()
            || command.source_device_id.trim().is_empty()
            || command.byte_size == 0
            || uuid::Uuid::parse_str(&command.media_id).is_err()
        {
            return DeviceCommandResponse::Rejected {
                reason: "invalid_chat_media_prepare".to_string(),
                detail: "Chat Media prepare identity fields are invalid".to_string(),
            };
        }
        let Some(rua_root) = self.data_dir.as_deref() else {
            return DeviceCommandResponse::Rejected {
                reason: "chat_media_unavailable".to_string(),
                detail: "target Device has no Rua Root".to_string(),
            };
        };
        let scope =
            rua_core::chat_store::ChatScopeStore::resolve(rua_root, command.address.clone());
        let placement = match crate::services::conversation_authority_target::load_optional_chat_authority_placement(
            scope.root(),
            command.address.chat_id().as_str(),
        )
        .await
        {
            Ok(Some(placement)) => Some(placement),
            Ok(None) if command.first_submission => {
                if uuid::Uuid::parse_str(command.address.chat_id().as_str()).is_err() {
                    return DeviceCommandResponse::Rejected { reason: "invalid_chat_media_prepare".into(), detail: "Chat id must be a UUID".into() };
                }
                let store = rua_session::WorkspaceSessionStore::new(scope.root());
                match store.load_chat_header(command.address.chat_id().as_str()).await {
                    Ok(None) => {},
                    _ => return DeviceCommandResponse::Rejected { reason: "chat_not_ready".into(), detail: "Chat storage is not a new submission target".into() },
                }
                if let Some(project_id) = command.address.project_id() {
                    let Some(catalog) = self.project_catalog.as_ref() else { return DeviceCommandResponse::Rejected { reason: "project_runtime_unavailable".into(), detail: "Project catalog unavailable".into() }; };
                    let project = match catalog.get(project_id) {
                        Ok(Some(project)) => project,
                        Ok(None) => return DeviceCommandResponse::Rejected { reason: "project_not_found".into(), detail: "Project not found".into() },
                        Err(error) => return DeviceCommandResponse::Rejected { reason: "project_store_failed".into(), detail: error.to_string() },
                    };
                    if project.resource_device_id != self.local_device_id || project.lifecycle != rua_project::ProjectLifecycle::Active {
                        return DeviceCommandResponse::Rejected { reason: "project_authority_fixed".into(), detail: "Project media target must be its active resource Device".into() };
                    }
                } else if let Some(agent_id) = command.address.agent_id() {
                    let manager = crate::services::agent_manager::AgentManager::new(rua_root.to_path_buf());
                    match manager.get_agent(agent_id.as_str()).await {
                        Ok(Some(agent)) if std::path::Path::new(&agent.workspace_dir) == scope.root() => {},
                        Ok(Some(_)) => return DeviceCommandResponse::Rejected { reason: "agent_scope_mismatch".into(), detail: "Agent media scope differs from its registered workspace".into() },
                        Ok(None) => return DeviceCommandResponse::Rejected { reason: "agent_unavailable".into(), detail: "Agent is not registered".into() },
                        Err(error) => return DeviceCommandResponse::Rejected { reason: "agent_unavailable".into(), detail: error.to_string() },
                    }
                }
                None
            }
            _ => {
                return DeviceCommandResponse::Rejected {
                    reason: "conversation_authority_missing".to_string(),
                    detail: "Conversation Authority placement is not materialized".to_string(),
                }
            }
        };
        if let Some(placement) = placement.filter(|placement| {
            placement.device_id != self.local_device_id
                || placement.authority_epoch != command.authority_epoch
        }) {
            return DeviceCommandResponse::Rejected {
                reason: "conversation_authority_redirect".to_string(),
                detail: format!(
                    "Conversation Authority is on Device '{}' at epoch '{}'",
                    placement.device_id, placement.authority_epoch
                ),
            };
        }
        if let Some(expected) = command.sha256.as_deref() {
            let path = scope.root().join("media").join(&command.media_id);
            if path.exists() {
                match crate::services::remote_conversation_input::media_fingerprint(&path).await {
                    Ok((size, digest)) if size == command.byte_size && digest == expected => {
                        return DeviceCommandResponse::ChatMediaPrepared {
                            response: ChatMediaPreparedResponse {
                                already_present: true,
                                operation_id: command.operation_id,
                                media_ref: format!("media/{}", command.media_id),
                                media_id: command.media_id,
                                authority_epoch: command.authority_epoch,
                            },
                        };
                    }
                    _ => {
                        return DeviceCommandResponse::Rejected {
                            reason: "media_conflict".into(),
                            detail: "Existing attachment does not match the submitted content"
                                .into(),
                        }
                    }
                }
            }
        }
        let Some(registry) = self.chat_media_transfers.as_ref() else {
            return DeviceCommandResponse::Rejected {
                reason: "chat_media_unavailable".to_string(),
                detail: "target Device has no Chat Media transfer registry".to_string(),
            };
        };
        if registry
            .authorize_upload(
                crate::services::chat_media_transfer::AuthorizedMediaUpload {
                    operation_id: command.operation_id.clone(),
                    source_device_id: command.source_device_id,
                    target_device_id: command.target_device_id,
                    media_dir: scope.root().join("media"),
                    media_id: command.media_id.clone(),
                    byte_size: command.byte_size,
                    mime_type: command.mime_type,
                    authorized_at: std::time::Instant::now(),
                },
            )
            .is_err()
        {
            return DeviceCommandResponse::Rejected {
                reason: "chat_media_prepare_conflict".to_string(),
                detail: "Chat Media transfer operation already exists".to_string(),
            };
        }
        if command.first_submission {
            let marker = scope
                .root()
                .join("media")
                .join(format!(".staged-{}.json", command.media_id));
            if let Err(error) = rua_config::atomic_write_json(
                &marker,
                &serde_json::json!({"address":command.address}),
            ) {
                registry.abort(&command.operation_id);
                return DeviceCommandResponse::Rejected {
                    reason: "media_staging_failed".into(),
                    detail: error.to_string(),
                };
            }
        }
        DeviceCommandResponse::ChatMediaPrepared {
            response: ChatMediaPreparedResponse {
                already_present: false,
                operation_id: command.operation_id,
                media_ref: format!("media/{}", command.media_id),
                media_id: command.media_id,
                authority_epoch: command.authority_epoch,
            },
        }
    }

    async fn handle_project(&self, command: ProjectDeviceCommand) -> DeviceCommandResponse {
        if let Err(error) = command.validate_for_target(&self.local_device_id) {
            return DeviceCommandResponse::Rejected {
                reason: "invalid_project_command".to_string(),
                detail: error.to_string(),
            };
        }
        match command.action {
            action @ (ProjectDeviceAction::BrowseDirectory { .. }
            | ProjectDeviceAction::CreateDirectory { .. }) => {
                let result = match action {
                    ProjectDeviceAction::BrowseDirectory { path } => {
                        rua_project::list_project_directory(&path)
                    }
                    ProjectDeviceAction::CreateDirectory { parent, name } => {
                        rua_project::create_project_directory(&parent, &name)
                    }
                    _ => unreachable!(),
                };
                match result {
                    Ok(listing) => DeviceCommandResponse::Project {
                        response: ProjectDeviceResponse::Directory {
                            device_id: self.local_device_id.clone(),
                            path: listing.path,
                            entries: listing.entries,
                            roots: listing.roots,
                        },
                    },
                    Err(error) => DeviceCommandResponse::Rejected {
                        reason: "project_directory_unavailable".to_string(),
                        detail: error.to_string(),
                    },
                }
            }
            action @ (ProjectDeviceAction::Create { .. }
            | ProjectDeviceAction::CreateResources { .. }) => {
                let (resource_roots, primary_index, display_name) = match action {
                    ProjectDeviceAction::Create {
                        resource_root,
                        display_name,
                    } => (vec![resource_root], 0, display_name),
                    ProjectDeviceAction::CreateResources {
                        resource_roots,
                        primary_index,
                        display_name,
                    } => (resource_roots, primary_index, display_name),
                    _ => unreachable!(),
                };
                let Some(catalog) = self.project_catalog.as_ref() else {
                    return DeviceCommandResponse::Rejected {
                        reason: "project_runtime_unavailable".to_string(),
                        detail: "Project Catalog is unavailable".to_string(),
                    };
                };
                match catalog.create_resources(rua_project::ProjectCreateResourcesRequest {
                    resource_device_id: self.local_device_id.clone(),
                    resource_roots: resource_roots
                        .iter()
                        .map(std::path::PathBuf::from)
                        .collect(),
                    primary_index,
                    display_name,
                }) {
                    Ok(outcome) => {
                        if let Some(publication) = self.project_publication.as_ref() {
                            let result = crate::sync_publication::publish_sync_fact_with_runtime(
                                publication,
                                crate::sync_publication::SyncPublicationFact::ProjectDescriptorCommitted {
                                    descriptor: outcome.project.clone(),
                                },
                            )
                            .await;
                            if result.status
                                == crate::sync_publication::PublicationStatus::AppendFailed
                            {
                                tracing::warn!(
                                    project_id = %outcome.project.project_id,
                                    errors = ?result.errors,
                                    "remote Project committed but descriptor publication failed"
                                );
                            }
                        }
                        DeviceCommandResponse::Project {
                            response: ProjectDeviceResponse::Created { outcome },
                        }
                    }
                    Err(rua_project::ProjectError::DuplicateBinding { owner }) => {
                        DeviceCommandResponse::Project {
                            response: ProjectDeviceResponse::Duplicate { owner: *owner },
                        }
                    }
                    Err(error) => DeviceCommandResponse::Rejected {
                        reason: "project_create_failed".to_string(),
                        detail: error.to_string(),
                    },
                }
            }
            ProjectDeviceAction::EditResources {
                project_id,
                expected_revision,
                resource_roots,
                primary_index,
                directory_ids,
            } => {
                let Some(editor) = self.project_resource_editor.as_ref() else {
                    return DeviceCommandResponse::Rejected {
                        reason: "project_runtime_unavailable".into(),
                        detail: "Project runtime unavailable".into(),
                    };
                };
                match editor
                    .edit_resources(
                        &project_id,
                        expected_revision,
                        &resource_roots,
                        primary_index,
                        &directory_ids,
                    )
                    .await
                {
                    Ok(outcome) => DeviceCommandResponse::Project {
                        response: ProjectDeviceResponse::ResourcesEdited { outcome },
                    },
                    Err(detail) => DeviceCommandResponse::Rejected {
                        reason: "project_resources_conflict".into(),
                        detail,
                    },
                }
            }
            ProjectDeviceAction::SetPrimaryDirectory {
                project_id,
                expected_revision,
                directory_id,
            } => {
                let Some(editor) = self.project_resource_editor.as_ref() else {
                    return DeviceCommandResponse::Rejected {
                        reason: "project_runtime_unavailable".into(),
                        detail: "Project runtime unavailable".into(),
                    };
                };
                match editor
                    .set_primary_directory(&project_id, expected_revision, &directory_id)
                    .await
                {
                    Ok(project) => DeviceCommandResponse::Project {
                        response: ProjectDeviceResponse::ResourcesUpdated { project },
                    },
                    Err(detail) => DeviceCommandResponse::Rejected {
                        reason: "project_resources_conflict".into(),
                        detail,
                    },
                }
            }
            ProjectDeviceAction::ArchiveIdleChats {
                project_id,
                chat_ids,
            } => {
                let Some(intake) = self.conversation_input_intake.as_ref() else {
                    return DeviceCommandResponse::Rejected {
                        reason: "project_runtime_unavailable".into(),
                        detail: "Project runtime unavailable".into(),
                    };
                };
                match intake
                    .archive_idle_project_chats(&project_id, chat_ids, &command.source_device_id)
                    .await
                {
                    Ok(result) => DeviceCommandResponse::Project {
                        response: ProjectDeviceResponse::ChatsArchived { result },
                    },
                    Err(detail) => DeviceCommandResponse::Rejected {
                        reason: "project_chat_archive_failed".into(),
                        detail,
                    },
                }
            }
            ProjectDeviceAction::InspectResource {
                project_id,
                directory_id,
                operation,
            } => {
                let binding = match self
                    .project_bindings
                    .as_ref()
                    .map(|store| store.get(&project_id))
                {
                    Some(Ok(Some(binding))) => binding,
                    _ => {
                        return DeviceCommandResponse::Rejected {
                            reason: "project_resource_unavailable".into(),
                            detail: "Project binding is unavailable".into(),
                        }
                    }
                };
                match tokio::task::spawn_blocking(move || {
                    crate::services::projects::inspect_project_resource(
                        &binding,
                        &directory_id,
                        operation,
                    )
                })
                .await
                {
                    Ok(Ok(response)) => DeviceCommandResponse::Project { response },
                    result => DeviceCommandResponse::Rejected {
                        reason: "project_resource_unavailable".into(),
                        detail: match result {
                            Ok(Err(error)) => error,
                            Err(error) => error.to_string(),
                            _ => unreachable!(),
                        },
                    },
                }
            }
            ProjectDeviceAction::InspectGit { project_id } => {
                let Some(bindings) = self.project_bindings.as_ref() else {
                    return DeviceCommandResponse::Rejected {
                        reason: "project_runtime_unavailable".to_string(),
                        detail: "Project Resource Binding Store is unavailable".to_string(),
                    };
                };
                let binding = match bindings.get(&project_id) {
                    Ok(Some(binding)) => binding,
                    Ok(None) => {
                        return DeviceCommandResponse::Rejected {
                            reason: "project_git_unavailable".to_string(),
                            detail: "Project binding is unavailable".to_string(),
                        };
                    }
                    Err(error) => {
                        return DeviceCommandResponse::Rejected {
                            reason: "project_git_unavailable".to_string(),
                            detail: error.to_string(),
                        };
                    }
                };
                match tokio::task::spawn_blocking(move || {
                    rua_project::inspect_project_git(binding.primary_root())
                })
                .await
                {
                    Ok(Ok(status)) => DeviceCommandResponse::Project {
                        response: ProjectDeviceResponse::GitStatus { status },
                    },
                    Ok(Err(error)) => DeviceCommandResponse::Rejected {
                        reason: "project_git_unavailable".to_string(),
                        detail: error,
                    },
                    Err(error) => DeviceCommandResponse::Rejected {
                        reason: "project_git_unavailable".to_string(),
                        detail: format!("inspect Project Git task failed: {error}"),
                    },
                }
            }
            ProjectDeviceAction::InspectGitHistory {
                project_id,
                offset,
                limit,
            } => {
                let Some(bindings) = self.project_bindings.as_ref() else {
                    return DeviceCommandResponse::Rejected {
                        reason: "project_runtime_unavailable".to_string(),
                        detail: "Project Resource Binding Store is unavailable".to_string(),
                    };
                };
                let binding = match bindings.get(&project_id) {
                    Ok(Some(binding)) => binding,
                    Ok(None) => {
                        return DeviceCommandResponse::Rejected {
                            reason: "project_git_unavailable".to_string(),
                            detail: "Project binding is unavailable".to_string(),
                        };
                    }
                    Err(error) => {
                        return DeviceCommandResponse::Rejected {
                            reason: "project_git_unavailable".to_string(),
                            detail: error.to_string(),
                        };
                    }
                };
                match tokio::task::spawn_blocking(move || {
                    rua_project::inspect_project_git_history(binding.primary_root(), offset, limit)
                })
                .await
                {
                    Ok(Ok(history)) => DeviceCommandResponse::Project {
                        response: ProjectDeviceResponse::GitHistory { history },
                    },
                    Ok(Err(error)) => DeviceCommandResponse::Rejected {
                        reason: "project_git_unavailable".to_string(),
                        detail: error,
                    },
                    Err(error) => DeviceCommandResponse::Rejected {
                        reason: "project_git_unavailable".to_string(),
                        detail: format!("inspect Project Git history task failed: {error}"),
                    },
                }
            }
            ProjectDeviceAction::InspectLifecycle { project_id } => {
                let Some(intake) = self.conversation_input_intake.as_ref() else {
                    return DeviceCommandResponse::Rejected {
                        reason: "project_runtime_unavailable".to_string(),
                        detail: "Project lifecycle runtime is unavailable".to_string(),
                    };
                };
                match intake.inspect_project_lifecycle(&project_id).await {
                    Ok(preview) => DeviceCommandResponse::Project {
                        response: ProjectDeviceResponse::LifecyclePreview { preview },
                    },
                    Err(detail) => DeviceCommandResponse::Rejected {
                        reason: "project_lifecycle_inspect_failed".to_string(),
                        detail,
                    },
                }
            }
            ProjectDeviceAction::Archive { project_id } => {
                let Some(intake) = self.conversation_input_intake.as_ref() else {
                    return DeviceCommandResponse::Rejected {
                        reason: "project_runtime_unavailable".to_string(),
                        detail: "Project lifecycle runtime is unavailable".to_string(),
                    };
                };
                match intake.archive_project(&project_id).await {
                    Ok(result) => {
                        if result.changed {
                            self.publish_project_descriptor(&result.project).await;
                        }
                        DeviceCommandResponse::Project {
                            response: ProjectDeviceResponse::Archived { result },
                        }
                    }
                    Err(detail) => DeviceCommandResponse::Rejected {
                        reason: "project_archive_failed".to_string(),
                        detail,
                    },
                }
            }
            ProjectDeviceAction::Reopen { project_id } => {
                let Some(intake) = self.conversation_input_intake.as_ref() else {
                    return DeviceCommandResponse::Rejected {
                        reason: "project_runtime_unavailable".to_string(),
                        detail: "Project lifecycle runtime is unavailable".to_string(),
                    };
                };
                match intake.reopen_project(&project_id).await {
                    Ok(result) => {
                        if result.changed {
                            self.publish_project_descriptor(&result.project).await;
                        }
                        DeviceCommandResponse::Project {
                            response: ProjectDeviceResponse::Reopened { result },
                        }
                    }
                    Err(detail) => DeviceCommandResponse::Rejected {
                        reason: "project_reopen_failed".to_string(),
                        detail,
                    },
                }
            }
            ProjectDeviceAction::SwitchChatAgent {
                project_id,
                chat_id,
                expected_agent_id,
                agent_id,
            } => {
                let Some(intake) = self.conversation_input_intake.as_ref() else {
                    return DeviceCommandResponse::Rejected {
                        reason: "project_runtime_unavailable".to_string(),
                        detail: "Project Chat input runtime is unavailable".to_string(),
                    };
                };
                match intake
                    .switch_project_chat_agent(
                        &project_id,
                        &chat_id,
                        &expected_agent_id,
                        &agent_id,
                        &command.source_device_id,
                    )
                    .await
                {
                    Ok(state) => DeviceCommandResponse::Project {
                        response: ProjectDeviceResponse::ChatAgentSelection { state },
                    },
                    Err(detail) => DeviceCommandResponse::Rejected {
                        reason: "project_agent_switch_rejected".to_string(),
                        detail,
                    },
                }
            }
            ProjectDeviceAction::UpdateChat {
                project_id,
                chat_id,
                update,
            } => {
                let Some(intake) = self.conversation_input_intake.as_ref() else {
                    return DeviceCommandResponse::Rejected {
                        reason: "project_runtime_unavailable".to_string(),
                        detail: "Project Chat input runtime is unavailable".to_string(),
                    };
                };
                match intake
                    .update_project_chat(&project_id, &chat_id, update, &command.source_device_id)
                    .await
                {
                    Ok(chat) => DeviceCommandResponse::Project {
                        response: ProjectDeviceResponse::ChatUpdated {
                            chat: serde_json::to_value(chat)
                                .expect("Project Chat response is serializable"),
                        },
                    },
                    Err(detail) => DeviceCommandResponse::Rejected {
                        reason: "project_chat_update_rejected".to_string(),
                        detail,
                    },
                }
            }
            ProjectDeviceAction::DeleteChat {
                project_id,
                chat_id,
                force,
            } => {
                let Some(intake) = self.conversation_input_intake.as_ref() else {
                    return DeviceCommandResponse::Rejected {
                        reason: "project_runtime_unavailable".to_string(),
                        detail: "Project Chat input runtime is unavailable".to_string(),
                    };
                };
                let result = intake
                    .delete_project_chat(&project_id, &chat_id, force, &command.source_device_id)
                    .await;
                DeviceCommandResponse::Project {
                    response: ProjectDeviceResponse::ChatDeleted { result },
                }
            }
            ProjectDeviceAction::ListTasks {
                project_id,
                chat_id,
                filter,
            } => {
                let Some(runtime) = self.task_runtime.as_ref() else {
                    return DeviceCommandResponse::Rejected {
                        reason: "task_runtime_unavailable".to_string(),
                        detail: "Task Runtime is unavailable".to_string(),
                    };
                };
                let owner = project_task_owner(project_id, chat_id);
                match runtime.list(&owner, filter).await {
                    Ok(tasks) => DeviceCommandResponse::Project {
                        response: ProjectDeviceResponse::TaskList {
                            tasks: serde_json::to_value(tasks).expect("Task list is serializable"),
                        },
                    },
                    Err(error) => project_task_error_response(error),
                }
            }
            ProjectDeviceAction::GetTask {
                project_id,
                chat_id,
                task_id,
            } => {
                let Some(runtime) = self.task_runtime.as_ref() else {
                    return DeviceCommandResponse::Rejected {
                        reason: "task_runtime_unavailable".to_string(),
                        detail: "Task Runtime is unavailable".to_string(),
                    };
                };
                let owner = project_task_owner(project_id, chat_id);
                match runtime.get(&owner, &task_id).await {
                    Ok(task) => DeviceCommandResponse::Project {
                        response: ProjectDeviceResponse::Task {
                            task: serde_json::to_value(task).expect("Task is serializable"),
                        },
                    },
                    Err(error) => project_task_error_response(error),
                }
            }
            ProjectDeviceAction::SendTaskInput {
                project_id,
                chat_id,
                task_id,
                submission,
            } => {
                let Some(runtime) = self.task_runtime.as_ref() else {
                    return DeviceCommandResponse::Rejected {
                        reason: "task_runtime_unavailable".to_string(),
                        detail: "Task Runtime is unavailable".to_string(),
                    };
                };
                let owner = project_task_owner(project_id, chat_id);
                match runtime.send_input(&owner, &task_id, submission).await {
                    Ok(acceptance) => DeviceCommandResponse::Project {
                        response: ProjectDeviceResponse::TaskInput { acceptance },
                    },
                    Err(error) => project_task_error_response(error),
                }
            }
            ProjectDeviceAction::WatchTask {
                project_id,
                chat_id,
                task_id,
            } => {
                let Some(runtime) = self.task_runtime.as_ref() else {
                    return DeviceCommandResponse::Rejected {
                        reason: "task_runtime_unavailable".to_string(),
                        detail: "Task Runtime is unavailable".to_string(),
                    };
                };
                let owner = project_task_owner(project_id, chat_id);
                match runtime.watch(&owner, &task_id).await {
                    Ok(status) => DeviceCommandResponse::Project {
                        response: ProjectDeviceResponse::TaskWatch { status },
                    },
                    Err(error) => project_task_error_response(error),
                }
            }
            ProjectDeviceAction::UnwatchTask {
                project_id,
                chat_id,
                task_id,
            } => {
                let Some(runtime) = self.task_runtime.as_ref() else {
                    return DeviceCommandResponse::Rejected {
                        reason: "task_runtime_unavailable".to_string(),
                        detail: "Task Runtime is unavailable".to_string(),
                    };
                };
                let owner = project_task_owner(project_id, chat_id);
                match runtime.unwatch(&owner, &task_id).await {
                    Ok(status) => DeviceCommandResponse::Project {
                        response: ProjectDeviceResponse::TaskWatch { status },
                    },
                    Err(error) => project_task_error_response(error),
                }
            }
            ProjectDeviceAction::CancelTask {
                project_id,
                chat_id,
                task_id,
            } => {
                let Some(runtime) = self.task_runtime.as_ref() else {
                    return DeviceCommandResponse::Rejected {
                        reason: "task_runtime_unavailable".to_string(),
                        detail: "Task Runtime is unavailable".to_string(),
                    };
                };
                let owner = project_task_owner(project_id, chat_id);
                match runtime.cancel(&owner, &task_id).await {
                    Ok(task) => DeviceCommandResponse::Project {
                        response: ProjectDeviceResponse::Task {
                            task: serde_json::to_value(task).expect("Task is serializable"),
                        },
                    },
                    Err(error) => project_task_error_response(error),
                }
            }
            ProjectDeviceAction::RetryTask {
                project_id,
                chat_id,
                task_id,
                authorization_agent_id,
            } => {
                let Some(runtime) = self.task_runtime.as_ref() else {
                    return DeviceCommandResponse::Rejected {
                        reason: "task_runtime_unavailable".to_string(),
                        detail: "Task Runtime is unavailable".to_string(),
                    };
                };
                let Some(catalog) = self.project_catalog.as_ref() else {
                    return DeviceCommandResponse::Rejected {
                        reason: "project_catalog_unavailable".into(),
                        detail: "Project catalog unavailable".into(),
                    };
                };
                let _admission = match catalog.begin_admission(&project_id) {
                    Ok(guard) => guard,
                    Err(error) => {
                        return DeviceCommandResponse::Rejected {
                            reason: "project_resources_changing".into(),
                            detail: error.to_string(),
                        }
                    }
                };
                let owner = project_task_owner(project_id, chat_id);
                let mut authorization =
                    match rua_task::TaskAttemptAuthorization::new(authorization_agent_id) {
                        Ok(authorization) => authorization,
                        Err(detail) => {
                            return DeviceCommandResponse::Rejected {
                                reason: "invalid_task_attempt_authorization".to_string(),
                                detail,
                            };
                        }
                    };
                if let Some(root) = self.data_dir.as_deref() {
                    let scope = rua_core::ChatScopeStore::resolve(root, owner.chat_address.clone());
                    match rua_session::WorkspaceSessionStore::new(scope.root())
                        .load_chat_header(owner.chat_id())
                        .await
                    {
                        Ok(Some(header)) => {
                            authorization.configuration_epoch = header
                                .extra
                                .get("hook_configuration")
                                .and_then(|configuration| configuration.get("epoch"))
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_string)
                        }
                        Ok(None) => {}
                        Err(error) => {
                            return DeviceCommandResponse::Rejected {
                                reason: "invalid_task_configuration".into(),
                                detail: error.to_string(),
                            }
                        }
                    }
                }
                match runtime.retry(&owner, &task_id, authorization).await {
                    Ok(task) => DeviceCommandResponse::Project {
                        response: ProjectDeviceResponse::Task {
                            task: serde_json::to_value(task).expect("Task is serializable"),
                        },
                    },
                    Err(error) => project_task_error_response(error),
                }
            }
            ProjectDeviceAction::GetTaskResult {
                project_id,
                chat_id,
                task_id,
            } => {
                let Some(runtime) = self.task_runtime.as_ref() else {
                    return DeviceCommandResponse::Rejected {
                        reason: "task_runtime_unavailable".to_string(),
                        detail: "Task Runtime is unavailable".to_string(),
                    };
                };
                let owner = project_task_owner(project_id, chat_id);
                match runtime.read_full_result(&owner, &task_id).await {
                    Ok(bytes) => match serde_json::from_slice(&bytes) {
                        Ok(result) => DeviceCommandResponse::Project {
                            response: ProjectDeviceResponse::TaskResult { result },
                        },
                        Err(error) => DeviceCommandResponse::Rejected {
                            reason: "task_result_invalid".to_string(),
                            detail: error.to_string(),
                        },
                    },
                    Err(error) => project_task_error_response(error),
                }
            }
        }
    }

    async fn handle_sync_transfer(
        &self,
        command: SyncTransferDeviceCommand,
    ) -> DeviceCommandResponse {
        if let Err(error) = command.validate_for_target(&self.local_device_id) {
            return DeviceCommandResponse::Rejected {
                reason: "invalid_sync_transfer_command".to_string(),
                detail: error.to_string(),
            };
        }
        match command.action {
            SyncTransferDeviceAction::ListDir { path } => match list_device_directory(&path) {
                Ok(listing) => DeviceCommandResponse::SyncTransfer {
                    response: SyncTransferDeviceResponse::DirEntries {
                        device_id: self.local_device_id.clone(),
                        path: listing.path,
                        entries: listing.entries,
                    },
                },
                Err(error) => DeviceCommandResponse::Rejected {
                    reason: "sync_transfer_list_failed".to_string(),
                    detail: error.to_string(),
                },
            },
            SyncTransferDeviceAction::StartSend {
                operation_id,
                source_paths,
                transfer_target_device_id,
                target_dir,
                conflict,
            } => {
                let Some(session_manager) = self
                    .sync_transfer_session_manager
                    .as_ref()
                    .and_then(|cell| cell.read().ok().and_then(|guard| guard.as_ref().cloned()))
                else {
                    return DeviceCommandResponse::Rejected {
                        reason: "sync_transfer_runtime_unavailable".to_string(),
                        detail: "source Device has no shared Device Session runtime".to_string(),
                    };
                };
                let source_paths = source_paths
                    .into_iter()
                    .map(PathBuf::from)
                    .collect::<Vec<_>>();
                let manifest =
                    match crate::services::sync_transfer::build_transfer_manifest(&source_paths) {
                        Ok(manifest) => manifest,
                        Err(error) => {
                            return DeviceCommandResponse::Rejected {
                                reason: "sync_transfer_manifest_failed".to_string(),
                                detail: error.to_string(),
                            }
                        }
                    };
                let snapshot = match self.start_sync_transfer_operation(
                    session_manager,
                    operation_id,
                    transfer_target_device_id,
                    target_dir,
                    conflict,
                    manifest,
                ) {
                    Ok(snapshot) => snapshot,
                    Err(detail) => {
                        return DeviceCommandResponse::Rejected {
                            reason: "sync_transfer_start_failed".to_string(),
                            detail,
                        }
                    }
                };
                DeviceCommandResponse::SyncTransfer {
                    response: SyncTransferDeviceResponse::TransferOperation {
                        operation: snapshot,
                    },
                }
            }
            SyncTransferDeviceAction::StartChatGapSend {
                operation_id,
                agent_id,
                chat_id,
                expected_authority_epoch,
                required_manifest,
                transfer_target_device_id,
                target_dir,
                target_inventory,
            } => {
                let Some(data_dir) = self.data_dir.as_ref() else {
                    return DeviceCommandResponse::Rejected {
                        reason: "conversation_history_gap_unavailable".to_string(),
                        detail: "source Device has no workspace data root".to_string(),
                    };
                };
                let workspace_dir = data_dir.join("workspaces").join(&agent_id);
                let address = ChatAddress::agent(&agent_id, &chat_id);
                let actual_manifest =
                    match load_conversation_manifest(&workspace_dir, &address).await {
                        Ok(Some(manifest)) => manifest,
                        Ok(None) => {
                            return DeviceCommandResponse::Rejected {
                                reason: "conversation_history_gap_unavailable".to_string(),
                                detail: "source conversation manifest is unavailable".to_string(),
                            }
                        }
                        Err(error) => {
                            return DeviceCommandResponse::Rejected {
                                reason: "conversation_history_gap_unavailable".to_string(),
                                detail: error.to_string(),
                            }
                        }
                    };
                if !actual_manifest.compare(required_manifest.as_ref()).matches {
                    return DeviceCommandResponse::Rejected {
                        reason: "conversation_history_gap_mismatch".to_string(),
                        detail: "source conversation no longer matches the requested Manifest"
                            .to_string(),
                    };
                }
                let source_chat = match crate::services::chat_metadata_store::load_chat_record(
                    &workspace_dir,
                    &chat_id,
                )
                .await
                {
                    Ok(Some(chat)) => chat,
                    Ok(None) => {
                        return DeviceCommandResponse::Rejected {
                            reason: "conversation_history_gap_unavailable".to_string(),
                            detail: "source Chat Index record is unavailable".to_string(),
                        }
                    }
                    Err(error) => {
                        return DeviceCommandResponse::Rejected {
                            reason: "conversation_history_gap_unavailable".to_string(),
                            detail: error.to_string(),
                        }
                    }
                };
                let source_placement = match crate::services::conversation_authority_placement::parse_conversation_authority_placement(
                    &source_chat.meta,
                ) {
                    Ok(placement) => placement,
                    Err(error) => {
                        return DeviceCommandResponse::Rejected {
                            reason: "conversation_history_gap_unavailable".to_string(),
                            detail: error.to_string(),
                        }
                    }
                };
                if source_placement.authority_epoch != expected_authority_epoch {
                    return DeviceCommandResponse::Rejected {
                        reason: "stale_authority_epoch".to_string(),
                        detail: format!(
                            "source has authority Epoch {}, expected {}",
                            source_placement.authority_epoch, expected_authority_epoch
                        ),
                    };
                }
                let source_chat_dir =
                    rua_session::WorkspaceSessionStore::new(&workspace_dir).chat_dir(&chat_id);
                let manifest = match crate::services::sync_transfer::build_gap_transfer_manifest(
                    &[source_chat_dir],
                    &target_inventory,
                ) {
                    Ok(manifest) => manifest,
                    Err(error) => {
                        return DeviceCommandResponse::Rejected {
                            reason: "sync_transfer_manifest_failed".to_string(),
                            detail: error.to_string(),
                        }
                    }
                };
                let Some(session_manager) = self
                    .sync_transfer_session_manager
                    .as_ref()
                    .and_then(|cell| cell.read().ok().and_then(|guard| guard.as_ref().cloned()))
                else {
                    return DeviceCommandResponse::Rejected {
                        reason: "sync_transfer_runtime_unavailable".to_string(),
                        detail: "source Device has no shared Device Session runtime".to_string(),
                    };
                };
                let snapshot = match self.start_sync_transfer_operation(
                    session_manager,
                    operation_id,
                    transfer_target_device_id,
                    target_dir,
                    SyncTransferConflictPolicy::Overwrite,
                    manifest,
                ) {
                    Ok(snapshot) => snapshot,
                    Err(detail) => {
                        return DeviceCommandResponse::Rejected {
                            reason: "sync_transfer_start_failed".to_string(),
                            detail,
                        }
                    }
                };
                DeviceCommandResponse::SyncTransfer {
                    response: SyncTransferDeviceResponse::ChatGapTransferOperation {
                        operation: snapshot,
                        source_chat: Box::new(source_chat),
                    },
                }
            }
            SyncTransferDeviceAction::GetSendStatus { operation_id } => {
                match self.sync_transfer_operations.get(&operation_id) {
                    Some(operation) => DeviceCommandResponse::SyncTransfer {
                        response: SyncTransferDeviceResponse::TransferOperation { operation },
                    },
                    None => DeviceCommandResponse::Rejected {
                        reason: "sync_transfer_operation_not_found".to_string(),
                        detail: format!("transfer operation not found: {operation_id}"),
                    },
                }
            }
            SyncTransferDeviceAction::CancelSend { operation_id } => {
                match self.sync_transfer_operations.cancel(&operation_id) {
                    Some(operation) => DeviceCommandResponse::SyncTransfer {
                        response: SyncTransferDeviceResponse::TransferOperation { operation },
                    },
                    None => DeviceCommandResponse::Rejected {
                        reason: "sync_transfer_operation_not_found".to_string(),
                        detail: format!("transfer operation not found: {operation_id}"),
                    },
                }
            }
        }
    }

    fn start_sync_transfer_operation(
        &self,
        session_manager: rua_device_network::DeviceSessionManager,
        operation_id: String,
        transfer_target_device_id: String,
        target_dir: String,
        conflict: SyncTransferConflictPolicy,
        manifest: crate::services::sync_transfer::TransferManifest,
    ) -> Result<crate::services::sync_transfer::SourceTransferOperationSnapshot, String> {
        let total_bytes = manifest.total_bytes;
        let (snapshot, cancel_rx) = self.sync_transfer_operations.start(
            operation_id.clone(),
            total_bytes,
            self.local_device_id.clone(),
            transfer_target_device_id.clone(),
        )?;
        if let Some(cancel_rx) = cancel_rx {
            let operations = Arc::clone(&self.sync_transfer_operations);
            let watchdog_operations = Arc::clone(&operations);
            let watchdog_operation_id = operation_id.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    if !watchdog_operations.is_running(&watchdog_operation_id)
                        || watchdog_operations.cancel_if_control_expired(&watchdog_operation_id)
                    {
                        return;
                    }
                }
            });
            let source_device_id = self.local_device_id.clone();
            tokio::spawn(async move {
                let result = crate::services::sync_transfer::send_transfer_stream(
                    &session_manager,
                    crate::services::sync_transfer::TransferStreamSendRequest {
                        operation_id: operation_id.clone(),
                        source_device_id,
                        target_device_id: transfer_target_device_id,
                        manifest,
                        target_dir: PathBuf::from(target_dir),
                        conflict_policy: conflict.into(),
                        options: crate::services::sync_transfer::TransferExecutionOptions::default(
                        ),
                    },
                    cancel_rx,
                    {
                        let operations = Arc::clone(&operations);
                        let operation_id = operation_id.clone();
                        move |progress| {
                            operations.update_progress(&operation_id, progress.bytes_transferred);
                        }
                    },
                )
                .await;
                match result {
                    Ok(outcome) => operations.succeed(&operation_id, outcome),
                    Err(crate::services::sync_transfer::SyncTransferPlanError::Cancelled) => {
                        let _ = operations.cancel(&operation_id);
                    }
                    Err(error) => operations.fail(&operation_id, error.to_string()),
                }
            });
        }
        Ok(snapshot)
    }

    async fn handle_remote_conversation_input(
        &self,
        request: RemoteConversationInputRequest,
    ) -> DeviceCommandResponse {
        if let Err(error) = request.validate_for_target(&self.local_device_id) {
            return DeviceCommandResponse::Rejected {
                reason: "invalid_remote_conversation_input".to_string(),
                detail: error.to_string(),
            };
        }
        if let Some(redirect) = self
            .authority_redirect_for(&request.address, request.authority_epoch)
            .await
        {
            return DeviceCommandResponse::AuthorityRedirect { redirect };
        }
        let Some(intake) = self.conversation_input_intake.as_ref().cloned() else {
            return DeviceCommandResponse::Rejected {
                reason: "conversation_input_intake_unavailable".to_string(),
                detail: "target device has no unified conversation intake".to_string(),
            };
        };
        match intake
            .accept_remote_conversation_input(request, chrono::Utc::now().to_rfc3339())
            .await
        {
            Ok(ack) => DeviceCommandResponse::Accepted(ack),
            Err(error) => DeviceCommandResponse::Rejected {
                reason: "remote_conversation_input_rejected".to_string(),
                detail: error,
            },
        }
    }

    async fn handle_conversation_control(
        &self,
        command: ConversationControlDeviceCommand,
    ) -> DeviceCommandResponse {
        if let Err(error) = command.validate_for_target(&self.local_device_id) {
            return DeviceCommandResponse::Rejected {
                reason: "invalid_conversation_control".to_string(),
                detail: error.to_string(),
            };
        }
        if let Some(intake) = self.conversation_input_intake.as_ref() {
            return match intake.apply_conversation_control(command).await {
                Ok(response) => DeviceCommandResponse::ConversationControl {
                    response: Box::new(response),
                },
                Err(DeviceCommandDeliveryError::AuthorityRedirect(redirect)) => {
                    DeviceCommandResponse::AuthorityRedirect {
                        redirect: *redirect,
                    }
                }
                Err(DeviceCommandDeliveryError::Rejected { reason, detail }) => {
                    DeviceCommandResponse::Rejected { reason, detail }
                }
                Err(error) => DeviceCommandResponse::Rejected {
                    reason: "conversation_control_failed".to_string(),
                    detail: error.to_string(),
                },
            };
        }
        if matches!(&command.action, ConversationControlDeviceAction::Discover) {
            return DeviceCommandResponse::Rejected {
                reason: "conversation_input_intake_unavailable".to_string(),
                detail: "target device cannot manage Project Conversation state".to_string(),
            };
        }
        let chat_id = command.chat_id().to_string();
        let Some(registry) = self.workspace_registry.as_ref() else {
            return DeviceCommandResponse::Rejected {
                reason: "workspace_registry_unavailable".to_string(),
                detail: "target workspace registry is unavailable".to_string(),
            };
        };
        let runtime = match registry.get_or_create(&command.agent_id).await {
            Ok(runtime) => runtime,
            Err(error) => {
                return DeviceCommandResponse::Rejected {
                    reason: "workspace_unavailable".to_string(),
                    detail: error.to_string(),
                };
            }
        };
        let workspace_dir = runtime.workspace_dir();
        let placement =
            match crate::services::conversation_authority_target::load_chat_authority_placement_target(
                &workspace_dir,
                chat_id.as_str(),
            )
            .await
            {
                Ok(placement) => placement,
                Err(detail) => {
                    return DeviceCommandResponse::Rejected {
                        reason: "authority_placement_unavailable".to_string(),
                        detail,
                    };
                }
            };
        if placement.device_id != command.target_device_id
            || placement.authority_epoch != command.authority_epoch
        {
            if placement.authority_epoch > command.authority_epoch {
                return DeviceCommandResponse::AuthorityRedirect {
                    redirect: ConversationAuthorityRedirect {
                        address: command.address.clone(),
                        placement: ConversationAuthorityPlacement {
                            device_id: placement.device_id,
                            authority_epoch: placement.authority_epoch,
                            base_manifest_hash: placement.base_manifest_hash,
                        },
                    },
                };
            }
            return DeviceCommandResponse::Rejected {
                reason: "stale_authority_epoch".to_string(),
                detail: "conversation control targets a stale authority Epoch".to_string(),
            };
        }
        let Some(authority) = self.conversation_authority.as_ref() else {
            return DeviceCommandResponse::Rejected {
                reason: "conversation_authority_unavailable".to_string(),
                detail: "target conversation authority is unavailable".to_string(),
            };
        };
        let mut meta = crate::services::conversation_authority_target::load_chat_control_meta(
            &workspace_dir,
            chat_id.as_str(),
        )
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| serde_json::json!({}));
        if !meta.is_object() {
            meta = serde_json::json!({});
        }
        if let Some(intake) = self.conversation_input_intake.as_ref() {
            if let Ok(model) = intake
                .authority_chat_model(&command.agent_id, chat_id.as_str())
                .await
            {
                meta.as_object_mut()
                    .expect("control metadata is an object")
                    .insert(
                        crate::services::conversation_authority_placement::CHAT_MODEL_META_KEY
                            .to_string(),
                        model,
                    );
            }
        }

        let response = match command.action {
            ConversationControlDeviceAction::ViewConfiguration
            | ConversationControlDeviceAction::UpdateConfiguration { .. } => {
                let Some(intake) = self.conversation_input_intake.as_ref() else {
                    return DeviceCommandResponse::Rejected {
                        reason: "conversation_input_intake_unavailable".to_string(),
                        detail: "target conversation input intake is unavailable".to_string(),
                    };
                };
                let update = match &command.action {
                    ConversationControlDeviceAction::UpdateConfiguration {
                        expected_epoch,
                        candidate_epoch,
                    } => Some((expected_epoch.as_str(), candidate_epoch.as_str())),
                    _ => None,
                };
                match intake
                    .chat_configuration_control(
                        &command.address,
                        &command.agent_id,
                        &workspace_dir,
                        update,
                    )
                    .await
                {
                    Ok(configuration) => {
                        ConversationControlDeviceResponse::Configuration { configuration }
                    }
                    Err(detail) => {
                        return DeviceCommandResponse::Rejected {
                            reason: "chat_configuration_invalid".to_string(),
                            detail,
                        }
                    }
                }
            }
            ConversationControlDeviceAction::Discover | ConversationControlDeviceAction::View => {
                ConversationControlDeviceResponse::View {
                    control: authority.control_view(&command.address, &meta).await,
                    execution_agent_id: Some(command.agent_id.clone()),
                    authority_placement: Some(ConversationAuthorityPlacement {
                        device_id: placement.device_id.clone(),
                        authority_epoch: placement.authority_epoch,
                        base_manifest_hash: placement.base_manifest_hash.clone(),
                    }),
                }
            }
            ConversationControlDeviceAction::ViewRunningConfig => {
                let Some(intake) = self.conversation_input_intake.as_ref() else {
                    return DeviceCommandResponse::Rejected {
                        reason: "conversation_input_intake_unavailable".to_string(),
                        detail: "target conversation input intake is unavailable".to_string(),
                    };
                };
                let running = match intake
                    .authority_chat_running_config(
                        &command.address,
                        &command.agent_id,
                        &workspace_dir,
                    )
                    .await
                {
                    Ok(running) => serde_json::to_value(running).unwrap_or_default(),
                    Err(error) => {
                        return DeviceCommandResponse::Rejected {
                            reason: "chat_running_config_unavailable".to_string(),
                            detail: error.to_string(),
                        };
                    }
                };
                ConversationControlDeviceResponse::RunningConfig { running }
            }
            ConversationControlDeviceAction::UpdateModel { model } => {
                let Some(intake) = self.conversation_input_intake.as_ref() else {
                    return DeviceCommandResponse::Rejected {
                        reason: "conversation_input_intake_unavailable".to_string(),
                        detail: "target conversation input intake is unavailable".to_string(),
                    };
                };
                let model = match intake
                    .update_authority_chat_model(&command.agent_id, chat_id.as_str(), model)
                    .await
                {
                    Ok(model) => model,
                    Err(detail) => {
                        return DeviceCommandResponse::Rejected {
                            reason: "chat_model_update_rejected".to_string(),
                            detail,
                        };
                    }
                };
                let mut updated_meta = meta.clone();
                updated_meta
                    .as_object_mut()
                    .expect("control metadata is an object")
                    .insert(
                        crate::services::conversation_authority_placement::CHAT_MODEL_META_KEY
                            .to_string(),
                        model.clone(),
                    );
                ConversationControlDeviceResponse::ModelUpdated {
                    model,
                    control: authority
                        .control_view(&command.address, &updated_meta)
                        .await,
                    chat: crate::services::chat_index::get(&workspace_dir, chat_id.as_str())
                        .await
                        .ok()
                        .flatten()
                        .map(crate::endpoints::chats::chat_from_index_entry)
                        .and_then(|chat| serde_json::to_value(chat).ok()),
                }
            }
            ConversationControlDeviceAction::UpdateRunningConfig { running } => {
                let Some(intake) = self.conversation_input_intake.as_ref() else {
                    return DeviceCommandResponse::Rejected {
                        reason: "conversation_input_intake_unavailable".to_string(),
                        detail: "target conversation input intake is unavailable".to_string(),
                    };
                };
                let running = match intake
                    .update_authority_chat_running_config(
                        &command.address,
                        &command.agent_id,
                        &workspace_dir,
                        running,
                    )
                    .await
                {
                    Ok(applied) => serde_json::to_value(applied).unwrap_or_default(),
                    Err(error) => {
                        return DeviceCommandResponse::Rejected {
                            reason: "chat_running_config_update_rejected".to_string(),
                            detail: error.to_string(),
                        };
                    }
                };
                let mut control = authority.control_view(&command.address, &meta).await;
                control.running_config = Some(running.clone());
                ConversationControlDeviceResponse::RunningConfigUpdated { running, control }
            }
            ConversationControlDeviceAction::EditQueuedInput { input_id, message } => {
                let Some(entry) = authority
                    .edit_queued_input(&command.address, &input_id, &message)
                    .await
                else {
                    return DeviceCommandResponse::Rejected {
                        reason: "queued_input_not_editable".to_string(),
                        detail: "queued input is not editable".to_string(),
                    };
                };
                ConversationControlDeviceResponse::Edited {
                    entry,
                    control: authority.control_view(&command.address, &meta).await,
                }
            }
            ConversationControlDeviceAction::DeleteQueuedInput { input_id } => {
                let Some(entry) = authority
                    .delete_queued_input(&command.address, &input_id)
                    .await
                else {
                    return DeviceCommandResponse::Rejected {
                        reason: "queued_input_not_editable".to_string(),
                        detail: "queued input is not editable".to_string(),
                    };
                };
                ConversationControlDeviceResponse::Deleted {
                    deleted_input_id: entry.input_id,
                    control: authority.control_view(&command.address, &meta).await,
                }
            }
            ConversationControlDeviceAction::SendNow {
                input_id,
                control_command_id,
            } => {
                if matches!(
                    authority
                        .active_turn(&command.address)
                        .await
                        .map(|turn| turn.driver),
                    Some(crate::services::conversation_authority::ConversationDriverKind::Acp)
                ) {
                    let _ = self
                        .cancel_acp_passthrough_turn(&RemoteChatControlCommand {
                            command_id: control_command_id.clone(),
                            agent_id: command.agent_id.clone(),
                            address: command.address.clone(),
                            source_device_id: command.source_device_id.clone(),
                            target_device_id: command.target_device_id.clone(),
                            authority_epoch: command.authority_epoch,
                            device_delivery_id: None,
                            await_terminal: false,
                            action: RemoteChatControlAction::InterjectCurrentTurn,
                        })
                        .await;
                }
                let Some(intake) = self.conversation_input_intake.as_ref() else {
                    return DeviceCommandResponse::Rejected {
                        reason: "conversation_input_intake_unavailable".to_string(),
                        detail: "target conversation input intake is unavailable".to_string(),
                    };
                };
                let entry = match intake
                    .send_now(
                        &command.agent_id,
                        chat_id.as_str(),
                        &input_id,
                        command.authority_epoch,
                        &control_command_id,
                    )
                    .await
                {
                    Ok(entry) => entry,
                    Err(detail) => {
                        return DeviceCommandResponse::Rejected {
                            reason: "send_now_rejected".to_string(),
                            detail,
                        };
                    }
                };
                ConversationControlDeviceResponse::SendNow {
                    input_id: entry.input_id,
                    applied: true,
                    control: authority.control_view(&command.address, &meta).await,
                }
            }
            ConversationControlDeviceAction::ApprovalSnapshot
            | ConversationControlDeviceAction::StopTurn { .. }
            | ConversationControlDeviceAction::PauseQueuedInputs { .. }
            | ConversationControlDeviceAction::DeliverBackgroundResult { .. }
            | ConversationControlDeviceAction::DecideApproval { .. }
            | ConversationControlDeviceAction::ViewProjectionExclusions
            | ConversationControlDeviceAction::UpdateProjectionExclusions { .. }
            | ConversationControlDeviceAction::ViewResourcePolicy
            | ConversationControlDeviceAction::SelectVariant { .. }
            | ConversationControlDeviceAction::RollbackBeforeTurn { .. }
            | ConversationControlDeviceAction::BranchBeforeTurn { .. } => {
                return DeviceCommandResponse::Rejected {
                    reason: "conversation_input_intake_unavailable".to_string(),
                    detail: "target conversation input intake is unavailable".to_string(),
                };
            }
        };
        DeviceCommandResponse::ConversationControl {
            response: Box::new(response),
        }
    }

    async fn handle_terminal(&self, command: TerminalDeviceCommand) -> DeviceCommandResponse {
        if let Err(error) = command.validate_for_target(&self.local_device_id) {
            return DeviceCommandResponse::Rejected {
                reason: "invalid_terminal_command".to_string(),
                detail: error.to_string(),
            };
        }
        let Some(authority) = &self.terminal_authority else {
            return DeviceCommandResponse::Rejected {
                reason: "terminal_runtime_unavailable".to_string(),
                detail: "target device has no terminal session authority".to_string(),
            };
        };
        match command.action {
            TerminalDeviceAction::ListSessions => DeviceCommandResponse::Terminal {
                response: TerminalDeviceResponse::Sessions {
                    sessions: authority.list().await,
                },
            },
            TerminalDeviceAction::ListProfiles => DeviceCommandResponse::Terminal {
                response: TerminalDeviceResponse::Profiles {
                    profiles: authority.list_profiles(),
                },
            },
            TerminalDeviceAction::Open {
                browser_viewer_id,
                scrollback_lines,
                cwd,
                terminal_profile_id,
            } => {
                let viewer =
                    TerminalViewerIdentity::paired(command.source_device_id, browser_viewer_id);
                let cwd = cwd.map(std::path::PathBuf::from);
                match authority
                    .open(OpenTerminalSessionRequest {
                        viewer,
                        cwd,
                        terminal_profile_id,
                        scrollback_lines,
                    })
                    .await
                {
                    Ok(session) => DeviceCommandResponse::Terminal {
                        response: TerminalDeviceResponse::Opened { session },
                    },
                    Err(error) => Self::terminal_error_response("terminal_open_failed", error),
                }
            }
            TerminalDeviceAction::Attach {
                session_id,
                browser_viewer_id,
                scrollback_lines,
                cursor,
            } => {
                let viewer =
                    TerminalViewerIdentity::paired(command.source_device_id, browser_viewer_id);
                match authority
                    .attach_remote(&session_id, viewer, scrollback_lines, cursor)
                    .await
                {
                    Ok(attachment) => {
                        frame_safe_terminal_attachment_response(
                            &self.terminal_attachment_transfers,
                            attachment,
                        )
                        .await
                    }
                    Err(error) => Self::terminal_error_response("terminal_attach_failed", error),
                }
            }
            TerminalDeviceAction::AttachmentChunk {
                transfer_id,
                offset_bytes,
            } => {
                terminal_attachment_chunk_response(
                    &self.terminal_attachment_transfers,
                    transfer_id,
                    offset_bytes,
                )
                .await
            }
            TerminalDeviceAction::SetScrollback {
                session_id,
                scrollback_lines,
            } => match authority
                .set_scrollback_lines(&session_id, scrollback_lines)
                .await
            {
                Ok(()) => DeviceCommandResponse::Terminal {
                    response: TerminalDeviceResponse::ScrollbackUpdated { session_id },
                },
                Err(error) => {
                    Self::terminal_error_response("terminal_scrollback_update_failed", error)
                }
            },
            TerminalDeviceAction::Rename {
                session_id,
                display_name,
            } => match authority.rename(&session_id, display_name).await {
                Ok(session) => DeviceCommandResponse::Terminal {
                    response: TerminalDeviceResponse::Renamed { session },
                },
                Err(error) => Self::terminal_error_response("terminal_rename_failed", error),
            },
            TerminalDeviceAction::TakeInputLease {
                session_id,
                browser_viewer_id,
            } => {
                let viewer =
                    TerminalViewerIdentity::paired(command.source_device_id, browser_viewer_id);
                match authority.take_input_lease(&session_id, viewer).await {
                    Ok(session) => DeviceCommandResponse::Terminal {
                        response: TerminalDeviceResponse::InputLeaseTaken { session },
                    },
                    Err(error) => Self::terminal_error_response("terminal_takeover_failed", error),
                }
            }
            TerminalDeviceAction::SendInput {
                session_id,
                browser_viewer_id,
                data,
            } => {
                let viewer =
                    TerminalViewerIdentity::paired(command.source_device_id, browser_viewer_id);
                match authority.send_input(&session_id, &viewer, &data).await {
                    Ok(()) => DeviceCommandResponse::Terminal {
                        response: TerminalDeviceResponse::InputAccepted {
                            session_id: session_id.clone(),
                        },
                    },
                    Err(error) => Self::terminal_error_response("terminal_input_rejected", error),
                }
            }
            TerminalDeviceAction::AdaptPtyGeometry {
                session_id,
                browser_viewer_id,
                cols,
                rows,
            } => {
                let viewer =
                    TerminalViewerIdentity::paired(command.source_device_id, browser_viewer_id);
                match authority
                    .adapt_pty_geometry(&session_id, &viewer, TerminalPtyGeometry { cols, rows })
                    .await
                {
                    Ok(session) => DeviceCommandResponse::Terminal {
                        response: TerminalDeviceResponse::PtyGeometryAdapted { session },
                    },
                    Err(error) => {
                        Self::terminal_error_response("terminal_geometry_rejected", error)
                    }
                }
            }
            TerminalDeviceAction::UpdatePalette {
                session_id,
                browser_viewer_id,
                palette,
            } => {
                let viewer =
                    TerminalViewerIdentity::paired(command.source_device_id, browser_viewer_id);
                match authority
                    .update_palette(&session_id, &viewer, palette)
                    .await
                {
                    Ok(()) => DeviceCommandResponse::Terminal {
                        response: TerminalDeviceResponse::PaletteUpdated { session_id },
                    },
                    Err(error) => Self::terminal_error_response("terminal_palette_rejected", error),
                }
            }
            TerminalDeviceAction::DetachViewer {
                session_id,
                browser_viewer_id,
            } => {
                let viewer =
                    TerminalViewerIdentity::paired(command.source_device_id, browser_viewer_id);
                match authority.detach(&session_id, &viewer).await {
                    Ok(()) => DeviceCommandResponse::Terminal {
                        response: TerminalDeviceResponse::ViewerDetached {
                            session_id: session_id.clone(),
                        },
                    },
                    Err(error) => Self::terminal_error_response("terminal_detach_failed", error),
                }
            }
            TerminalDeviceAction::KillSession {
                session_id,
                browser_viewer_id,
            } => {
                let viewer =
                    TerminalViewerIdentity::paired(command.source_device_id, browser_viewer_id);
                match authority.kill(&session_id, &viewer).await {
                    Ok(()) => DeviceCommandResponse::Terminal {
                        response: TerminalDeviceResponse::SessionKilled {
                            session_id: session_id.clone(),
                        },
                    },
                    Err(error) => Self::terminal_error_response("terminal_kill_failed", error),
                }
            }
        }
    }

    fn terminal_error_response(
        fallback_reason: &str,
        error: TerminalError,
    ) -> DeviceCommandResponse {
        let reason = match error {
            TerminalError::NotFound(_) => "terminal_not_found",
            TerminalError::CursorUnavailable { .. } => "terminal_cursor_unavailable",
            TerminalError::InputLeaseRequired => "terminal_input_lease_required",
            TerminalError::Spawn(_) | TerminalError::SnapshotUnavailable(_) => fallback_reason,
        };
        DeviceCommandResponse::Rejected {
            reason: reason.to_string(),
            detail: error.to_string(),
        }
    }

    async fn handle_conversation_manifest(
        &self,
        command: ConversationManifestDeviceCommand,
    ) -> DeviceCommandResponse {
        if let Err(error) = command.validate_for_target(&self.local_device_id) {
            return DeviceCommandResponse::Rejected {
                reason: "invalid_manifest_command".to_string(),
                detail: error.to_string(),
            };
        }
        let Some(data_dir) = self.data_dir.as_ref() else {
            return DeviceCommandResponse::Rejected {
                reason: "manifest_unavailable".to_string(),
                detail: "target has no workspace data root".to_string(),
            };
        };
        let scope_store = ChatScopeStore::resolve(data_dir, command.address.clone());
        if command.required_manifest.is_none() {
            let watermarks = match crate::services::durable_publication_watermark::DurablePublicationWatermarkStore::new(data_dir.join("sync")) {
                Ok(watermarks) => watermarks,
                Err(error) => {
                    return DeviceCommandResponse::Rejected {
                        reason: "manifest_unavailable".to_string(),
                        detail: error.to_string(),
                    };
                }
            };
            return match watermarks.load(&command.address).await {
                Ok(watermark) => DeviceCommandResponse::ConversationManifest {
                    response: ConversationManifestDeviceResponse {
                        command_id: command.command_id,
                        target_device_id: command.target_device_id,
                        address: command.address,
                        manifest: watermark.map(|watermark| watermark.manifest),
                    },
                },
                Err(error) => DeviceCommandResponse::Rejected {
                    reason: "manifest_unavailable".to_string(),
                    detail: error.to_string(),
                },
            };
        }
        if let Some(requirement) = command.required_manifest.as_ref() {
            if let Err(error) =
                ensure_conversation_history_ready(ConversationHistoryReadinessRequest {
                    data_dir,
                    address: &command.address,
                    source_device_id: &command.source_device_id,
                    requirement,
                    sync_tx: self.remote_history_sync_tx.as_ref(),
                })
                .await
            {
                return DeviceCommandResponse::Rejected {
                    reason: "history_not_ready".to_string(),
                    detail: error.to_string(),
                };
            }
        }
        match load_conversation_manifest(scope_store.root(), &command.address).await {
            Ok(manifest) => DeviceCommandResponse::ConversationManifest {
                response: ConversationManifestDeviceResponse {
                    command_id: command.command_id,
                    target_device_id: command.target_device_id,
                    address: command.address,
                    manifest,
                },
            },
            Err(error) => DeviceCommandResponse::Rejected {
                reason: "manifest_unavailable".to_string(),
                detail: error.to_string(),
            },
        }
    }

    async fn handle_conversation_authority_adopt(
        &self,
        command: ConversationAuthorityAdoptDeviceCommand,
    ) -> DeviceCommandResponse {
        if command.address.project_id().is_some() {
            return DeviceCommandResponse::Rejected {
                reason: "project_authority_fixed".to_string(),
                detail: "Project Chat authority is fixed to its resource Device".to_string(),
            };
        }
        if let Err(error) = command.validate_for_target(&self.local_device_id) {
            return DeviceCommandResponse::Rejected {
                reason: "invalid_authority_adopt_command".to_string(),
                detail: error.to_string(),
            };
        }
        let Some(data_dir) = self.data_dir.as_ref() else {
            return DeviceCommandResponse::Rejected {
                reason: "authority_adopt_unavailable".to_string(),
                detail: "target has no workspace data root".to_string(),
            };
        };
        if command
            .required_manifest
            .latest_turn_status
            .as_deref()
            .is_some_and(|status| {
                !rua_session::TurnLifecycleStatus::parse(status).is_syncable_conversation_state()
            })
        {
            return DeviceCommandResponse::Rejected {
                reason: "history_not_terminal".to_string(),
                detail: "latest conversation Turn is not terminal".to_string(),
            };
        }
        let base_manifest_hash = match command.required_manifest.hash() {
            Ok(hash) => hash,
            Err(error) => {
                return DeviceCommandResponse::Rejected {
                    reason: "invalid_conversation_manifest".to_string(),
                    detail: error.to_string(),
                };
            }
        };
        let scope_store = ChatScopeStore::resolve(data_dir, command.address.clone());
        let workspace_dir = scope_store.root();
        if let Ok(current) =
            crate::services::conversation_authority_target::load_chat_authority_placement_target(
                workspace_dir,
                command.chat_id(),
            )
            .await
        {
            if current.device_id == command.target_device_id
                && current.authority_epoch > command.expected_authority_epoch
                && current.base_manifest_hash == base_manifest_hash
            {
                return DeviceCommandResponse::ConversationAuthorityAdopt {
                    response: ConversationAuthorityAdoptDeviceResponse {
                        command_id: command.command_id,
                        target_device_id: command.target_device_id,
                        address: command.address,
                        placement: ConversationAuthorityPlacement {
                            device_id: current.device_id,
                            authority_epoch: current.authority_epoch,
                            base_manifest_hash: current.base_manifest_hash,
                        },
                    },
                };
            }
            if current.authority_epoch > command.expected_authority_epoch {
                return DeviceCommandResponse::Rejected {
                    reason: "stale_authority_epoch".to_string(),
                    detail: format!(
                        "target already knows newer authority Epoch {}, expected {}",
                        current.authority_epoch, command.expected_authority_epoch
                    ),
                };
            }
        }
        let session_manager = self
            .sync_transfer_session_manager
            .as_ref()
            .and_then(|cell| cell.read().ok().and_then(|guard| guard.as_ref().cloned()));
        let source_chat = match ensure_conversation_history_ready_via_device_session(
            ConversationHistoryGapRequest {
                data_dir,
                agent_id: &command.agent_id,
                chat_id: command.chat_id(),
                source_device_id: &command.source_device_id,
                media_source_device_id: &command.authority_source_device_id,
                target_device_id: &command.target_device_id,
                expected_authority_epoch: command.expected_authority_epoch,
                requirement: &command.required_manifest,
                session_manager: session_manager.as_ref(),
            },
        )
        .await
        {
            Ok(source_chat) => source_chat,
            Err(error) => {
                return DeviceCommandResponse::Rejected {
                    reason: "history_not_ready".to_string(),
                    detail: error,
                }
            }
        };
        let current = match crate::services::conversation_authority_placement::parse_conversation_authority_placement(
            &source_chat.meta,
        ) {
            Ok(placement) => placement,
            Err(error) => {
                return DeviceCommandResponse::Rejected {
                    reason: "authority_placement_unavailable".to_string(),
                    detail: error.to_string(),
                }
            }
        };
        if current.device_id == command.target_device_id
            && current.authority_epoch > command.expected_authority_epoch
            && current.base_manifest_hash == base_manifest_hash
        {
            return DeviceCommandResponse::ConversationAuthorityAdopt {
                response: ConversationAuthorityAdoptDeviceResponse {
                    command_id: command.command_id,
                    target_device_id: command.target_device_id,
                    address: command.address,
                    placement: ConversationAuthorityPlacement {
                        device_id: current.device_id,
                        authority_epoch: current.authority_epoch,
                        base_manifest_hash: current.base_manifest_hash,
                    },
                },
            };
        }
        if current.authority_epoch != command.expected_authority_epoch {
            return DeviceCommandResponse::Rejected {
                reason: "stale_authority_epoch".to_string(),
                detail: format!(
                    "target has authority Epoch {}, expected {}",
                    current.authority_epoch, command.expected_authority_epoch
                ),
            };
        }
        let Some(authority) = self.conversation_authority.as_ref() else {
            return DeviceCommandResponse::Rejected {
                reason: "conversation_authority_unavailable".to_string(),
                detail: "target Conversation Authority is unavailable".to_string(),
            };
        };
        if authority.active_turn(&command.address).await.is_some() {
            return DeviceCommandResponse::Rejected {
                reason: "active_turn".to_string(),
                detail: "cannot adopt authority while the target has an active turn".to_string(),
            };
        }
        match crate::services::conversation_authority_target::load_chat_control_meta(
            workspace_dir,
            command.chat_id(),
        )
        .await
        {
            Ok(Some(meta)) if chat_acp_binding_active(&meta) => {
                return DeviceCommandResponse::Rejected {
                    reason: "acp_active".to_string(),
                    detail: "cannot adopt authority while ACP passthrough is active".to_string(),
                };
            }
            Ok(_) => {}
            Err(error) => {
                return DeviceCommandResponse::Rejected {
                    reason: "authority_placement_unavailable".to_string(),
                    detail: error.to_string(),
                };
            }
        }
        let Some(intake) = self.conversation_input_intake.as_ref() else {
            return DeviceCommandResponse::Rejected {
                reason: "conversation_input_intake_unavailable".to_string(),
                detail: "target conversation input intake is unavailable".to_string(),
            };
        };
        if let Err(detail) = intake
            .validate_authority_adoption_model(&command.agent_id, command.chat_id())
            .await
        {
            return DeviceCommandResponse::Rejected {
                reason: "model_unavailable".to_string(),
                detail,
            };
        }
        let target = ConversationAuthorityPlacement {
            authority_epoch: ConversationAuthorityEpoch::next_for_device(
                &command.target_device_id,
                Some(current.authority_epoch),
            ),
            device_id: command.target_device_id.clone(),
            base_manifest_hash,
        };
        match Box::pin(intake.adopt_conversation_authority(
            &command.agent_id,
            command.chat_id(),
            target.clone(),
        ))
        .await
        {
            Ok(_) => {
                spawn_conversation_authority_supersession(
                    self.sync_transfer_session_manager.as_ref(),
                    &command.agent_id,
                    command.chat_id(),
                    &current.device_id,
                    current.authority_epoch,
                    &target,
                );
                DeviceCommandResponse::ConversationAuthorityAdopt {
                    response: ConversationAuthorityAdoptDeviceResponse {
                        command_id: command.command_id,
                        target_device_id: command.target_device_id,
                        address: command.address,
                        placement: target,
                    },
                }
            }
            Err(detail) => DeviceCommandResponse::Rejected {
                reason: "authority_adopt_rejected".to_string(),
                detail,
            },
        }
    }

    async fn handle_conversation_authority_supersede(
        &self,
        command: ConversationAuthoritySupersedeDeviceCommand,
    ) -> DeviceCommandResponse {
        if command.address.project_id().is_some() {
            return DeviceCommandResponse::Rejected {
                reason: "project_authority_fixed".to_string(),
                detail: "Project Chat authority is fixed to its resource Device".to_string(),
            };
        }
        if let Err(error) = command.validate_for_target(&self.local_device_id) {
            return DeviceCommandResponse::Rejected {
                reason: "invalid_authority_supersede_command".to_string(),
                detail: error.to_string(),
            };
        }
        let Some(data_dir) = self.data_dir.as_ref() else {
            return DeviceCommandResponse::Rejected {
                reason: "authority_supersede_unavailable".to_string(),
                detail: "target has no workspace data root".to_string(),
            };
        };
        let scope_store = ChatScopeStore::resolve(data_dir, command.address.clone());
        let workspace_dir = scope_store.root();
        let current =
            match crate::services::conversation_authority_target::load_chat_authority_placement_target(
                workspace_dir,
                command.chat_id(),
            )
            .await
            {
                Ok(placement) => placement,
                Err(detail) => {
                    return DeviceCommandResponse::Rejected {
                        reason: "authority_placement_unavailable".to_string(),
                        detail,
                    };
                }
            };
        if current.authority_epoch > command.replacement.authority_epoch {
            return DeviceCommandResponse::Rejected {
                reason: "stale_authority_supersession".to_string(),
                detail: format!(
                    "target already knows newer authority Epoch {}",
                    current.authority_epoch
                ),
            };
        }
        if current.authority_epoch == command.replacement.authority_epoch
            && (current.device_id != command.replacement.device_id
                || current.base_manifest_hash != command.replacement.base_manifest_hash)
        {
            return DeviceCommandResponse::Rejected {
                reason: "authority_supersession_conflict".to_string(),
                detail: "same authority Epoch names a different placement".to_string(),
            };
        }

        let Some(authority) = self.conversation_authority.as_ref() else {
            return DeviceCommandResponse::Rejected {
                reason: "conversation_authority_unavailable".to_string(),
                detail: "target Conversation Authority is unavailable".to_string(),
            };
        };
        match authority
            .apply_authority_supersession(
                workspace_dir,
                &command.address,
                command.replacement.clone(),
            )
            .await
        {
            Ok((record, superseded_input_ids)) => {
                DeviceCommandResponse::ConversationAuthoritySuperseded {
                    response: ConversationAuthoritySupersedeDeviceResponse {
                        command_id: command.command_id,
                        target_device_id: command.target_device_id,
                        address: command.address,
                        placement: crate::services::conversation_authority_placement::parse_conversation_authority_placement(
                            &record.meta,
                        )
                        .expect("supersession persisted a valid authority placement"),
                        superseded_input_ids,
                    },
                }
            }
            Err(detail) => DeviceCommandResponse::Rejected {
                reason: "authority_supersession_rejected".to_string(),
                detail: detail.to_string(),
            },
        }
    }

    async fn authority_redirect_for(
        &self,
        address: &ChatAddress,
        requested_epoch: ConversationAuthorityEpoch,
    ) -> Option<ConversationAuthorityRedirect> {
        if address.project_id().is_some() {
            return None;
        }
        let scope_store = ChatScopeStore::resolve(self.data_dir.as_ref()?, address.clone());
        let placement =
            crate::services::conversation_authority_target::load_chat_authority_placement_target(
                scope_store.root(),
                address.chat_id().as_str(),
            )
            .await
            .ok()?;
        (placement.authority_epoch > requested_epoch).then(|| ConversationAuthorityRedirect {
            address: address.clone(),
            placement: ConversationAuthorityPlacement {
                device_id: placement.device_id,
                authority_epoch: placement.authority_epoch,
                base_manifest_hash: placement.base_manifest_hash,
            },
        })
    }

    async fn handle_acp_chat(&self, command: AcpChatDeviceCommand) -> DeviceCommandResponse {
        if let Err(error) = command.validate_for_target(&self.local_device_id) {
            return DeviceCommandResponse::Rejected {
                reason: "invalid_acp_chat_command".to_string(),
                detail: error.to_string(),
            };
        }
        if let Some(intake) = self.conversation_input_intake.as_ref() {
            return match intake.apply_acp_control(command).await {
                Ok(response) => DeviceCommandResponse::AcpChat { response },
                Err(DeviceCommandDeliveryError::AuthorityRedirect(redirect)) => {
                    DeviceCommandResponse::AuthorityRedirect {
                        redirect: *redirect,
                    }
                }
                Err(DeviceCommandDeliveryError::Rejected { reason, detail }) => {
                    DeviceCommandResponse::Rejected { reason, detail }
                }
                Err(error) => DeviceCommandResponse::Rejected {
                    reason: "acp_chat_control_failed".to_string(),
                    detail: error.to_string(),
                },
            };
        }
        let chat_id = command.chat_id().to_string();
        let Some(registry) = &self.acp_chat_registry else {
            return DeviceCommandResponse::Rejected {
                reason: "acp_runtime_unavailable".to_string(),
                detail: "target device has no ACP chat runtime registry".to_string(),
            };
        };
        let Some(workspace_registry) = &self.workspace_registry else {
            return DeviceCommandResponse::Rejected {
                reason: "workspace_unavailable".to_string(),
                detail: "target device has no workspace registry".to_string(),
            };
        };
        let runtime = match workspace_registry.get_or_create(&command.agent_id).await {
            Ok(runtime) => runtime,
            Err(error) => {
                return DeviceCommandResponse::Rejected {
                    reason: "workspace_unavailable".to_string(),
                    detail: error.to_string(),
                };
            }
        };
        match crate::services::conversation_authority_target::load_chat_authority_placement_target(
            &runtime.workspace_dir(),
            chat_id.as_str(),
        )
        .await
        {
            Ok(placement)
                if placement.device_id == command.target_device_id
                    && placement.authority_epoch == command.authority_epoch => {}
            Ok(placement) => {
                if placement.authority_epoch > command.authority_epoch {
                    return DeviceCommandResponse::AuthorityRedirect {
                        redirect: ConversationAuthorityRedirect {
                            address: command.address.clone(),
                            placement: ConversationAuthorityPlacement {
                                device_id: placement.device_id,
                                authority_epoch: placement.authority_epoch,
                                base_manifest_hash: placement.base_manifest_hash,
                            },
                        },
                    };
                }
                return DeviceCommandResponse::Rejected {
                    reason: "stale_authority_epoch".to_string(),
                    detail: "ACP command targets a stale conversation authority Epoch".to_string(),
                };
            }
            Err(error) => {
                return DeviceCommandResponse::Rejected {
                    reason: "authority_placement_unavailable".to_string(),
                    detail: error,
                };
            }
        }
        let Some(handle) = registry.get(&command.agent_id) else {
            return DeviceCommandResponse::Rejected {
                reason: "acp_runtime_unavailable".to_string(),
                detail: format!(
                    "ACP chat runtime for agent '{}' is not registered on target device",
                    command.agent_id
                ),
            };
        };
        handle.refresh_config();

        match command.action {
            AcpChatDeviceAction::ListRunners => DeviceCommandResponse::AcpChat {
                response: AcpChatDeviceResponse::Runners {
                    runners: acp_runner_infos_from_service(&handle.service),
                },
            },
            AcpChatDeviceAction::Enter {
                runner,
                cwd,
                resume_session_id,
            } => {
                let Some(intake) = self.conversation_input_intake.as_ref() else {
                    return DeviceCommandResponse::Rejected {
                        reason: "conversation_input_intake_unavailable".to_string(),
                        detail: "target conversation input intake is unavailable".to_string(),
                    };
                };
                if handle.manager.get(chat_id.as_str()).await.is_some() {
                    return DeviceCommandResponse::Rejected {
                        reason: "acp_already_active".to_string(),
                        detail: "chat is already in ACP passthrough mode".to_string(),
                    };
                }
                if let Some(authority) = self.conversation_authority.as_ref() {
                    if authority.active_turn(&command.address).await.is_some() {
                        return DeviceCommandResponse::Rejected {
                            reason: "chat_active_turn".to_string(),
                            detail: "cannot enter ACP mode while the chat has an active turn"
                                .to_string(),
                        };
                    }
                }
                let expanded_cwd = cwd
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(expand_tilde);
                let cwd_arg = expanded_cwd.as_deref();
                let (message, history_preview) = match resume_session_id.as_deref() {
                    Some(session_id) if !session_id.trim().is_empty() => {
                        let resumed = handle
                            .controller
                            .resume_with_history_preview(
                                chat_id.as_str(),
                                &runner,
                                session_id.trim(),
                                cwd_arg,
                            )
                            .await;
                        (resumed.message, resumed.history_preview)
                    }
                    _ => {
                        let message = handle
                            .controller
                            .enter(chat_id.as_str(), &runner, cwd_arg)
                            .await;
                        (message, None)
                    }
                };
                let status =
                    build_acp_device_status(&handle, chat_id.as_str(), &self.local_device_id).await;
                if status.active {
                    let binding = AcpBindingMetadata {
                        target_device_id: Some(self.local_device_id.clone()),
                        runner: status.runner.clone().unwrap_or(runner),
                        cwd: status.cwd.clone(),
                        runner_session_id: status.session_id.clone(),
                    };
                    if let Err(detail) = intake
                        .enter_authority_acp(&command.agent_id, chat_id.as_str(), binding)
                        .await
                    {
                        let _ = handle.controller.exit(chat_id.as_str()).await;
                        return DeviceCommandResponse::Rejected {
                            reason: "acp_binding_update_rejected".to_string(),
                            detail,
                        };
                    }
                }
                DeviceCommandResponse::AcpChat {
                    response: AcpChatDeviceResponse::Entered {
                        ok: status.active,
                        message,
                        status,
                        history_preview,
                    },
                }
            }
            AcpChatDeviceAction::Exit => {
                let Some(intake) = self.conversation_input_intake.as_ref() else {
                    return DeviceCommandResponse::Rejected {
                        reason: "conversation_input_intake_unavailable".to_string(),
                        detail: "target conversation input intake is unavailable".to_string(),
                    };
                };
                let message = handle.controller.exit(chat_id.as_str()).await;
                if let Err(detail) = intake
                    .exit_authority_acp(&command.agent_id, chat_id.as_str())
                    .await
                {
                    return DeviceCommandResponse::Rejected {
                        reason: "acp_binding_update_rejected".to_string(),
                        detail,
                    };
                }
                DeviceCommandResponse::AcpChat {
                    response: AcpChatDeviceResponse::Exited { ok: true, message },
                }
            }
            AcpChatDeviceAction::Status => {
                let status =
                    build_acp_device_status(&handle, chat_id.as_str(), &self.local_device_id).await;
                DeviceCommandResponse::AcpChat {
                    response: AcpChatDeviceResponse::Status { status },
                }
            }
            AcpChatDeviceAction::ListSessions { runner, cwd } => {
                let Some(config) = handle.service.agent_config(&runner) else {
                    return DeviceCommandResponse::Rejected {
                        reason: "acp_runner_not_found".to_string(),
                        detail: format!("ACP runner `{runner}` not configured on target device"),
                    };
                };
                if !config.enabled {
                    return DeviceCommandResponse::Rejected {
                        reason: "acp_runner_disabled".to_string(),
                        detail: format!("ACP runner `{runner}` is disabled on target device"),
                    };
                }
                let cwd_filter = match cwd.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                    Some("all") => None,
                    Some(path) => Some(expand_tilde(path)),
                    None => Some(expand_tilde(&handle.workspace_dir.to_string_lossy())),
                };
                let Some(data_dir) = self.data_dir.as_deref() else {
                    return DeviceCommandResponse::Rejected {
                        reason: "workspace_unavailable".to_string(),
                        detail: "target has no workspace data root".to_string(),
                    };
                };
                let cache_dir = rua_config::dirs::resolve_cache_dir(data_dir);
                match rua_acp::client::AcpClientAdapter::list_sessions_for_runtime(
                    &config, cwd_filter, &cache_dir,
                )
                .await
                {
                    Ok(sessions) => {
                        let sessions = sessions
                            .into_iter()
                            .map(|session| AcpChatDeviceSessionEntry {
                                session_id: session.session_id.to_string(),
                                title: session.title,
                                cwd: session.cwd.to_string_lossy().to_string(),
                                updated_at: session.updated_at,
                            })
                            .collect();
                        DeviceCommandResponse::AcpChat {
                            response: AcpChatDeviceResponse::Sessions { sessions },
                        }
                    }
                    Err(error) => DeviceCommandResponse::Rejected {
                        reason: "acp_list_sessions_failed".to_string(),
                        detail: error.to_string(),
                    },
                }
            }
            AcpChatDeviceAction::PermissionSnapshot => {
                let Some(relay) = self.acp_permission_relay.as_ref() else {
                    return DeviceCommandResponse::Rejected {
                        reason: "acp_permission_relay_unavailable".to_string(),
                        detail: "target ACP permission relay is unavailable".to_string(),
                    };
                };
                DeviceCommandResponse::AcpChat {
                    response: AcpChatDeviceResponse::PermissionSnapshot {
                        snapshot: relay.snapshot(&command.agent_id, chat_id.as_str()).await,
                    },
                }
            }
            AcpChatDeviceAction::DecidePermission {
                request_id,
                option_id,
            } => {
                let Some(relay) = self.acp_permission_relay.as_ref() else {
                    return DeviceCommandResponse::Rejected {
                        reason: "acp_permission_relay_unavailable".to_string(),
                        detail: "target ACP permission relay is unavailable".to_string(),
                    };
                };
                if let Some(request) = relay.request(&request_id).await {
                    if request.agent_id == command.agent_id
                        && request.chat_id == chat_id.as_str()
                        && request
                            .options
                            .iter()
                            .any(|option| option.option_id == option_id)
                    {
                        if let Some(authority) = self.conversation_authority.as_ref() {
                            let _ = authority
                                .transition_active_turn_phase(
                                    &command.address,
                                    None,
                                    crate::services::conversation_authority::ConversationActiveTurnPhase::RunningTool,
                                )
                                .await;
                        }
                    }
                }
                match relay
                    .decide(&command.agent_id, chat_id.as_str(), &request_id, &option_id)
                    .await
                {
                    Ok(request) => DeviceCommandResponse::AcpChat {
                        response: AcpChatDeviceResponse::PermissionDecided { request },
                    },
                    Err(AcpPermissionRelayError::NotFound(_)) => DeviceCommandResponse::Rejected {
                        reason: "acp_permission_not_found".to_string(),
                        detail: request_id,
                    },
                    Err(AcpPermissionRelayError::WrongScope) => DeviceCommandResponse::Rejected {
                        reason: "acp_permission_wrong_scope".to_string(),
                        detail: request_id,
                    },
                    Err(AcpPermissionRelayError::UnknownOption(_)) => {
                        DeviceCommandResponse::Rejected {
                            reason: "acp_permission_unknown_option".to_string(),
                            detail: option_id,
                        }
                    }
                }
            }
        }
    }

    async fn publish_project_descriptor(&self, descriptor: &rua_project::ProjectDescriptor) {
        let Some(publication) = self.project_publication.as_ref() else {
            return;
        };
        let result = crate::sync_publication::publish_sync_fact_with_runtime(
            publication,
            crate::sync_publication::SyncPublicationFact::ProjectDescriptorCommitted {
                descriptor: descriptor.clone(),
            },
        )
        .await;
        if result.status == crate::sync_publication::PublicationStatus::AppendFailed {
            tracing::warn!(
                project_id = %descriptor.project_id,
                errors = ?result.errors,
                "Project lifecycle committed but descriptor publication failed"
            );
        }
    }

    async fn handle_remote_chat_control(
        &self,
        command: RemoteChatControlCommand,
    ) -> DeviceCommandResponse {
        if let Err(error) = command.validate_for_target(&self.local_device_id) {
            return DeviceCommandResponse::Rejected {
                reason: "invalid_remote_chat_control".to_string(),
                detail: error.to_string(),
            };
        }
        if let Some(intake) = self.conversation_input_intake.as_ref() {
            let validation = ConversationControlDeviceCommand {
                command_id: command.command_id.clone(),
                agent_id: command.agent_id.clone(),
                address: command.address.clone(),
                source_device_id: command.source_device_id.clone(),
                target_device_id: command.target_device_id.clone(),
                authority_epoch: command.authority_epoch,
                action: ConversationControlDeviceAction::View,
            };
            match intake
                .resolve_conversation_control_target(&validation)
                .await
            {
                Ok(_) => {}
                Err(DeviceCommandDeliveryError::AuthorityRedirect(redirect)) => {
                    return DeviceCommandResponse::AuthorityRedirect {
                        redirect: *redirect,
                    };
                }
                Err(DeviceCommandDeliveryError::Rejected { reason, detail }) => {
                    return DeviceCommandResponse::Rejected { reason, detail };
                }
                Err(error) => {
                    return DeviceCommandResponse::Rejected {
                        reason: "remote_chat_control_failed".to_string(),
                        detail: error.to_string(),
                    };
                }
            }
        }
        if let Some(redirect) = self
            .authority_redirect_for(&command.address, command.authority_epoch)
            .await
        {
            return DeviceCommandResponse::AuthorityRedirect { redirect };
        }
        let Some(authority) = self.conversation_authority.as_ref() else {
            return DeviceCommandResponse::Rejected {
                reason: "conversation_authority_unavailable".to_string(),
                detail: "target conversation authority is unavailable".to_string(),
            };
        };
        let Some(active_turn) = authority.active_turn(&command.address).await else {
            return DeviceCommandResponse::RemoteChatControl {
                response: RemoteChatControlResponse {
                    command_id: command.command_id,
                    target_device_id: command.target_device_id,
                    address: command.address,
                    action: command.action,
                    applied: false,
                    detail: Some("target conversation has no active turn".to_string()),
                },
            };
        };
        if active_turn.authority_device_id != command.target_device_id
            || active_turn.authority_epoch != command.authority_epoch
        {
            return DeviceCommandResponse::Rejected {
                reason: "stale_authority_epoch".to_string(),
                detail: "remote control targets a stale conversation authority Epoch".to_string(),
            };
        }

        match command.action {
            RemoteChatControlAction::StopCurrentTurn => {
                if let Some(intake) = self.conversation_input_intake.as_ref() {
                    intake.clear_approvals_for_address(&command.address).await;
                }
                let target_turn_id = active_turn.turn_id.clone();
                let mut control_events = authority.subscribe_control_events(&command.address).await;
                let acp_applied = self.cancel_acp_passthrough_turn(&command).await;
                let authority_applied = authority
                    .request_stop_matching(
                        self.remote_chat_control_streams.as_deref(),
                        CONSOLE_CHANNEL_ID,
                        &command.address,
                        Some(&target_turn_id),
                        Some(command.authority_epoch),
                    )
                    .await
                    .is_some();
                let applied = acp_applied || authority_applied;
                if applied
                    && command.await_terminal
                    && !authority
                        .wait_for_turn_handoff(
                            &mut control_events,
                            &command.address,
                            &target_turn_id,
                            REMOTE_CHAT_CONTROL_CONFIRM_TIMEOUT,
                        )
                        .await
                {
                    return DeviceCommandResponse::Rejected {
                        reason: "stop_confirmation_timeout".to_string(),
                        detail: "target Turn did not finish persistence and handoff before timeout"
                            .to_string(),
                    };
                }
                DeviceCommandResponse::RemoteChatControl {
                    response: RemoteChatControlResponse {
                        command_id: command.command_id,
                        target_device_id: command.target_device_id,
                        address: command.address,
                        action: RemoteChatControlAction::StopCurrentTurn,
                        applied,
                        detail: (!applied).then(|| {
                            "no active target-side turn was available to stop".to_string()
                        }),
                    },
                }
            }
            RemoteChatControlAction::InterjectCurrentTurn => {
                if let Some(detail) = self.remote_interjection_fifo_rejection(&command).await {
                    return DeviceCommandResponse::RemoteChatControl {
                        response: RemoteChatControlResponse {
                            command_id: command.command_id,
                            target_device_id: command.target_device_id,
                            address: command.address,
                            action: RemoteChatControlAction::InterjectCurrentTurn,
                            applied: false,
                            detail: Some(detail),
                        },
                    };
                }

                let acp_applied = self.cancel_acp_passthrough_turn(&command).await;
                let stream_interjected = if let Some(streams) = &self.remote_chat_control_streams {
                    streams
                        .interject_all_priorities(CONSOLE_CHANNEL_ID, &command.address)
                        .await
                        > 0
                } else {
                    false
                };
                let applied = acp_applied || stream_interjected;
                if applied {
                    if let Some(authority) = self.conversation_authority.as_ref() {
                        let _ = authority.request_interjection(&command.address).await;
                    }
                }
                DeviceCommandResponse::RemoteChatControl {
                    response: RemoteChatControlResponse {
                        command_id: command.command_id,
                        target_device_id: command.target_device_id,
                        address: command.address,
                        action: RemoteChatControlAction::InterjectCurrentTurn,
                        applied,
                        detail: (!applied).then(|| {
                            "no active target-side run was available to interject".to_string()
                        }),
                    },
                }
            }
        }
    }

    async fn remote_interjection_fifo_rejection(
        &self,
        command: &RemoteChatControlCommand,
    ) -> Option<String> {
        let device_delivery_id = command.device_delivery_id.as_deref()?;
        let Some(authority) = self.conversation_authority.as_ref() else {
            return Some("target conversation authority is unavailable".to_string());
        };
        match authority
            .queued_device_delivery_head(&command.address)
            .await
        {
            Some(head) if head == device_delivery_id => None,
            Some(head) => Some(format!(
                "device delivery '{}' is not the FIFO queue head '{}'",
                device_delivery_id, head
            )),
            None => Some(format!(
                "device delivery '{device_delivery_id}' is not pending on the target"
            )),
        }
    }

    async fn cancel_acp_passthrough_turn(&self, command: &RemoteChatControlCommand) -> bool {
        let Some(registry) = &self.acp_chat_registry else {
            return false;
        };
        let Some(handle) = registry.get(&command.agent_id) else {
            return false;
        };
        let Some(session) = handle.manager.get(command.chat_id()).await else {
            return false;
        };
        let sid = rua_core::id::SessionId::new(command.chat_id());
        if let Err(error) = handle.service.cancel_turn(&sid, &session.runner).await {
            tracing::warn!(
                agent_id = %command.agent_id,
                chat_id = %command.chat_id(),
                runner = %session.runner,
                error = %error,
                "failed to cancel target ACP passthrough turn for remote chat control"
            );
        }
        true
    }
}

pub fn acp_runner_infos_from_service(
    service: &rua_acp::service::AcpService,
) -> Vec<AcpChatRunnerInfo> {
    service
        .agent_configs()
        .into_iter()
        .map(|(key, config)| acp_runner_info(key, config))
        .collect()
}

fn acp_runner_info(key: String, config: AcpAgentConfig) -> AcpChatRunnerInfo {
    AcpChatRunnerInfo {
        is_builtin: default_builtin_runners().contains_key(&key),
        key,
        enabled: config.enabled,
        command: config.command,
        args: config.args,
        env: config.env,
        trusted: config.trusted,
        tool_parse_mode: config.tool_parse_mode,
        stdio_buffer_limit_bytes: config.stdio_buffer_limit_bytes,
        extra: config.extra,
    }
}

impl RemoteConversationInputRequest {
    pub fn validate_for_target(&self, local_device_id: &str) -> Result<(), DeviceCommandBusError> {
        validate_id(&self.source_device_id)?;
        validate_id(&self.target_device_id)?;
        if self.base_manifest_hash.len() != 71 || !self.base_manifest_hash.starts_with("sha256:") {
            return Err(DeviceCommandBusError::InvalidId(
                self.base_manifest_hash.clone(),
            ));
        }
        validate_id(&self.submission_id)?;
        validate_id(&self.device_delivery_id)?;
        if self.device_delivery_sequence == 0 {
            return Err(DeviceCommandBusError::InvalidSequence);
        }
        if self.target_device_id != local_device_id {
            return Err(DeviceCommandBusError::WrongTarget {
                target: self.target_device_id.clone(),
                local: local_device_id.to_string(),
            });
        }
        for media in &self.media_requirements {
            validate_media_filename(&media.filename)?;
            if media.agent_id.trim().is_empty() {
                return Err(DeviceCommandBusError::InvalidId(media.agent_id.clone()));
            }
        }
        Ok(())
    }
}

impl AcpChatDeviceCommand {
    pub fn validate_for_target(&self, local_device_id: &str) -> Result<(), DeviceCommandBusError> {
        validate_id(&self.command_id)?;
        validate_id(&self.source_device_id)?;
        validate_id(&self.target_device_id)?;
        if self.agent_id.trim().is_empty() || self.chat_id().trim().is_empty() {
            return Err(DeviceCommandBusError::InvalidId(format!(
                "{}:{}",
                self.agent_id,
                self.chat_id()
            )));
        }
        if self.target_device_id != local_device_id {
            return Err(DeviceCommandBusError::WrongTarget {
                target: self.target_device_id.clone(),
                local: local_device_id.to_string(),
            });
        }
        Ok(())
    }
}

impl ConversationManifestDeviceCommand {
    pub fn validate_for_target(&self, local_device_id: &str) -> Result<(), DeviceCommandBusError> {
        validate_id(&self.command_id)?;
        validate_id(&self.source_device_id)?;
        validate_id(&self.target_device_id)?;
        if self.agent_id.trim().is_empty() || self.chat_id().trim().is_empty() {
            return Err(DeviceCommandBusError::InvalidId(format!(
                "{}:{}",
                self.agent_id,
                self.chat_id()
            )));
        }
        if self
            .required_manifest
            .as_ref()
            .is_some_and(|manifest| manifest.address != self.address)
        {
            return Err(DeviceCommandBusError::InvalidId(self.address.to_string()));
        }
        if self.target_device_id != local_device_id {
            return Err(DeviceCommandBusError::WrongTarget {
                target: self.target_device_id.clone(),
                local: local_device_id.to_string(),
            });
        }
        Ok(())
    }
}

impl ConversationAuthorityAdoptDeviceCommand {
    pub fn validate_for_target(&self, local_device_id: &str) -> Result<(), DeviceCommandBusError> {
        validate_id(&self.command_id)?;
        validate_id(&self.source_device_id)?;
        validate_id(&self.authority_source_device_id)?;
        validate_id(&self.target_device_id)?;
        if self.agent_id.trim().is_empty()
            || self.chat_id().trim().is_empty()
            || self.required_manifest.address != self.address
        {
            return Err(DeviceCommandBusError::InvalidId(format!(
                "{}:{}",
                self.agent_id,
                self.chat_id()
            )));
        }
        if self.target_device_id != local_device_id {
            return Err(DeviceCommandBusError::WrongTarget {
                target: self.target_device_id.clone(),
                local: local_device_id.to_string(),
            });
        }
        Ok(())
    }
}

impl ConversationAuthoritySupersedeDeviceCommand {
    pub fn validate_for_target(&self, local_device_id: &str) -> Result<(), DeviceCommandBusError> {
        validate_id(&self.command_id)?;
        validate_id(&self.source_device_id)?;
        validate_id(&self.target_device_id)?;
        if self.agent_id.trim().is_empty()
            || self.chat_id().trim().is_empty()
            || self.replacement.device_id != self.source_device_id
            || self.replacement.authority_epoch <= self.previous_authority_epoch
        {
            return Err(DeviceCommandBusError::InvalidId(format!(
                "{}:{}",
                self.agent_id,
                self.chat_id()
            )));
        }
        self.replacement
            .validate()
            .map_err(|error| DeviceCommandBusError::InvalidId(error.to_string()))?;
        if self.target_device_id != local_device_id {
            return Err(DeviceCommandBusError::WrongTarget {
                target: self.target_device_id.clone(),
                local: local_device_id.to_string(),
            });
        }
        Ok(())
    }
}

impl ConversationControlDeviceCommand {
    pub fn validate_for_target(&self, local_device_id: &str) -> Result<(), DeviceCommandBusError> {
        validate_id(&self.command_id)?;
        validate_id(&self.source_device_id)?;
        validate_id(&self.target_device_id)?;
        let project_discovery = self.address.project_id().is_some()
            && matches!(self.action, ConversationControlDeviceAction::Discover);
        if (!project_discovery && self.agent_id.trim().is_empty())
            || self.chat_id().trim().is_empty()
        {
            return Err(DeviceCommandBusError::InvalidId(format!(
                "{}:{}",
                self.agent_id,
                self.chat_id()
            )));
        }
        if self.target_device_id != local_device_id {
            return Err(DeviceCommandBusError::WrongTarget {
                target: self.target_device_id.clone(),
                local: local_device_id.to_string(),
            });
        }
        Ok(())
    }
}

impl RemoteChatControlCommand {
    pub fn validate_for_target(&self, local_device_id: &str) -> Result<(), DeviceCommandBusError> {
        validate_id(&self.command_id)?;
        validate_id(&self.source_device_id)?;
        validate_id(&self.target_device_id)?;
        if let Some(device_delivery_id) = &self.device_delivery_id {
            validate_id(device_delivery_id)?;
        }
        if self.agent_id.trim().is_empty() || self.chat_id().trim().is_empty() {
            return Err(DeviceCommandBusError::InvalidId(format!(
                "{}:{}",
                self.agent_id,
                self.chat_id()
            )));
        }
        if self.target_device_id != local_device_id {
            return Err(DeviceCommandBusError::WrongTarget {
                target: self.target_device_id.clone(),
                local: local_device_id.to_string(),
            });
        }
        Ok(())
    }
}

async fn frame_safe_terminal_attachment_response(
    transfers: &tokio::sync::Mutex<
        std::collections::HashMap<String, PendingTerminalAttachmentTransfer>,
    >,
    attachment: TerminalSessionAttachment,
) -> DeviceCommandResponse {
    let candidate = DeviceCommandResponse::Terminal {
        response: TerminalDeviceResponse::Attached {
            session: attachment.session.clone(),
            snapshot: attachment.snapshot.clone(),
            tail: attachment.tail.clone(),
            live_cursor: attachment.live_cursor,
        },
    };
    if serde_json::to_vec(&candidate).is_ok_and(|bytes| bytes.len() <= COMMAND_FRAME_LIMIT) {
        return candidate;
    }
    let snapshot_cursor = attachment.snapshot.cursor;
    let live_cursor = attachment.live_cursor;
    let payload = match serde_json::to_vec(&TerminalAttachmentTransferPayload {
        snapshot: attachment.snapshot,
        tail: attachment.tail,
    }) {
        Ok(payload) => payload,
        Err(error) => {
            return DeviceCommandResponse::Rejected {
                reason: "terminal_attachment_transfer_failed".to_string(),
                detail: format!("failed to encode attachment transfer: {error}"),
            };
        }
    };
    let transfer_id = format!("terminal-attachment-{}", uuid::Uuid::new_v4());
    let total_bytes = payload.len();
    let mut transfers = transfers.lock().await;
    prune_terminal_attachment_transfers(&mut transfers);
    while transfers.len() >= TERMINAL_ATTACHMENT_TRANSFER_LIMIT {
        let Some(oldest) = transfers
            .iter()
            .min_by_key(|(_, transfer)| transfer.created_at)
            .map(|(transfer_id, _)| transfer_id.clone())
        else {
            break;
        };
        transfers.remove(&oldest);
    }
    transfers.insert(
        transfer_id.clone(),
        PendingTerminalAttachmentTransfer {
            payload,
            created_at: Instant::now(),
        },
    );
    drop(transfers);
    tracing::debug!(
        event = "terminal_attachment_transfer_created",
        transfer_id,
        session_id = %attachment.session.session_id,
        snapshot_activity_sequence = snapshot_cursor.next_activity_sequence,
        snapshot_output_offset = snapshot_cursor.next_output_offset,
        live_activity_sequence = live_cursor.next_activity_sequence,
        live_output_offset = live_cursor.next_output_offset,
        total_bytes,
    );
    DeviceCommandResponse::Terminal {
        response: TerminalDeviceResponse::AttachedTransfer {
            session: attachment.session,
            snapshot_cursor,
            live_cursor,
            transfer_id,
            total_bytes,
        },
    }
}

async fn terminal_attachment_chunk_response(
    transfers: &tokio::sync::Mutex<
        std::collections::HashMap<String, PendingTerminalAttachmentTransfer>,
    >,
    transfer_id: String,
    offset_bytes: usize,
) -> DeviceCommandResponse {
    let mut transfers = transfers.lock().await;
    prune_terminal_attachment_transfers(&mut transfers);
    let Some(transfer) = transfers.get(&transfer_id) else {
        return DeviceCommandResponse::Rejected {
            reason: "terminal_attachment_transfer_not_found".to_string(),
            detail: format!("terminal attachment transfer not found: {transfer_id}"),
        };
    };
    if offset_bytes > transfer.payload.len() {
        return DeviceCommandResponse::Rejected {
            reason: "terminal_attachment_transfer_invalid_offset".to_string(),
            detail: format!(
                "terminal attachment transfer {transfer_id} offset {offset_bytes} exceeds total {}",
                transfer.payload.len()
            ),
        };
    }
    let next_offset_bytes = offset_bytes
        .saturating_add(TERMINAL_ATTACHMENT_CHUNK_BYTES)
        .min(transfer.payload.len());
    let total_bytes = transfer.payload.len();
    let data_base64 = BASE64_STANDARD.encode(&transfer.payload[offset_bytes..next_offset_bytes]);
    tracing::trace!(
        event = "terminal_attachment_transfer_chunk",
        transfer_id,
        offset_bytes,
        next_offset_bytes,
        total_bytes,
        done = next_offset_bytes == total_bytes,
    );
    DeviceCommandResponse::Terminal {
        response: TerminalDeviceResponse::AttachmentChunk {
            transfer_id,
            offset_bytes,
            next_offset_bytes,
            total_bytes,
            data_base64,
            done: next_offset_bytes == total_bytes,
        },
    }
}

fn prune_terminal_attachment_transfers(
    transfers: &mut std::collections::HashMap<String, PendingTerminalAttachmentTransfer>,
) {
    transfers
        .retain(|_, transfer| transfer.created_at.elapsed() <= TERMINAL_ATTACHMENT_TRANSFER_TTL);
}

impl TerminalDeviceCommand {
    pub fn validate_for_target(&self, local_device_id: &str) -> Result<(), DeviceCommandBusError> {
        validate_id(&self.command_id)?;
        validate_id(&self.source_device_id)?;
        validate_id(&self.target_device_id)?;
        if self.target_device_id != local_device_id {
            return Err(DeviceCommandBusError::WrongTarget {
                target: self.target_device_id.clone(),
                local: local_device_id.to_string(),
            });
        }
        match &self.action {
            TerminalDeviceAction::ListSessions | TerminalDeviceAction::ListProfiles => {}
            TerminalDeviceAction::Open {
                browser_viewer_id, ..
            } => {
                validate_id(browser_viewer_id)?;
            }
            TerminalDeviceAction::AttachmentChunk { transfer_id, .. } => {
                validate_id(transfer_id)?;
            }
            TerminalDeviceAction::TakeInputLease {
                session_id,
                browser_viewer_id,
            }
            | TerminalDeviceAction::Attach {
                session_id,
                browser_viewer_id,
                ..
            }
            | TerminalDeviceAction::SendInput {
                session_id,
                browser_viewer_id,
                ..
            }
            | TerminalDeviceAction::AdaptPtyGeometry {
                session_id,
                browser_viewer_id,
                ..
            }
            | TerminalDeviceAction::UpdatePalette {
                session_id,
                browser_viewer_id,
                ..
            }
            | TerminalDeviceAction::DetachViewer {
                session_id,
                browser_viewer_id,
            } => {
                validate_id(session_id)?;
                validate_id(browser_viewer_id)?;
            }
            TerminalDeviceAction::KillSession {
                session_id,
                browser_viewer_id,
            } => {
                validate_id(session_id)?;
                validate_id(browser_viewer_id)?;
            }
            TerminalDeviceAction::Rename { session_id, .. }
            | TerminalDeviceAction::SetScrollback { session_id, .. } => {
                validate_id(session_id)?;
            }
        }
        Ok(())
    }
}

impl SyncTransferDeviceCommand {
    pub fn validate_for_target(&self, local_device_id: &str) -> Result<(), DeviceCommandBusError> {
        validate_id(&self.command_id)?;
        validate_id(&self.source_device_id)?;
        validate_id(&self.target_device_id)?;
        if self.target_device_id != local_device_id {
            return Err(DeviceCommandBusError::WrongTarget {
                target: self.target_device_id.clone(),
                local: local_device_id.to_string(),
            });
        }
        match &self.action {
            SyncTransferDeviceAction::ListDir { path } => {
                if path.trim().is_empty() {
                    return Err(DeviceCommandBusError::InvalidId("empty path".to_string()));
                }
            }
            SyncTransferDeviceAction::StartSend {
                operation_id,
                source_paths,
                transfer_target_device_id,
                target_dir,
                ..
            } => {
                validate_id(operation_id)?;
                if source_paths.is_empty() {
                    return Err(DeviceCommandBusError::InvalidId(
                        "empty source paths".to_string(),
                    ));
                }
                if source_paths.iter().any(|path| path.trim().is_empty()) {
                    return Err(DeviceCommandBusError::InvalidId(
                        "empty source path".to_string(),
                    ));
                }
                if target_dir.trim().is_empty() {
                    return Err(DeviceCommandBusError::InvalidId(
                        "empty target dir".to_string(),
                    ));
                }
                validate_id(transfer_target_device_id)?;
            }
            SyncTransferDeviceAction::StartChatGapSend {
                operation_id,
                agent_id,
                chat_id,
                required_manifest,
                transfer_target_device_id,
                target_dir,
                ..
            } => {
                validate_id(operation_id)?;
                if agent_id.trim().is_empty()
                    || chat_id.trim().is_empty()
                    || required_manifest.address
                        != ChatAddress::agent(agent_id.as_str(), chat_id.as_str())
                {
                    return Err(DeviceCommandBusError::InvalidId(format!(
                        "{agent_id}:{chat_id}"
                    )));
                }
                if target_dir.trim().is_empty() {
                    return Err(DeviceCommandBusError::InvalidId(
                        "empty target dir".to_string(),
                    ));
                }
                validate_id(transfer_target_device_id)?;
            }
            SyncTransferDeviceAction::GetSendStatus { operation_id }
            | SyncTransferDeviceAction::CancelSend { operation_id } => {
                validate_id(operation_id)?;
            }
        }
        Ok(())
    }
}

impl ProjectDeviceCommand {
    pub fn validate_for_target(&self, local_device_id: &str) -> Result<(), DeviceCommandBusError> {
        validate_id(&self.command_id)?;
        validate_id(&self.source_device_id)?;
        validate_id(&self.target_device_id)?;
        if self.target_device_id != local_device_id {
            return Err(DeviceCommandBusError::WrongTarget {
                target: self.target_device_id.clone(),
                local: local_device_id.to_string(),
            });
        }
        match &self.action {
            ProjectDeviceAction::CreateDirectory { parent, name } => {
                if parent.trim().is_empty() || name.trim().is_empty() {
                    return Err(DeviceCommandBusError::InvalidId(
                        "empty directory parent or name".to_string(),
                    ));
                }
            }
            ProjectDeviceAction::BrowseDirectory { path } => {
                if path.trim().is_empty() {
                    return Err(DeviceCommandBusError::InvalidId("empty path".to_string()));
                }
            }
            ProjectDeviceAction::Create { resource_root, .. } => {
                if resource_root.trim().is_empty() {
                    return Err(DeviceCommandBusError::InvalidId(
                        "empty Project Resource Root".to_string(),
                    ));
                }
            }
            ProjectDeviceAction::CreateResources {
                resource_roots,
                primary_index,
                ..
            } => {
                if resource_roots.is_empty()
                    || resource_roots.iter().any(|root| root.trim().is_empty())
                    || *primary_index >= resource_roots.len()
                {
                    return Err(DeviceCommandBusError::InvalidId(
                        "Invalid Project resource collection".into(),
                    ));
                }
            }
            ProjectDeviceAction::InspectGit { project_id }
            | ProjectDeviceAction::ArchiveIdleChats { project_id, .. }
            | ProjectDeviceAction::InspectResource { project_id, .. }
            | ProjectDeviceAction::EditResources { project_id, .. }
            | ProjectDeviceAction::SetPrimaryDirectory { project_id, .. }
            | ProjectDeviceAction::InspectGitHistory { project_id, .. }
            | ProjectDeviceAction::InspectLifecycle { project_id }
            | ProjectDeviceAction::Archive { project_id }
            | ProjectDeviceAction::Reopen { project_id } => {
                validate_id(project_id.as_str())?;
            }
            ProjectDeviceAction::SwitchChatAgent {
                project_id,
                chat_id,
                expected_agent_id,
                agent_id,
            } => {
                validate_id(project_id.as_str())?;
                validate_id(chat_id)?;
                validate_id(expected_agent_id)?;
                validate_id(agent_id)?;
            }
            ProjectDeviceAction::UpdateChat {
                project_id,
                chat_id,
                ..
            }
            | ProjectDeviceAction::DeleteChat {
                project_id,
                chat_id,
                ..
            } => {
                validate_id(project_id.as_str())?;
                validate_id(chat_id)?;
            }
            ProjectDeviceAction::ListTasks {
                project_id,
                chat_id,
                ..
            } => {
                validate_id(project_id.as_str())?;
                validate_id(chat_id)?;
            }
            ProjectDeviceAction::GetTask {
                project_id,
                chat_id,
                task_id,
            }
            | ProjectDeviceAction::CancelTask {
                project_id,
                chat_id,
                task_id,
            }
            | ProjectDeviceAction::WatchTask {
                project_id,
                chat_id,
                task_id,
            }
            | ProjectDeviceAction::UnwatchTask {
                project_id,
                chat_id,
                task_id,
            }
            | ProjectDeviceAction::RetryTask {
                project_id,
                chat_id,
                task_id,
                ..
            }
            | ProjectDeviceAction::GetTaskResult {
                project_id,
                chat_id,
                task_id,
            } => {
                validate_id(project_id.as_str())?;
                validate_id(chat_id)?;
                validate_id(task_id)?;
            }
            ProjectDeviceAction::SendTaskInput {
                project_id,
                chat_id,
                task_id,
                submission,
            } => {
                validate_id(project_id.as_str())?;
                validate_id(chat_id)?;
                validate_id(task_id)?;
                validate_id(&submission.submission_id)?;
            }
        }
        Ok(())
    }
}

fn project_task_owner(project_id: rua_core::ProjectId, chat_id: String) -> rua_task::TaskOwner {
    rua_task::TaskOwner::from_address(rua_core::ChatAddress::project(project_id, chat_id))
}

fn project_task_error_response(error: rua_task::TaskError) -> DeviceCommandResponse {
    let reason = match &error {
        rua_task::TaskError::NotFound(_) => "task_not_found",
        rua_task::TaskError::Invalid(_) => "task_invalid",
        rua_task::TaskError::InvalidTransition(_) => "task_invalid_transition",
        rua_task::TaskError::LimitExceeded(_) => "task_limit_exceeded",
        rua_task::TaskError::AuthorityRejected(_) => "task_authority_rejected",
        rua_task::TaskError::InputUnavailable(_) => "task_input_unavailable",
        rua_task::TaskError::InputConflict(_) => "task_input_conflict",
        rua_task::TaskError::AuthorityUnavailable(_)
        | rua_task::TaskError::ResourceUnavailable(_) => "task_resource_unavailable",
        rua_task::TaskError::WaitTimedOut(_) => "task_wait_timed_out",
        _ => "task_operation_failed",
    };
    DeviceCommandResponse::Rejected {
        reason: reason.to_string(),
        detail: error.to_string(),
    }
}

pub fn list_device_directory(
    path: impl AsRef<Path>,
) -> Result<SyncTransferDirectoryListing, DeviceCommandBusError> {
    let expanded = expand_tilde(&path.as_ref().to_string_lossy());
    let requested = PathBuf::from(expanded);
    let canonical = requested.canonicalize()?;
    if !canonical.is_dir() {
        return Err(DeviceCommandBusError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("not a directory: {}", canonical.display()),
        )));
    }

    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&canonical)? {
        let entry = entry?;
        let metadata = std::fs::symlink_metadata(entry.path())?;
        let file_type = metadata.file_type();
        let kind = if file_type.is_dir() {
            SyncTransferDeviceEntryKind::Directory
        } else if file_type.is_file() {
            SyncTransferDeviceEntryKind::File
        } else if file_type.is_symlink() {
            SyncTransferDeviceEntryKind::Symlink
        } else {
            SyncTransferDeviceEntryKind::Other
        };
        let size = (kind == SyncTransferDeviceEntryKind::File).then_some(metadata.len());
        let modified_at_ms = metadata.modified().ok().and_then(system_time_to_epoch_ms);
        entries.push(SyncTransferDeviceEntry {
            name: entry.file_name().to_string_lossy().to_string(),
            path: entry.path(),
            kind,
            size,
            modified_at_ms,
        });
    }
    entries.sort_by(|a, b| {
        let rank = |kind: SyncTransferDeviceEntryKind| match kind {
            SyncTransferDeviceEntryKind::Directory => 0,
            SyncTransferDeviceEntryKind::File => 1,
            SyncTransferDeviceEntryKind::Symlink => 2,
            SyncTransferDeviceEntryKind::Other => 3,
        };
        rank(a.kind)
            .cmp(&rank(b.kind))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });

    Ok(SyncTransferDirectoryListing {
        path: canonical,
        entries,
    })
}

pub async fn deliver_terminal_device_command_via_session(
    session_manager: &rua_device_network::DeviceSessionManager,
    command: &TerminalDeviceCommand,
    timeout: Duration,
) -> Result<TerminalDeviceResponse, DeviceCommandDeliveryError> {
    tokio::time::timeout(
        timeout,
        deliver_terminal_device_command_reassembled_via_session(session_manager, command, timeout),
    )
    .await
    .map_err(|_| DeviceCommandDeliveryError::Timeout)?
}

pub async fn deliver_remote_conversation_input_via_session(
    session_manager: &rua_device_network::DeviceSessionManager,
    request: &RemoteConversationInputRequest,
    timeout: Duration,
) -> Result<RemoteConversationInputAck, DeviceCommandDeliveryError> {
    match deliver_envelope_via_session(
        session_manager,
        &request.target_device_id,
        DeviceCommandEnvelope::ConversationInput(Box::new(request.clone())),
        timeout,
    )
    .await?
    {
        DeviceCommandResponse::Accepted(ack) => Ok(ack),
        response => unexpected_response(response, "remote conversation input"),
    }
}

pub async fn deliver_remote_chat_control_via_session(
    session_manager: &rua_device_network::DeviceSessionManager,
    command: &RemoteChatControlCommand,
    timeout: Duration,
) -> Result<RemoteChatControlResponse, DeviceCommandDeliveryError> {
    match deliver_envelope_via_session(
        session_manager,
        &command.target_device_id,
        DeviceCommandEnvelope::RemoteChatControl(command.clone()),
        timeout,
    )
    .await?
    {
        DeviceCommandResponse::RemoteChatControl { response } => Ok(response),
        response => unexpected_response(response, "remote Chat control"),
    }
}

pub async fn deliver_conversation_control_via_session(
    session_manager: &rua_device_network::DeviceSessionManager,
    command: &ConversationControlDeviceCommand,
    timeout: Duration,
) -> Result<ConversationControlDeviceResponse, DeviceCommandDeliveryError> {
    match deliver_envelope_via_session(
        session_manager,
        &command.target_device_id,
        DeviceCommandEnvelope::ConversationControl(command.clone()),
        timeout,
    )
    .await?
    {
        DeviceCommandResponse::ConversationControl { response } => Ok(*response),
        response => unexpected_response(response, "Conversation Control"),
    }
}

pub async fn deliver_acp_chat_command_via_session(
    session_manager: &rua_device_network::DeviceSessionManager,
    command: &AcpChatDeviceCommand,
    timeout: Duration,
) -> Result<AcpChatDeviceResponse, DeviceCommandDeliveryError> {
    match deliver_envelope_via_session(
        session_manager,
        &command.target_device_id,
        DeviceCommandEnvelope::AcpChat(command.clone()),
        timeout,
    )
    .await?
    {
        DeviceCommandResponse::AcpChat { response } => Ok(response),
        response => unexpected_response(response, "ACP Chat command"),
    }
}

pub async fn deliver_conversation_manifest_command_via_session(
    session_manager: &rua_device_network::DeviceSessionManager,
    command: &ConversationManifestDeviceCommand,
    timeout: Duration,
) -> Result<ConversationManifestDeviceResponse, DeviceCommandDeliveryError> {
    match deliver_envelope_via_session(
        session_manager,
        &command.target_device_id,
        DeviceCommandEnvelope::ConversationManifest(command.clone()),
        timeout,
    )
    .await?
    {
        DeviceCommandResponse::ConversationManifest { response } => Ok(response),
        response => unexpected_response(response, "Conversation Manifest"),
    }
}

pub async fn deliver_conversation_authority_adopt_via_session(
    session_manager: &rua_device_network::DeviceSessionManager,
    command: &ConversationAuthorityAdoptDeviceCommand,
    timeout: Duration,
) -> Result<ConversationAuthorityAdoptDeviceResponse, DeviceCommandDeliveryError> {
    match deliver_envelope_via_session(
        session_manager,
        &command.target_device_id,
        DeviceCommandEnvelope::ConversationAuthorityAdopt(command.clone()),
        timeout,
    )
    .await?
    {
        DeviceCommandResponse::ConversationAuthorityAdopt { response } => Ok(response),
        response => unexpected_response(response, "Conversation Authority adoption"),
    }
}

pub fn spawn_conversation_authority_supersession(
    session_manager_cell: Option<
        &Arc<std::sync::RwLock<Option<rua_device_network::DeviceSessionManager>>>,
    >,
    agent_id: &str,
    chat_id: &str,
    previous_authority_device_id: &str,
    previous_authority_epoch: ConversationAuthorityEpoch,
    replacement: &ConversationAuthorityPlacement,
) {
    if previous_authority_device_id == replacement.device_id {
        return;
    }
    let Some(session_manager) = session_manager_cell
        .and_then(|cell| cell.read().ok().and_then(|guard| guard.as_ref().cloned()))
    else {
        tracing::warn!(
            agent_id,
            chat_id,
            previous_authority_device_id,
            "authority supersession skipped because the shared Device Session runtime is unavailable"
        );
        return;
    };
    let command = ConversationAuthoritySupersedeDeviceCommand {
        command_id: format!("authority-supersede-{}", uuid::Uuid::new_v4()),
        agent_id: agent_id.to_string(),
        address: ChatAddress::agent(agent_id, chat_id),
        source_device_id: replacement.device_id.clone(),
        target_device_id: previous_authority_device_id.to_string(),
        previous_authority_epoch,
        replacement: replacement.clone(),
    };
    tokio::spawn(async move {
        if let Err(error) = deliver_conversation_authority_supersede_via_session(
            &session_manager,
            &command,
            Duration::from_secs(5),
        )
        .await
        {
            tracing::warn!(
                agent_id = %command.agent_id,
                chat_id = %command.chat_id(),
                previous_authority_device_id = %command.target_device_id,
                error = %error,
                "best-effort authority supersession failed; ordinary Sync remains the fallback"
            );
        }
    });
}

pub async fn deliver_conversation_authority_supersede_via_session(
    session_manager: &rua_device_network::DeviceSessionManager,
    command: &ConversationAuthoritySupersedeDeviceCommand,
    timeout: Duration,
) -> Result<ConversationAuthoritySupersedeDeviceResponse, DeviceCommandDeliveryError> {
    match deliver_envelope_via_session(
        session_manager,
        &command.target_device_id,
        DeviceCommandEnvelope::ConversationAuthoritySupersede(command.clone()),
        timeout,
    )
    .await?
    {
        DeviceCommandResponse::ConversationAuthoritySuperseded { response } => Ok(response),
        response => unexpected_response(response, "Conversation Authority supersession"),
    }
}

pub async fn deliver_sync_transfer_device_command_via_session(
    session_manager: &rua_device_network::DeviceSessionManager,
    command: &SyncTransferDeviceCommand,
    timeout: Duration,
) -> Result<SyncTransferDeviceResponse, DeviceCommandDeliveryError> {
    match deliver_envelope_via_session(
        session_manager,
        &command.target_device_id,
        DeviceCommandEnvelope::SyncTransfer(command.clone()),
        timeout,
    )
    .await?
    {
        DeviceCommandResponse::SyncTransfer { response } => Ok(response),
        response => unexpected_response(response, "Sync Transfer command"),
    }
}

pub async fn deliver_chat_media_prepare_via_session(
    session_manager: &rua_device_network::DeviceSessionManager,
    command: &ChatMediaPrepareCommand,
    timeout: Duration,
) -> Result<ChatMediaPreparedResponse, DeviceCommandDeliveryError> {
    match deliver_envelope_via_session(
        session_manager,
        &command.target_device_id,
        DeviceCommandEnvelope::ChatMediaPrepare(command.clone()),
        timeout,
    )
    .await?
    {
        DeviceCommandResponse::ChatMediaPrepared { response } => Ok(response),
        response => unexpected_response(response, "Chat Media prepare command"),
    }
}

pub async fn deliver_project_device_command_via_session(
    session_manager: &rua_device_network::DeviceSessionManager,
    command: &ProjectDeviceCommand,
    timeout: Duration,
) -> Result<ProjectDeviceResponse, DeviceCommandDeliveryError> {
    match deliver_envelope_via_session(
        session_manager,
        &command.target_device_id,
        DeviceCommandEnvelope::Project(command.clone()),
        timeout,
    )
    .await?
    {
        DeviceCommandResponse::Project { response } => Ok(response),
        response => unexpected_response(response, "Project command"),
    }
}

pub async fn deliver_collaboration_device_command_via_session(
    session_manager: &rua_device_network::DeviceSessionManager,
    command: &CollaborationDeviceCommand,
    timeout: Duration,
) -> Result<CollaborationDeviceResponse, DeviceCommandDeliveryError> {
    match deliver_envelope_via_session(
        session_manager,
        &command.target_device_id,
        DeviceCommandEnvelope::Collaboration(command.clone()),
        timeout,
    )
    .await?
    {
        DeviceCommandResponse::Collaboration { response } => Ok(response),
        DeviceCommandResponse::Rejected { reason, detail } => {
            Err(DeviceCommandDeliveryError::Rejected { reason, detail })
        }
        response => unexpected_response(response, "Collaboration command"),
    }
}

async fn deliver_terminal_device_command_inner_via_session(
    session_manager: &rua_device_network::DeviceSessionManager,
    command: &TerminalDeviceCommand,
    timeout: Duration,
) -> Result<TerminalDeviceResponse, DeviceCommandDeliveryError> {
    match deliver_envelope_via_session(
        session_manager,
        &command.target_device_id,
        DeviceCommandEnvelope::Terminal(command.clone()),
        timeout,
    )
    .await?
    {
        DeviceCommandResponse::Terminal { response } => Ok(response),
        response => unexpected_response(response, "Terminal command"),
    }
}

async fn deliver_envelope_via_session(
    session_manager: &rua_device_network::DeviceSessionManager,
    target_device_id: &str,
    envelope: DeviceCommandEnvelope,
    timeout: Duration,
) -> Result<DeviceCommandResponse, DeviceCommandDeliveryError> {
    let payload = serde_json::to_vec(&envelope)?;
    let response_bytes = session_manager
        .command_to(target_device_id, &payload, timeout)
        .await?;
    let response: DeviceCommandResponse = serde_json::from_slice(&response_bytes)?;
    match response {
        DeviceCommandResponse::AuthorityRedirect { redirect } => Err(
            DeviceCommandDeliveryError::AuthorityRedirect(Box::new(redirect)),
        ),
        DeviceCommandResponse::Rejected { reason, detail } => {
            Err(DeviceCommandDeliveryError::Rejected { reason, detail })
        }
        response => Ok(response),
    }
}

fn unexpected_response<T>(
    response: DeviceCommandResponse,
    operation: &str,
) -> Result<T, DeviceCommandDeliveryError> {
    Err(DeviceCommandDeliveryError::Rejected {
        reason: "unexpected_response".to_string(),
        detail: format!(
            "{} response returned for {operation}",
            device_command_response_kind(&response)
        ),
    })
}

fn device_command_response_kind(response: &DeviceCommandResponse) -> &'static str {
    match response {
        DeviceCommandResponse::Accepted(_) => "Conversation Input",
        DeviceCommandResponse::AcpChat { .. } => "ACP Chat",
        DeviceCommandResponse::RemoteChatControl { .. } => "Remote Chat Control",
        DeviceCommandResponse::ConversationControl { .. } => "Conversation Control",
        DeviceCommandResponse::ConversationManifest { .. } => "Conversation Manifest",
        DeviceCommandResponse::ConversationAuthorityAdopt { .. } => "Conversation Authority Adopt",
        DeviceCommandResponse::ConversationAuthoritySuperseded { .. } => {
            "Conversation Authority Superseded"
        }
        DeviceCommandResponse::AuthorityRedirect { .. } => "Conversation Authority Redirect",
        DeviceCommandResponse::Terminal { .. } => "Terminal",
        DeviceCommandResponse::SyncTransfer { .. } => "Sync Transfer",
        DeviceCommandResponse::ChatMediaPrepared { .. } => "Chat Media Prepared",
        DeviceCommandResponse::Project { .. } => "Project",
        DeviceCommandResponse::Collaboration { .. } => "Collaboration",
        DeviceCommandResponse::Rejected { .. } => "Rejected",
    }
}

async fn deliver_terminal_device_command_reassembled_via_session(
    session_manager: &rua_device_network::DeviceSessionManager,
    command: &TerminalDeviceCommand,
    timeout: Duration,
) -> Result<TerminalDeviceResponse, DeviceCommandDeliveryError> {
    let response =
        deliver_terminal_device_command_inner_via_session(session_manager, command, timeout)
            .await?;
    reassemble_terminal_device_response(command, response, |chunk_command| async move {
        deliver_terminal_device_command_inner_via_session(session_manager, &chunk_command, timeout)
            .await
    })
    .await
}

async fn reassemble_terminal_device_response<F, Fut>(
    attach_command: &TerminalDeviceCommand,
    response: TerminalDeviceResponse,
    mut deliver_chunk: F,
) -> Result<TerminalDeviceResponse, DeviceCommandDeliveryError>
where
    F: FnMut(TerminalDeviceCommand) -> Fut,
    Fut: std::future::Future<Output = Result<TerminalDeviceResponse, DeviceCommandDeliveryError>>,
{
    let TerminalDeviceResponse::AttachedTransfer {
        session,
        snapshot_cursor,
        live_cursor,
        transfer_id,
        total_bytes,
    } = response
    else {
        return match response {
            TerminalDeviceResponse::AttachmentChunk { .. } => Err(invalid_terminal_transfer(
                "attachment chunk returned without an attachment transfer",
            )),
            response => Ok(response),
        };
    };

    let mut payload = Vec::with_capacity(total_bytes);
    let mut offset_bytes = 0;
    while offset_bytes < total_bytes {
        let response = deliver_terminal_attachment_chunk_with_retry(
            attach_command,
            &transfer_id,
            offset_bytes,
            &mut deliver_chunk,
        )
        .await?;
        let TerminalDeviceResponse::AttachmentChunk {
            transfer_id: response_transfer_id,
            offset_bytes: response_offset,
            next_offset_bytes,
            total_bytes: response_total,
            data_base64,
            done,
        } = response
        else {
            return Err(invalid_terminal_transfer(
                "non-chunk response returned for attachment transfer",
            ));
        };
        if response_transfer_id != transfer_id
            || response_offset != offset_bytes
            || response_total != total_bytes
            || next_offset_bytes <= offset_bytes
            || next_offset_bytes > total_bytes
            || done != (next_offset_bytes == total_bytes)
        {
            return Err(invalid_terminal_transfer(format!(
                "invalid attachment chunk metadata for transfer {transfer_id} at offset {offset_bytes}"
            )));
        }
        let data = BASE64_STANDARD.decode(data_base64).map_err(|error| {
            invalid_terminal_transfer(format!(
                "invalid attachment chunk encoding for transfer {transfer_id} at offset {offset_bytes}: {error}"
            ))
        })?;
        if data.len() != next_offset_bytes.saturating_sub(offset_bytes) {
            return Err(invalid_terminal_transfer(format!(
                "invalid attachment chunk length for transfer {transfer_id} at offset {offset_bytes}"
            )));
        }
        payload.extend_from_slice(&data);
        offset_bytes = next_offset_bytes;
    }
    if payload.len() != total_bytes {
        return Err(invalid_terminal_transfer(format!(
            "attachment transfer {transfer_id} ended at {} of {total_bytes} bytes",
            payload.len()
        )));
    }
    let payload: TerminalAttachmentTransferPayload =
        serde_json::from_slice(&payload).map_err(|error| {
            invalid_terminal_transfer(format!(
                "failed to decode attachment transfer {transfer_id}: {error}"
            ))
        })?;
    tracing::debug!(
        event = "terminal_attachment_transfer_complete",
        transfer_id,
        session_id = %session.session_id,
        snapshot_activity_sequence = snapshot_cursor.next_activity_sequence,
        snapshot_output_offset = snapshot_cursor.next_output_offset,
        live_activity_sequence = live_cursor.next_activity_sequence,
        live_output_offset = live_cursor.next_output_offset,
        total_bytes,
    );
    Ok(TerminalDeviceResponse::Attached {
        session,
        snapshot: payload.snapshot,
        tail: payload.tail,
        live_cursor,
    })
}

async fn deliver_terminal_attachment_chunk_with_retry<F, Fut>(
    attach_command: &TerminalDeviceCommand,
    transfer_id: &str,
    offset_bytes: usize,
    deliver: &mut F,
) -> Result<TerminalDeviceResponse, DeviceCommandDeliveryError>
where
    F: FnMut(TerminalDeviceCommand) -> Fut,
    Fut: std::future::Future<Output = Result<TerminalDeviceResponse, DeviceCommandDeliveryError>>,
{
    const MAX_ATTEMPTS: usize = 3;

    let command = TerminalDeviceCommand {
        command_id: format!("terminal-command-{}", uuid::Uuid::new_v4()),
        source_device_id: attach_command.source_device_id.clone(),
        target_device_id: attach_command.target_device_id.clone(),
        action: TerminalDeviceAction::AttachmentChunk {
            transfer_id: transfer_id.to_string(),
            offset_bytes,
        },
    };
    let mut last_error = None;
    for _ in 0..MAX_ATTEMPTS {
        match deliver(command.clone()).await {
            Ok(response) => return Ok(response),
            Err(error @ DeviceCommandDeliveryError::Rejected { .. }) => return Err(error),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        invalid_terminal_transfer(format!(
            "attachment transfer {transfer_id} failed at offset {offset_bytes}"
        ))
    }))
}

fn invalid_terminal_transfer(detail: impl Into<String>) -> DeviceCommandDeliveryError {
    DeviceCommandDeliveryError::Rejected {
        reason: "invalid_terminal_attachment_transfer".to_string(),
        detail: detail.into(),
    }
}

async fn build_acp_device_status(
    handle: &crate::services::acp_chat_registry::AcpChatRuntimeHandle,
    chat_id: &str,
    target_device_id: &str,
) -> AcpChatDeviceStatus {
    let workspace_dir = expand_tilde(&handle.workspace_dir.to_string_lossy());
    match handle.manager.get(chat_id).await {
        Some(session) => AcpChatDeviceStatus {
            active: true,
            runner: Some(session.runner),
            session_id: session.runner_session_id,
            cwd: Some(expand_tilde(&session.cwd.to_string_lossy())),
            chat_id: chat_id.to_string(),
            workspace_dir,
            target_device_id: target_device_id.to_string(),
        },
        None => AcpChatDeviceStatus {
            active: false,
            runner: None,
            session_id: None,
            cwd: None,
            chat_id: chat_id.to_string(),
            workspace_dir,
            target_device_id: target_device_id.to_string(),
        },
    }
}

fn expand_tilde(path: &str) -> String {
    let home = std::env::var("HOME")
        .ok()
        .or_else(|| std::env::var("USERPROFILE").ok());
    let Some(home) = home else {
        return path.to_string();
    };
    if path == "~" {
        return home;
    }
    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        return format!("{home}/{rest}");
    }
    path.to_string()
}

fn system_time_to_epoch_ms(value: SystemTime) -> Option<u64> {
    value
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis() as u64)
}

fn validate_id(value: &str) -> Result<(), DeviceCommandBusError> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(DeviceCommandBusError::InvalidId(value.to_string()));
    }
    Ok(())
}

fn validate_media_filename(value: &str) -> Result<(), DeviceCommandBusError> {
    if value.is_empty()
        || value.contains('/')
        || value.contains('\\')
        || Path::new(value).components().count() != 1
        || value == "."
        || value == ".."
        || value.contains("..")
    {
        return Err(DeviceCommandBusError::InvalidId(value.to_string()));
    }
    Ok(())
}
