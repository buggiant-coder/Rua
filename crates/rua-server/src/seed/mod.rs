use std::path::{Path, PathBuf};

use rua_core::seed_manifest::{
    is_entry_materialized, normalize_seed_relative_path, record_seed_entry,
};
use rua_core::{Result, RuaError};

pub mod agents;
pub mod prompts;
pub mod skills;
pub mod templates;

pub trait SeedProducer: Send + Sync {
    fn entry_id(&self) -> String;
    fn producer_name(&self) -> &str;
    fn target_paths(&self) -> Vec<String>;
    fn materialize(&self, data_dir: &Path) -> Result<Vec<String>>;
}

pub struct SeedCoordinator {
    data_dir: PathBuf,
    producers: Vec<Box<dyn SeedProducer>>,
}

#[derive(Debug, Clone, Default)]
pub struct SeedRunReport {
    pub outcomes: Vec<SeedRunOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedRunOutcome {
    pub entry_id: String,
    pub producer: String,
    pub status: SeedRunStatus,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedRunStatus {
    Materialized,
    AlreadyMaterialized,
    TargetOccupied,
    Noop,
    Failed,
}

impl SeedCoordinator {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            producers: Vec::new(),
        }
    }

    pub fn with_producer(mut self, producer: Box<dyn SeedProducer>) -> Self {
        self.producers.push(producer);
        self
    }

    pub fn run(&self) -> SeedRunReport {
        let mut report = SeedRunReport::default();
        for producer in &self.producers {
            let outcome = self.run_one(producer.as_ref());
            if outcome.status == SeedRunStatus::Failed {
                tracing::warn!(
                    entry_id = %outcome.entry_id,
                    producer = %outcome.producer,
                    message = outcome.message.as_deref().unwrap_or("seed failed"),
                    "automatic seed producer failed"
                );
            }
            if outcome.status == SeedRunStatus::TargetOccupied {
                tracing::warn!(
                    entry_id = %outcome.entry_id,
                    producer = %outcome.producer,
                    message = outcome.message.as_deref().unwrap_or("seed target occupied"),
                    "automatic seed producer skipped occupied target"
                );
            }
            report.outcomes.push(outcome);
        }
        report
    }

    fn run_one(&self, producer: &dyn SeedProducer) -> SeedRunOutcome {
        let entry_id = producer.entry_id();
        let producer_name = producer.producer_name().to_string();

        match is_entry_materialized(&self.data_dir, &entry_id) {
            Ok(true) => {
                return SeedRunOutcome {
                    entry_id,
                    producer: producer_name,
                    status: SeedRunStatus::AlreadyMaterialized,
                    message: None,
                };
            }
            Ok(false) => {}
            Err(e) => {
                return failed(entry_id, producer_name, e);
            }
        }

        match self.occupied_target(&producer.target_paths()) {
            Ok(Some(path)) => {
                return SeedRunOutcome {
                    entry_id,
                    producer: producer_name,
                    status: SeedRunStatus::TargetOccupied,
                    message: Some(format!("seed target already exists: {path}")),
                };
            }
            Ok(None) => {}
            Err(e) => {
                return failed(entry_id, producer_name, e);
            }
        }

        let materialized_paths = match producer.materialize(&self.data_dir) {
            Ok(paths) => paths,
            Err(e) => return failed(entry_id, producer_name, e),
        };
        if materialized_paths.is_empty() {
            return SeedRunOutcome {
                entry_id,
                producer: producer_name,
                status: SeedRunStatus::Noop,
                message: None,
            };
        }
        let borrowed: Vec<&str> = materialized_paths.iter().map(String::as_str).collect();
        match record_seed_entry(
            &self.data_dir,
            &entry_id,
            &producer_name,
            borrowed.as_slice(),
        ) {
            Ok(_) => SeedRunOutcome {
                entry_id,
                producer: producer_name,
                status: SeedRunStatus::Materialized,
                message: None,
            },
            Err(e) => failed(entry_id, producer_name, e),
        }
    }

    fn occupied_target(&self, paths: &[String]) -> Result<Option<String>> {
        for path in paths {
            let normalized = normalize_seed_relative_path(path)?;
            if self.data_dir.join(&normalized).exists() {
                return Ok(Some(normalized));
            }
        }
        Ok(None)
    }
}

fn failed(entry_id: String, producer: String, error: RuaError) -> SeedRunOutcome {
    SeedRunOutcome {
        entry_id,
        producer,
        status: SeedRunStatus::Failed,
        message: Some(error.to_string()),
    }
}
