use std::path::Path;

use rua_session::WorkspaceSessionStore;

use crate::errors::ApiError;
use crate::state::AppState;

pub(crate) fn saved_working_directory(
    header: Option<&rua_session::ChatSessionHeader>,
    device_id: &str,
) -> Option<std::path::PathBuf> {
    let directory = header?.extra.get(rua_session::EXECUTION_DIRECTORY_KEY)?;
    if directory.get("device_id")?.as_str()? != device_id {
        return None;
    }
    directory
        .get("path")?
        .as_str()
        .map(std::path::PathBuf::from)
}

pub(crate) async fn working_directory_on_device(
    scope: &Path,
    chat_id: &str,
    device_id: &str,
    default: std::path::PathBuf,
) -> Result<std::path::PathBuf, String> {
    let header = WorkspaceSessionStore::new(scope)
        .load_stored_chat_header(chat_id)
        .await
        .map_err(|error| error.to_string())?;
    Ok(saved_working_directory(header.as_ref(), device_id).unwrap_or(default))
}

/// Configuration belongs to the saved Chat, never to its URL or the current selector.
#[derive(Debug, Clone)]
pub struct ChatContext {
    pub agent_id: String,
    pub project_id: Option<String>,
}

/// An absent field means the caller uses the saved membership. An explicit null asserts no Project.
pub(crate) fn requested_project(
    value: Option<&serde_json::Value>,
) -> Result<Option<Option<&str>>, ApiError> {
    match value {
        None => Ok(None),
        Some(serde_json::Value::Null) => Ok(Some(None)),
        Some(serde_json::Value::String(id)) if !id.trim().is_empty() => Ok(Some(Some(id.trim()))),
        Some(_) => Err(ApiError::BadRequest(
            "project_id must be a Project ID or null".into(),
        )),
    }
}

pub(crate) fn check_project_membership(
    saved: Option<&str>,
    requested: Option<Option<&str>>,
) -> Result<(), ApiError> {
    if requested.is_some_and(|id| id != saved) {
        return Err(ApiError::Conflict(
            "Chat Project membership is fixed".into(),
        ));
    }
    Ok(())
}

impl ChatContext {
    pub(crate) fn validate_project_field(
        &self,
        value: Option<&serde_json::Value>,
    ) -> Result<(), ApiError> {
        check_project_membership(self.project_id.as_deref(), requested_project(value)?)
    }
}

pub async fn load_chat_context(
    rua_root: &Path,
    chat_id: &str,
) -> Result<Option<ChatContext>, ApiError> {
    crate::services::console_chat::resolve_chat_id(chat_id).map_err(ApiError::BadRequest)?;
    Ok(load_live_chat_header(rua_root, chat_id)
        .await?
        .map(ChatContext::from))
}

impl From<rua_session::ChatSessionHeader> for ChatContext {
    fn from(header: rua_session::ChatSessionHeader) -> Self {
        Self {
            agent_id: header.agent_id,
            project_id: header.project_id,
        }
    }
}

pub(crate) async fn load_live_chat_header(
    rua_root: &Path,
    chat_id: &str,
) -> Result<Option<rua_session::ChatSessionHeader>, ApiError> {
    if chat_is_deleted(rua_root, chat_id)? {
        return Ok(None);
    }
    read_chat_header(rua_root, chat_id).await
}

/// Deletion retries may need the saved membership after the terminal intent was recorded.
pub(crate) async fn read_chat_header(
    rua_root: &Path,
    chat_id: &str,
) -> Result<Option<rua_session::ChatSessionHeader>, ApiError> {
    WorkspaceSessionStore::new(rua_core::RuaRoot::new(rua_root).chat_workspace().as_path())
        .load_chat_header(chat_id)
        .await
        .map_err(ApiError::from)
}

pub(crate) fn chat_is_deleted(rua_root: &Path, chat_id: &str) -> Result<bool, ApiError> {
    rua_session::chat_deletion::ChatDeleteIntentStore::from_rua_root(rua_root)
        .load(&rua_core::ChatAddress::new(chat_id))
        .map(|intent| intent.is_some())
        .map_err(ApiError::Internal)
}

pub(crate) fn ensure_chat_not_deleted(rua_root: &Path, chat_id: &str) -> Result<(), ApiError> {
    if chat_is_deleted(rua_root, chat_id)? {
        return Err(ApiError::NotFound(format!("Chat not found: {chat_id}")));
    }
    Ok(())
}

pub(crate) async fn load_indexed_context(
    workspace: &rua_core::WorkspaceRoot,
    chat_id: &str,
) -> Result<Option<ChatContext>, ApiError> {
    if !workspace
        .chat_index_path()
        .try_exists()
        .map_err(|error| ApiError::Internal(error.to_string()))?
    {
        return Ok(None);
    }
    let Some(row) = rua_session::chat_index::get(workspace.as_path(), chat_id)
        .await
        .map_err(ApiError::Internal)?
    else {
        return Ok(None);
    };
    let agent_id = row
        .meta
        .get("agent_id")
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| ApiError::Conflict("Saved Chat configuration has not arrived".into()))?;
    Ok(Some(ChatContext {
        agent_id: agent_id.to_owned(),
        project_id: row
            .meta
            .get("project_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
    }))
}

pub async fn resolve_submission_context(
    state: &AppState,
    chat_id: &str,
    draft_agent_id: Option<&str>,
    requested_project: Option<Option<&str>>,
) -> Result<ChatContext, ApiError> {
    crate::services::console_chat::resolve_chat_id(chat_id).map_err(ApiError::BadRequest)?;
    ensure_chat_not_deleted(&state.rua_root, chat_id)?;
    let saved = match read_chat_header(&state.rua_root, chat_id).await? {
        Some(header) => Some(ChatContext::from(header)),
        None => load_indexed_context(&state.chat_workspace(), chat_id).await?,
    };
    let context = if let Some(context) = saved {
        check_project_membership(context.project_id.as_deref(), requested_project)?;
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
            project_id: requested_project.flatten().map(str::to_string),
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
