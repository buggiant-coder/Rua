use crate::services::conversation_authority::ConversationAuthority;
use rua_core::{ChatAddress, ProjectId};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectLifecycleImpact {
    pub affected_chat_count: usize,
    pub active_turn_count: usize,
    pub queued_input_count: usize,
    pub pending_approval_count: usize,
    pub pending_prompt_count: usize,
    pub active_acp_count: usize,
    pub background_task_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectLifecyclePreview {
    pub project: rua_project::ProjectDescriptor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impact: Option<ProjectLifecycleImpact>,
    pub resource_device_revoked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectArchiveResult {
    pub project: rua_project::ProjectDescriptor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impact: Option<ProjectLifecycleImpact>,
    pub resource_device_revoked: bool,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectReopenResult {
    pub project: rua_project::ProjectDescriptor,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectPermanentDeleteResult {
    pub project_id: rua_core::ProjectId,
    pub deleted_chat_count: usize,
    pub changed: bool,
}

pub(crate) async fn load_project_chats(
    data_dir: &Path,
    catalog: &rua_project::ProjectCatalog,
    project_id: &ProjectId,
) -> Result<
    (
        rua_project::ProjectDescriptor,
        PathBuf,
        Vec<(ChatAddress, rua_session::ChatSessionHeader)>,
    ),
    String,
> {
    let project = catalog
        .get(project_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("Project not found: {project_id}"))?;
    let scope_root = rua_core::RuaRoot::new(data_dir)
        .chat_workspace()
        .as_path()
        .to_path_buf();
    let store = rua_session::WorkspaceSessionStore::new(&scope_root);
    let chats = store
        .list_chat_headers(0, usize::MAX)
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|header| header.project_id.as_deref() == Some(project_id.as_str()))
        .map(|header| (ChatAddress::new(&header.chat_id), header))
        .collect();
    Ok((project, scope_root, chats))
}

pub(crate) async fn inspect_project_work(
    authority: &ConversationAuthority,
    approvals: &rua_security::ApprovalAuthority,
    task_runtime: Option<&rua_task::TaskRuntime>,
    chats: &[(ChatAddress, rua_session::ChatSessionHeader)],
) -> Result<ProjectLifecycleImpact, String> {
    let mut impact = ProjectLifecycleImpact::default();
    for (address, header) in chats {
        let work = inspect_chat_work(authority, approvals, task_runtime, address).await?;
        // Keep the existing wire field as executor-selection information only.
        let acp = crate::services::conversation_authority_placement::chat_acp_binding_active(
            &serde_json::Value::Object(header.extra.clone()),
        );
        impact.active_turn_count += usize::from(work.active_turn);
        impact.queued_input_count += work.queued_inputs;
        impact.pending_approval_count += work.pending_approvals;
        impact.pending_prompt_count += usize::from(work.pending_prompt);
        impact.active_acp_count += usize::from(acp);
        impact.background_task_count += work.background_tasks;
        impact.affected_chat_count += usize::from(work.is_busy());
    }
    Ok(impact)
}

/// Work that must finish or be stopped; executor selection is not work.
struct ProjectChatWork {
    active_turn: bool,
    queued_inputs: usize,
    pending_approvals: usize,
    pending_prompt: bool,
    background_tasks: usize,
}

impl ProjectChatWork {
    fn is_busy(&self) -> bool {
        self.active_turn
            || self.queued_inputs > 0
            || self.pending_approvals > 0
            || self.pending_prompt
            || self.background_tasks > 0
    }
}

async fn inspect_chat_work(
    authority: &ConversationAuthority,
    approvals: &rua_security::ApprovalAuthority,
    task_runtime: Option<&rua_task::TaskRuntime>,
    address: &ChatAddress,
) -> Result<ProjectChatWork, String> {
    let active_turn = authority.active_turn(address).await.is_some();
    let queued_inputs = authority.queue_snapshot(address).await.len();
    let pending_approvals = approvals.list_pending_for_address(address).await.len();
    let pending_prompt = rua_tool::extended::ask_user::AskUserService::global()
        .get_pending_by_session(address.chat_id().as_str())
        .await
        .is_some();
    let background_tasks = if let Some(runtime) = task_runtime {
        runtime
            .activity_snapshot(&rua_task::TaskOwner::from_address(address.clone()))
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .filter(|task| !task.state.is_terminal())
            .count()
    } else {
        0
    };
    Ok(ProjectChatWork {
        active_turn,
        queued_inputs,
        pending_approvals,
        pending_prompt,
        background_tasks,
    })
}
