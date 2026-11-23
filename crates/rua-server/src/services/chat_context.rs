use std::path::Path;

use rua_session::WorkspaceSessionStore;

use crate::errors::ApiError;
use crate::state::AppState;

pub(crate) fn saved_directory(
    header: &rua_session::ChatSessionHeader,
    device_id: &str,
) -> Result<Option<rua_session::ChatExecutionDirectory>, String> {
    let Some(value) = header.extra.get(rua_session::EXECUTION_DIRECTORY_KEY) else {
        return Ok(None);
    };
    let directory: rua_session::ChatExecutionDirectory = serde_json::from_value(value.clone())
        .map_err(|error| format!("Invalid saved Chat execution directory: {error}"))?;
    if directory.device_id != device_id {
        return Err("Chat execution directory belongs to another Device".into());
    }
    Ok(Some(directory))
}

pub(crate) fn saved_working_directory(
    header: Option<&rua_session::ChatSessionHeader>,
    device_id: &str,
) -> Result<Option<std::path::PathBuf>, String> {
    let Some(header) = header else {
        return Ok(None);
    };
    let Some(directory) = saved_directory(header, device_id)? else {
        return Ok(None);
    };
    let path = directory.host_path().map_err(|error| error.to_string())?;
    if !path.is_dir() {
        return Err(format!(
            "Chat execution directory is unavailable: {}",
            directory.path.display()
        ));
    }
    Ok(Some(path))
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
        .map_err(|e| e.to_string())?;
    Ok(saved_working_directory(header.as_ref(), device_id)?.unwrap_or(default))
}

/// The name of a managed directory depends only on the Chat and Device.
pub(crate) async fn select_execution_directory(
    rua_root: &Path,
    address: &rua_core::ChatAddress,
    device_id: &str,
    path: Option<&Path>,
) -> Result<rua_session::ChatExecutionDirectory, String> {
    let managed = path.is_none();
    let path = match path {
        Some(path) => super::execution_directory::validate(path, rua_root).await?,
        None => {
            let host_path = rua_core::RuaRoot::new(rua_root)
                .managed_chat_directory(address.chat_id().as_str(), device_id)
                .map_err(|error| error.to_string())?;
            let layout = rua_core::runtime_layout::RuntimeLayout::from_current_process(None);
            match layout.linux_compat {
                Some(guest) => guest
                    .map_host_path(&host_path)
                    .ok_or("Managed Chat directory is unavailable in this execution environment")?,
                None => host_path,
            }
        }
    };
    Ok(rua_session::ChatExecutionDirectory {
        extra: Default::default(),
        device_id: device_id.into(),
        managed,
        path,
    })
}

/// Creates only the Chat's own directory at admission. The returned path is
/// newly created and can be removed if the enclosing durable commit fails.
pub(crate) async fn create_managed_directory(
    directory: &rua_session::ChatExecutionDirectory,
) -> Result<Option<std::path::PathBuf>, String> {
    if !directory.managed {
        return Ok(None);
    }
    let path = directory.host_path().map_err(|e| e.to_string())?;
    let parent = path
        .parent()
        .ok_or("Managed Chat directory has no parent")?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|e| e.to_string())?;
    match tokio::fs::create_dir(&path).await {
        Ok(()) => Ok(Some(path)),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => {
            Ok(None)
        }
        Err(error) => Err(format!(
            "Create managed Chat directory {}: {error}",
            path.display()
        )),
    }
}

pub(crate) async fn initial_execution_directory(
    rua_root: &Path,
    address: &rua_core::ChatAddress,
    device_id: &str,
    extra: &serde_json::Map<String, serde_json::Value>,
) -> Result<rua_session::ChatExecutionDirectory, String> {
    let request = extra
        .get(rua_session::EXECUTION_DIRECTORY_KEY)
        .cloned()
        .map(serde_json::from_value::<rua_core::ExecutionDirectorySelection>)
        .transpose()
        .map_err(|e| format!("Invalid execution directory: {e}"))?;
    if request.as_ref().is_some_and(|r| r.device_id != device_id) {
        return Err("Execution directory belongs to another Device".into());
    }
    select_execution_directory(
        rua_root,
        address,
        device_id,
        request.as_ref().and_then(|r| r.path.as_deref()),
    )
    .await
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

/// Resolve a Chat's saved Project membership on this Device.
///
/// `Ok(None)` means this Device knows about no such Chat. The inner `Option` is the
/// fixed Project membership: `None` for an ordinary Chat.
///
/// A controlling Device that holds only the synchronized Chat Index entry (and no
/// local Chat Header) must still be able to address a Project Chat, so the Index is
/// consulted as the synchronized discovery surface before reporting the Chat absent.
pub(crate) async fn saved_project_membership(
    rua_root: &Path,
    chat_id: &str,
) -> Result<Option<Option<String>>, ApiError> {
    if let Some(header) = load_live_chat_header(rua_root, chat_id).await? {
        return Ok(Some(header.project_id));
    }
    if chat_is_deleted(rua_root, chat_id)? {
        return Ok(None);
    }
    let entry = crate::services::chat_index::get(
        rua_core::RuaRoot::new(rua_root).chat_workspace().as_path(),
        chat_id,
    )
    .await
    .map_err(ApiError::Internal)?;
    Ok(entry.map(|entry| {
        entry
            .meta
            .get("project_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    }))
}

pub async fn load_chat_context(
    rua_root: &Path,
    chat_id: &str,
) -> Result<Option<ChatContext>, ApiError> {
    crate::services::console_chat::resolve_chat_id(chat_id).map_err(ApiError::BadRequest)?;
    if let Some(context) = load_live_chat_header(rua_root, chat_id)
        .await?
        .map(ChatContext::from)
    {
        return Ok(Some(context));
    }
    if chat_is_deleted(rua_root, chat_id)? {
        return Ok(None);
    }
    // A controlling Device holds only the synchronized Chat Index entry for a Chat
    // that lives on another Device. Resolving the same saved configuration from that
    // projection is what lets canonical Chat routes address a remote Chat at all.
    let workspace = rua_core::RuaRoot::new(rua_root).chat_workspace();
    load_indexed_context(&workspace, chat_id).await
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
