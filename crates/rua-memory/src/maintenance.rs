use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::error::MemoryResult;
use crate::types::{MemoryHealth, ReindexReport, ReindexScope};
use crate::StructuredMemoryStore;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepairPlan {
    pub issues: Vec<RepairIssue>,
    pub estimated_duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepairIssue {
    pub description: String,
    pub severity: IssueSeverity,
    pub fix: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueSeverity {
    Low,
    Medium,
    High,
    Critical,
}

pub struct MemoryMaintenance {
    workspace_path: PathBuf,
}

impl MemoryMaintenance {
    pub fn new(workspace_path: impl Into<PathBuf>) -> Self {
        Self {
            workspace_path: workspace_path.into(),
        }
    }

    pub async fn health_check(&self) -> MemoryResult<MemoryHealth> {
        let ws = &self.workspace_path;
        let mut issues = Vec::new();

        let memory_db_path = ws.join("memory.db");
        let (store_ok, document_count) = if memory_db_path.exists() {
            match StructuredMemoryStore::open(&memory_db_path) {
                Ok(store) => (true, store.count()?),
                Err(e) => {
                    issues.push(format!("memory.db is not readable: {e}"));
                    (false, 0)
                }
            }
        } else {
            issues.push("memory.db not found".into());
            (false, 0)
        };

        let index_path = ws.join("memory_vec.db");
        let index_ok = index_path.exists();
        if !index_ok {
            issues.push("memory_vec.db not found (reindex needed)".into());
        }

        let index_size = if index_ok {
            fs::metadata(&index_path)
                .await
                .map(|m| m.len())
                .unwrap_or(0)
        } else {
            0
        };

        Ok(MemoryHealth {
            index_ok,
            source_files_ok: store_ok,
            document_count,
            index_size_bytes: index_size,
            issues,
        })
    }

    pub async fn diagnose(&self) -> MemoryResult<RepairPlan> {
        let health = self.health_check().await?;
        let mut repair_issues = Vec::new();

        if !health.index_ok {
            repair_issues.push(RepairIssue {
                description: "Memory search index missing or corrupt".into(),
                severity: IssueSeverity::High,
                fix: "Rebuild index from source files".into(),
            });
        }

        if !health.source_files_ok {
            repair_issues.push(RepairIssue {
                description: "Structured Memory store missing or corrupt".into(),
                severity: IssueSeverity::High,
                fix: "Create or repair memory.db".into(),
            });
        }

        Ok(RepairPlan {
            issues: repair_issues,
            estimated_duration_ms: 1000,
        })
    }

    pub async fn repair(&self) -> MemoryResult<Vec<String>> {
        let ws = &self.workspace_path;
        let mut actions = Vec::new();

        if !ws.exists() {
            fs::create_dir_all(ws).await?;
            actions.push("Created workspace directory".into());
        }

        let memory_db_path = ws.join("memory.db");
        let memory_db_existed = memory_db_path.exists();
        let _store = StructuredMemoryStore::open(&memory_db_path)?;
        if !memory_db_existed {
            actions.push("Created memory.db".into());
        }

        let vec_db_path = ws.join("memory_vec.db");
        let vec_db_existed = vec_db_path.exists();
        let _vec_store = sqlite_vecstore::VecStore::open(
            &vec_db_path,
            sqlite_vecstore::VecStoreConfig::default(),
        )?;
        if !vec_db_existed {
            actions.push("Created memory_vec.db".into());
        }

        Ok(actions)
    }

    pub async fn create_backup(&self) -> MemoryResult<PathBuf> {
        let ws = &self.workspace_path;
        let backup_dir = ws.join("backup");
        fs::create_dir_all(&backup_dir).await?;

        Ok(backup_dir)
    }

    pub async fn reindex(&self, _scope: ReindexScope) -> MemoryResult<ReindexReport> {
        let start = std::time::Instant::now();
        let memory_db_path = self.workspace_path.join("memory.db");
        let doc_count = if memory_db_path.exists() {
            let store = StructuredMemoryStore::open(&memory_db_path)?;
            store.rebuild_derived_indexes()?
        } else {
            0
        };

        Ok(ReindexReport {
            documents_indexed: doc_count,
            errors: vec![],
            duration_ms: start.elapsed().as_millis() as u64,
        })
    }
}
