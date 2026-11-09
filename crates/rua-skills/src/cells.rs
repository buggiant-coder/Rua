use std::path::PathBuf;
use std::sync::Arc;

use cell_store::{CellStore, CellStoreError, FileSystemCellStore};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;

use crate::error::SkillError;
use crate::manifest::WorkspaceManifest;

const WORKSPACE_SKILL_ROOT: &str = "$";
const WORKSPACE_SKILLS_SECTION: &str = "$.skills";

#[derive(Clone)]
pub struct WorkspaceSkillCells {
    store: Arc<FileSystemCellStore>,
    workspaces_dir: PathBuf,
}

impl WorkspaceSkillCells {
    pub fn new(store: Arc<FileSystemCellStore>, workspaces_dir: PathBuf) -> Self {
        Self {
            store,
            workspaces_dir,
        }
    }

    pub fn cell<'a>(&'a self, agent_id: &str) -> WorkspaceSkillCell<'a> {
        WorkspaceSkillCell {
            cells: self,
            agent_id: agent_id.to_string(),
        }
    }

    fn skill_json_path(&self, agent_id: &str) -> PathBuf {
        self.workspaces_dir.join(agent_id).join("skill.json")
    }
}

pub struct WorkspaceSkillCell<'a> {
    cells: &'a WorkspaceSkillCells,
    agent_id: String,
}

impl<'a> WorkspaceSkillCell<'a> {
    pub async fn read(&self) -> Result<Option<WorkspaceManifest>, SkillError> {
        self.cells
            .store
            .read::<WorkspaceManifest>(
                &self.cells.skill_json_path(&self.agent_id),
                WORKSPACE_SKILL_ROOT,
            )
            .await
            .map_err(cell_err_to_skill)
    }

    pub async fn read_value(&self) -> Result<Option<Value>, SkillError> {
        self.cells
            .store
            .read::<Value>(
                &self.cells.skill_json_path(&self.agent_id),
                WORKSPACE_SKILL_ROOT,
            )
            .await
            .map_err(cell_err_to_skill)
    }

    pub async fn write(&self, manifest: WorkspaceManifest) -> Result<(), SkillError> {
        self.cells
            .store
            .write(
                &self.cells.skill_json_path(&self.agent_id),
                WORKSPACE_SKILL_ROOT,
                manifest,
            )
            .await
            .map_err(cell_err_to_skill)
    }

    pub async fn write_value(&self, value: Value) -> Result<(), SkillError> {
        self.cells
            .store
            .write(
                &self.cells.skill_json_path(&self.agent_id),
                WORKSPACE_SKILL_ROOT,
                value,
            )
            .await
            .map_err(cell_err_to_skill)
    }

    pub async fn update<F>(&self, mutator: F) -> Result<WorkspaceManifest, SkillError>
    where
        F: FnOnce(Option<WorkspaceManifest>) -> WorkspaceManifest + Send,
    {
        self.cells
            .store
            .update::<WorkspaceManifest, _>(
                &self.cells.skill_json_path(&self.agent_id),
                WORKSPACE_SKILL_ROOT,
                mutator,
            )
            .await
            .map_err(cell_err_to_skill)
    }

    pub async fn update_value<F>(&self, mutator: F) -> Result<Value, SkillError>
    where
        F: FnOnce(Option<Value>) -> Value + Send,
    {
        self.cells
            .store
            .update::<Value, _>(
                &self.cells.skill_json_path(&self.agent_id),
                WORKSPACE_SKILL_ROOT,
                mutator,
            )
            .await
            .map_err(cell_err_to_skill)
    }

    pub async fn update_skills<V, F>(&self, mutator: F) -> Result<V, SkillError>
    where
        V: Serialize + DeserializeOwned + Send + 'static,
        F: FnOnce(Option<V>) -> V + Send,
    {
        self.cells
            .store
            .update::<V, _>(
                &self.cells.skill_json_path(&self.agent_id),
                WORKSPACE_SKILLS_SECTION,
                mutator,
            )
            .await
            .map_err(cell_err_to_skill)
    }
}

fn cell_err_to_skill(err: CellStoreError) -> SkillError {
    SkillError::Consistency(format!("workspace skill cell store error: {err}"))
}
