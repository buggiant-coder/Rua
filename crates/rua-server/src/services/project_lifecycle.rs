use crate::services::conversation_authority::ConversationAuthority;
use crate::sync_publication::{
    publish_sync_fact_with_runtime, SyncPublicationFact, SyncPublicationRuntime,
};
use rua_core::{ChatAddress, ChatScopeStore, ProjectId};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

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

/// Owns resource-device edits from admission exclusion through committed publication.
#[derive(Clone)]
pub struct ProjectResourceEditor {
    data_dir: PathBuf,
    catalog: Arc<rua_project::ProjectCatalog>,
    bindings: Arc<rua_project::ProjectResourceBindingStore>,
    authority: Arc<ConversationAuthority>,
    approvals: Arc<rua_security::ApprovalAuthority>,
    tasks: Arc<rua_task::TaskRuntime>,
    publication: SyncPublicationRuntime,
}

impl ProjectResourceEditor {
    pub fn new(
        data_dir: PathBuf,
        catalog: Arc<rua_project::ProjectCatalog>,
        bindings: Arc<rua_project::ProjectResourceBindingStore>,
        authority: Arc<ConversationAuthority>,
        approvals: Arc<rua_security::ApprovalAuthority>,
        tasks: Arc<rua_task::TaskRuntime>,
        publication: SyncPublicationRuntime,
    ) -> Self {
        Self {
            data_dir,
            catalog,
            bindings,
            authority,
            approvals,
            tasks,
            publication,
        }
    }

    pub fn from_app_state(state: &crate::state::AppState) -> Self {
        Self::new(
            state.rua_root.clone(),
            state.project_catalog.clone(),
            state.project_bindings.clone(),
            state.conversation_authority.clone(),
            state.security.approval_authority().clone(),
            state.task_runtime.clone(),
            SyncPublicationRuntime::from_app_state(state),
        )
    }

    pub async fn edit_resources(
        &self,
        project_id: &ProjectId,
        expected_revision: u64,
        paths: &[PathBuf],
        primary_index: usize,
        directory_ids: &[Option<String>],
    ) -> Result<rua_project::ProjectCreationOutcome, String> {
        let guard = self
            .catalog
            .begin_resource_edit(project_id)
            .map_err(|e| e.to_string())?;
        let chats = self.editable_chats(project_id, expected_revision).await?;
        let binding = self
            .bindings
            .get(project_id)
            .map_err(|e| e.to_string())?
            .ok_or("Project binding missing")?;
        let candidate = binding
            .resources
            .edited_with_directory_ids(paths, primary_index, directory_ids)
            .map_err(|e| e.to_string())?;
        let mut referenced = Vec::new();
        for (address, header) in &chats {
            let id = header
                .extra
                .get(crate::services::project_chat::PROJECT_WORKING_DIRECTORY_META_KEY)
                .and_then(serde_json::Value::as_str)
                .unwrap_or(rua_project::INITIAL_PROJECT_DIRECTORY_ID);
            if candidate.directory(id).is_err() {
                referenced.push(address.chat_id().to_string());
            }
        }
        if !referenced.is_empty() {
            return Err(format!(
                "Cannot remove directories used by Chats (including archived): {}",
                referenced.join(", ")
            ));
        }
        let outcome = guard
            .commit_resources(expected_revision, candidate)
            .map_err(|e| e.to_string())?;
        self.publish(outcome.project.clone()).await;
        Ok(outcome)
    }

    pub async fn set_primary_directory(
        &self,
        project_id: &ProjectId,
        expected_revision: u64,
        directory_id: &str,
    ) -> Result<rua_project::ProjectDescriptor, String> {
        let guard = self
            .catalog
            .begin_resource_edit(project_id)
            .map_err(|e| e.to_string())?;
        self.editable_chats(project_id, expected_revision).await?;
        let project = guard
            .set_primary_directory(expected_revision, directory_id)
            .map_err(|e| e.to_string())?;
        self.publish(project.clone()).await;
        Ok(project)
    }

    async fn editable_chats(
        &self,
        project_id: &ProjectId,
        expected_revision: u64,
    ) -> Result<Vec<(ChatAddress, rua_session::ChatSessionHeader)>, String> {
        let (project, _, chats) =
            load_project_chats(&self.data_dir, &self.catalog, &self.bindings, project_id).await?;
        if project.lifecycle != rua_project::ProjectLifecycle::Active {
            return Err("Archived Project cannot change resources".into());
        }
        let binding = self
            .bindings
            .get(project_id)
            .map_err(|e| e.to_string())?
            .ok_or("Project binding missing")?;
        if binding.resources.revision != expected_revision {
            return Err("Project resources changed; reload before editing".into());
        }
        self.ensure_project_resources_idle(&chats).await?;
        Ok(chats)
    }

    async fn publish(&self, descriptor: rua_project::ProjectDescriptor) {
        let _ = publish_sync_fact_with_runtime(
            &self.publication,
            SyncPublicationFact::ProjectDescriptorCommitted { descriptor },
        )
        .await;
    }
    async fn ensure_project_resources_idle(
        &self,
        chats: &[(ChatAddress, rua_session::ChatSessionHeader)],
    ) -> Result<(), String> {
        for chat in chats {
            let work =
                inspect_chat_work(&self.authority, &self.approvals, Some(&self.tasks), &chat.0)
                    .await?;
            if work.is_busy() {
                return Err(format!(
                    "Project Chat {} blocks resource changes (running turns: {}, queued inputs: {}, pending approvals: {}, pending prompts: {}, background tasks: {})",
                    chat.0.chat_id(), usize::from(work.active_turn), work.queued_inputs,
                    work.pending_approvals, usize::from(work.pending_prompt), work.background_tasks,
                ));
            }
        }
        Ok(())
    }
}

pub(crate) async fn load_project_chats(
    data_dir: &Path,
    catalog: &rua_project::ProjectCatalog,
    bindings: &rua_project::ProjectResourceBindingStore,
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
    let binding = bindings
        .get(project_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("Project Resource Binding not found: {project_id}"))?;
    if binding.resource_device_id != project.resource_device_id {
        return Err("Project Resource Binding Device does not match the Project".to_string());
    }
    let scope_root = ChatScopeStore::resolve(
        data_dir,
        ChatAddress::project(project_id.clone(), "00000000-0000-4000-8000-000000000000"),
    )
    .root()
    .to_path_buf();
    let store = rua_session::WorkspaceSessionStore::new(&scope_root);
    let mut chats = Vec::new();
    for chat in crate::services::chat_index::list_all(&scope_root).await? {
        let address = ChatAddress::project(project_id.clone(), chat.id.clone());
        let header = store
            .load_chat_header(address.chat_id().as_str())
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| {
                format!(
                    "Project Chat header not found: {}",
                    address.chat_id().as_str()
                )
            })?;
        chats.push((address, header));
    }
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
