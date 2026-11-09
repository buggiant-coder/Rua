use std::path::PathBuf;
use std::sync::Arc;

use cell_store::{CellStore, CellStoreError, FileSystemCellStore};
use rua_config::types::cron::{CronJobSpec, JobsFile};
use rua_contract::cron::{CronJob, SystemScheduler};
use rua_core::RuaError;
use tracing::{debug, warn};

use crate::manager::{compute_next_run_for_schedule, spec_to_cron_job};

const JOBS_FILE_ROOT: &str = "$";

pub struct CronCells {
    store: Arc<FileSystemCellStore>,
    scheduler: Arc<dyn SystemScheduler>,
    file: PathBuf,
}

impl CronCells {
    pub fn new(
        store: Arc<FileSystemCellStore>,
        scheduler: Arc<dyn SystemScheduler>,
        file: PathBuf,
    ) -> Self {
        Self {
            store,
            scheduler,
            file,
        }
    }

    pub fn cell<'a>(&'a self, job_id: &str) -> CronCell<'a> {
        CronCell {
            cells: self,
            job_id: job_id.to_string(),
        }
    }

    pub fn file(&self) -> &std::path::Path {
        &self.file
    }

    pub async fn list(&self) -> Result<Vec<CronJobSpec>, RuaError> {
        let file: Option<JobsFile> = self
            .store
            .read(&self.file, JOBS_FILE_ROOT)
            .await
            .map_err(cell_err_to_rua)?;
        Ok(file
            .map(|f| f.jobs.into_values().collect())
            .unwrap_or_default())
    }
}

pub struct CronCell<'a> {
    cells: &'a CronCells,
    job_id: String,
}

impl<'a> CronCell<'a> {
    pub async fn read(&self) -> Result<Option<CronJobSpec>, RuaError> {
        let file: Option<JobsFile> = self
            .cells
            .store
            .read(&self.cells.file, JOBS_FILE_ROOT)
            .await
            .map_err(cell_err_to_rua)?;
        Ok(file.and_then(|f| f.jobs.get(&self.job_id).cloned()))
    }

    pub async fn write(&self, spec: CronJobSpec) -> Result<(), RuaError> {
        let job_id = self.job_id.clone();
        let _ = self
            .cells
            .store
            .update::<JobsFile, _>(&self.cells.file, JOBS_FILE_ROOT, move |existing| {
                let mut file = existing.unwrap_or_default();
                file.jobs.insert(job_id.clone(), spec.clone());
                file
            })
            .await
            .map_err(cell_err_to_rua)?;
        let written = self.read().await?.expect("just wrote it");
        self.reconcile_scheduler(&written).await;
        Ok(())
    }

    pub async fn update<F>(&self, mutator: F) -> Result<CronJobSpec, RuaError>
    where
        F: FnOnce(Option<CronJobSpec>) -> CronJobSpec + Send,
    {
        let job_id = self.job_id.clone();
        let new_file = self
            .cells
            .store
            .update::<JobsFile, _>(&self.cells.file, JOBS_FILE_ROOT, move |existing| {
                let mut file = existing.unwrap_or_default();
                let current = file.jobs.get(&job_id).cloned();
                let new_spec = mutator(current);
                file.jobs.insert(job_id.clone(), new_spec);
                file
            })
            .await
            .map_err(cell_err_to_rua)?;
        let new_spec = new_file
            .jobs
            .get(&self.job_id)
            .cloned()
            .expect("just inserted");
        self.reconcile_scheduler(&new_spec).await;
        Ok(new_spec)
    }

    pub async fn delete(&self) -> Result<(), RuaError> {
        let job_id = self.job_id.clone();
        let _ = self
            .cells
            .store
            .update::<JobsFile, _>(&self.cells.file, JOBS_FILE_ROOT, move |existing| {
                let mut file = existing.unwrap_or_default();
                file.jobs.remove(&job_id);
                file
            })
            .await
            .map_err(cell_err_to_rua)?;
        if let Err(e) = self.cells.scheduler.cancel(&self.job_id).await {
            warn!(
                job_id = %self.job_id,
                error = %e,
                "scheduler cancel failed after delete; timer registry may have stale entry"
            );
        }
        debug!(job_id = %self.job_id, "deleted cron cell and cancelled timer");
        Ok(())
    }

    async fn reconcile_scheduler(&self, spec: &CronJobSpec) {
        if !spec.enabled || spec.paused {
            if let Err(e) = self.cells.scheduler.cancel(&self.job_id).await {
                warn!(
                    job_id = %self.job_id,
                    paused = spec.paused,
                    enabled = spec.enabled,
                    error = %e,
                    "scheduler cancel failed during reconcile"
                );
            } else {
                debug!(
                    job_id = %self.job_id,
                    paused = spec.paused,
                    enabled = spec.enabled,
                    "cancelled cron timer after write"
                );
            }
            return;
        }

        let Some(next) = compute_next_run_for_schedule(&spec.schedule) else {
            debug!(
                job_id = %self.job_id,
                schedule_type = %spec.schedule.r#type,
                "compute_next_run returned None; skipping timer registration"
            );
            return;
        };
        let cron_job: CronJob = spec_to_cron_job(spec);
        if let Err(e) = self
            .cells
            .scheduler
            .register_next_run(&cron_job, next)
            .await
        {
            warn!(
                job_id = %self.job_id,
                error = %e,
                "scheduler register_next_run failed after write"
            );
        } else {
            debug!(
                job_id = %self.job_id,
                next_run_at = %next,
                "registered cron timer after write"
            );
        }
    }
}

fn cell_err_to_rua(err: CellStoreError) -> RuaError {
    RuaError::Cron {
        job_id: String::new(),
        message: format!("cron cell store error: {err}"),
    }
}
