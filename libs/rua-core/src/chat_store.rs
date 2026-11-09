use std::path::{Path, PathBuf};

use crate::{ChatAddress, ChatScope, Result, RuaError, RuaRoot};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatScopeStore {
    address: ChatAddress,
    root: PathBuf,
}

impl ChatScopeStore {
    pub fn resolve(rua_root: &Path, address: ChatAddress) -> Self {
        let layout = RuaRoot::new(rua_root);
        let root = match address.scope() {
            ChatScope::Agent { agent_id, .. } => layout
                .agent_workspace(agent_id.as_str())
                .expect("ChatAddress Agent ID must be a valid Workspace owner"),
            ChatScope::Project { project_id, .. } => layout
                .project_workspace(project_id.as_str())
                .expect("ChatAddress Project ID must be a valid Workspace owner"),
        };
        Self {
            address,
            root: root.as_path().to_path_buf(),
        }
    }

    pub fn for_agent_workspace(workspace_dir: &Path, address: ChatAddress) -> Result<Self> {
        if !matches!(address.scope(), ChatScope::Agent { .. }) {
            return Err(RuaError::Validation {
                message: "Agent workspace cannot own a Project-Scoped Chat".to_string(),
            });
        }
        Ok(Self {
            address,
            root: workspace_dir.to_path_buf(),
        })
    }

    pub fn address(&self) -> &ChatAddress {
        &self.address
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn chat_index_path(&self) -> PathBuf {
        self.root.join("chat-index.sqlite")
    }

    pub fn evergreen_chat_root(&self) -> PathBuf {
        self.root
            .join("sessions")
            .join(self.address.chat_id().as_str())
    }
}
