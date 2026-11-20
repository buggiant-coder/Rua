use std::path::{Path, PathBuf};

use crate::{ChatAddress, RuaRoot, WorkspaceRoot};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatStore {
    address: ChatAddress,
    workspace: WorkspaceRoot,
}

impl ChatStore {
    pub fn resolve(rua_root: &Path, address: ChatAddress) -> Self {
        Self::at_workspace(RuaRoot::new(rua_root).chat_workspace(), address)
    }

    /// Adapts an already resolved Chat workspace, including the isolated
    /// workspace used by atomic materialization. Agent workspaces contain
    /// prompts and configuration and must not be passed here.
    pub fn at_workspace(workspace: WorkspaceRoot, address: ChatAddress) -> Self {
        Self { address, workspace }
    }

    pub fn address(&self) -> &ChatAddress {
        &self.address
    }

    pub fn root(&self) -> &Path {
        self.workspace.as_path()
    }

    pub fn chat_index_path(&self) -> PathBuf {
        self.workspace.chat_index_path()
    }

    pub fn evergreen_chat_root(&self) -> PathBuf {
        self.workspace
            .sessions_dir()
            .join(self.address.chat_id().as_str())
    }
}
