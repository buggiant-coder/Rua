use std::path::Path;

use rua_session::WorkspaceSessionStore;

use crate::errors::ApiError;
use crate::state::AppState;

/// Configuration belongs to the saved Chat, never to its URL or the current selector.
#[derive(Debug, Clone)]
pub struct ChatContext {
    pub agent_id: String,
    pub project_id: Option<String>,
}

pub async fn load_chat_context(
    rua_root: &Path,
    chat_id: &str,
) -> Result<Option<ChatContext>, ApiError> {
    crate::services::console_chat::resolve_chat_id(chat_id).map_err(ApiError::BadRequest)?;
    let header =
        WorkspaceSessionStore::new(rua_core::RuaRoot::new(rua_root).chat_workspace().as_path())
            .load_chat_header(chat_id)
            .await
            .map_err(ApiError::from)?;
    Ok(header.map(|header| ChatContext {
        agent_id: header.agent_id,
        project_id: header.project_id,
    }))
}

pub async fn resolve_submission_context(
    state: &AppState,
    chat_id: &str,
    draft_agent_id: Option<&str>,
    draft_project_id: Option<&str>,
) -> Result<ChatContext, ApiError> {
    let context = if let Some(context) = load_chat_context(&state.rua_root, chat_id).await? {
        context
    } else {
        let agent_id = draft_agent_id
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| ApiError::BadRequest("agent_id is required for a new Chat".into()))?;
        if state
            .agent_service
            .get_agent(agent_id)
            .await
            .map_err(ApiError::from)?
            .is_none()
        {
            return Err(ApiError::NotFound(format!("Agent not found: {agent_id}")));
        }
        ChatContext {
            agent_id: agent_id.to_string(),
            project_id: draft_project_id
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_string),
        }
    };
    if let Some(project_id) = context.project_id.as_deref() {
        let project_id = rua_core::ProjectId::new(project_id);
        if state
            .project_catalog
            .lifecycle_transition_in_progress(&project_id)
        {
            return Err(ApiError::Conflict(
                "Project lifecycle transition is in progress".into(),
            ));
        }
        let project = state
            .project_catalog
            .get(&project_id)
            .map_err(|error| ApiError::Internal(error.to_string()))?
            .ok_or_else(|| ApiError::NotFound(format!("Project not found: {project_id}")))?;
        if project.lifecycle != rua_project::ProjectLifecycle::Active {
            return Err(ApiError::Conflict(
                "Archived Project cannot accept input".into(),
            ));
        }
    }
    Ok(context)
}
