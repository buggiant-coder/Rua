use std::path::PathBuf;
use std::sync::Arc;

use cell_store::{CellStore, CellStoreError, FileSystemCellStore};
use rua_config::AgentProfileConfig;
use serde::{de::DeserializeOwned, Serialize};

use crate::error::AgentError;

const AGENT_FILE_ROOT: &str = "$";

#[derive(Clone)]
pub struct AgentCells {
    store: Arc<FileSystemCellStore>,
    workspaces_dir: PathBuf,
}

impl AgentCells {
    pub fn new(store: Arc<FileSystemCellStore>, workspaces_dir: PathBuf) -> Self {
        Self {
            store,
            workspaces_dir,
        }
    }

    pub fn workspaces_dir(&self) -> &std::path::Path {
        &self.workspaces_dir
    }

    pub fn full<'a>(&'a self, agent_id: &str) -> AgentCell<'a> {
        AgentCell {
            cells: self,
            agent_id: agent_id.to_string(),
        }
    }

    pub fn section<'a, V>(&'a self, agent_id: &str, section_name: &str) -> AgentSectionCell<'a, V>
    where
        V: Serialize + DeserializeOwned + Send + 'static,
    {
        AgentSectionCell {
            cells: self,
            agent_id: agent_id.to_string(),
            section_name: section_name.to_string(),
            _marker: std::marker::PhantomData,
        }
    }

    pub async fn list(&self) -> Result<Vec<String>, AgentError> {
        let mut entries = match tokio::fs::read_dir(&self.workspaces_dir).await {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => {
                return Err(AgentError::config(format!(
                    "failed to read workspaces dir {}: {e}",
                    self.workspaces_dir.display()
                )));
            }
        };
        let mut ids = Vec::new();
        loop {
            let entry = entries.next_entry().await.map_err(|e| {
                AgentError::config(format!(
                    "failed to iterate workspaces dir {}: {e}",
                    self.workspaces_dir.display()
                ))
            })?;
            let Some(entry) = entry else {
                break;
            };
            let file_type = match entry.file_type().await {
                Ok(t) => t,
                Err(_) => continue,
            };
            if !file_type.is_dir() {
                continue;
            }
            let agent_id = match entry.file_name().to_str() {
                Some(n) => n.to_string(),
                None => continue,
            };
            if !entry.path().join("agent.json").exists() {
                continue;
            }
            ids.push(agent_id);
        }
        ids.sort();
        Ok(ids)
    }

    fn agent_json_path(&self, agent_id: &str) -> PathBuf {
        self.workspaces_dir.join(agent_id).join("agent.json")
    }

    fn store(&self) -> &FileSystemCellStore {
        &self.store
    }
}

pub struct AgentCell<'a> {
    cells: &'a AgentCells,
    agent_id: String,
}

impl<'a> AgentCell<'a> {
    pub async fn read(&self) -> Result<Option<AgentProfileConfig>, AgentError> {
        self.cells
            .store()
            .read::<AgentProfileConfig>(
                &self.cells.agent_json_path(&self.agent_id),
                AGENT_FILE_ROOT,
            )
            .await
            .map_err(cell_err_to_agent)
    }

    pub async fn write(&self, config: AgentProfileConfig) -> Result<(), AgentError> {
        self.cells
            .store()
            .write(
                &self.cells.agent_json_path(&self.agent_id),
                AGENT_FILE_ROOT,
                config,
            )
            .await
            .map_err(cell_err_to_agent)
    }

    pub async fn update<F>(&self, mutator: F) -> Result<AgentProfileConfig, AgentError>
    where
        F: FnOnce(Option<AgentProfileConfig>) -> AgentProfileConfig + Send,
    {
        self.cells
            .store()
            .update::<AgentProfileConfig, _>(
                &self.cells.agent_json_path(&self.agent_id),
                AGENT_FILE_ROOT,
                mutator,
            )
            .await
            .map_err(cell_err_to_agent)
    }

    pub async fn delete(&self) -> Result<(), AgentError> {
        self.cells
            .store()
            .delete(&self.cells.agent_json_path(&self.agent_id), AGENT_FILE_ROOT)
            .await
            .map_err(cell_err_to_agent)
    }

    pub async fn read_value(&self) -> Result<Option<serde_json::Value>, AgentError> {
        self.cells
            .store()
            .read::<serde_json::Value>(&self.cells.agent_json_path(&self.agent_id), AGENT_FILE_ROOT)
            .await
            .map_err(cell_err_to_agent)
    }
}

pub struct AgentSectionCell<'a, V> {
    cells: &'a AgentCells,
    agent_id: String,
    section_name: String,
    _marker: std::marker::PhantomData<V>,
}

impl<'a, V> AgentSectionCell<'a, V>
where
    V: Serialize + DeserializeOwned + Send + 'static,
{
    pub async fn read(&self) -> Result<Option<V>, AgentError> {
        let path = validate_section_path(&self.section_name)?;
        self.cells
            .store()
            .read::<V>(&self.cells.agent_json_path(&self.agent_id), &path)
            .await
            .map_err(cell_err_to_agent)
    }

    pub async fn write(&self, value: V) -> Result<(), AgentError> {
        let path = validate_section_path(&self.section_name)?;
        self.cells
            .store()
            .write(&self.cells.agent_json_path(&self.agent_id), &path, value)
            .await
            .map_err(cell_err_to_agent)
    }

    pub async fn update<F>(&self, mutator: F) -> Result<V, AgentError>
    where
        F: FnOnce(Option<V>) -> V + Send,
    {
        let path = validate_section_path(&self.section_name)?;
        self.cells
            .store()
            .update::<V, _>(&self.cells.agent_json_path(&self.agent_id), &path, mutator)
            .await
            .map_err(cell_err_to_agent)
    }

    pub async fn delete(&self) -> Result<(), AgentError> {
        let path = validate_section_path(&self.section_name)?;
        self.cells
            .store()
            .delete(&self.cells.agent_json_path(&self.agent_id), &path)
            .await
            .map_err(cell_err_to_agent)
    }
}

fn validate_section_name(name: &str) -> Result<(), AgentError> {
    if name.is_empty() {
        return Err(AgentError::config("section name must not be empty"));
    }
    for c in name.chars() {
        if !(c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            return Err(AgentError::config(format!(
                "section name {name:?} contains invalid character {c:?} \
                 (allowed: ASCII alphanumeric, '_', '-')",
            )));
        }
    }
    Ok(())
}

fn validate_section_path(name: &str) -> Result<String, AgentError> {
    validate_section_name(name)?;
    Ok(format!("$.{name}"))
}

fn cell_err_to_agent(err: CellStoreError) -> AgentError {
    AgentError::config(format!("agent cell store error: {err}"))
}
