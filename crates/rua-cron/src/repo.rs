use rua_config::types::cron::{CronJobSpec, JobsFile};
use std::path::{Path, PathBuf};
use tracing;

#[derive(Debug)]
pub enum RepoError {
    Io(std::io::Error),

    Json(serde_json::Error),
}

impl std::fmt::Display for RepoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "repository IO error: {e}"),
            Self::Json(e) => write!(f, "repository JSON error: {e}"),
        }
    }
}

impl std::error::Error for RepoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Json(e) => Some(e),
        }
    }
}

impl From<std::io::Error> for RepoError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for RepoError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

pub struct JsonCronJobRepository {
    path: PathBuf,
}

impl JsonCronJobRepository {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<JobsFile, RepoError> {
        if !self.path.exists() {
            tracing::trace!("Jobs file {:?} does not exist, returning empty", self.path);
            return Ok(JobsFile::default());
        }

        let content = std::fs::read_to_string(&self.path)?;
        if content.trim().is_empty() {
            tracing::debug!("Jobs file {:?} is empty, returning default", self.path);
            return Ok(JobsFile::default());
        }

        match serde_json::from_str(&content) {
            Ok(jobs_file) => Ok(jobs_file),
            Err(e) => {
                tracing::error!("Failed to parse jobs file {:?}: {}", self.path, e);
                Err(RepoError::Json(e))
            }
        }
    }

    pub fn save(&self, jobs_file: &JobsFile) -> Result<(), RepoError> {
        let content = serde_json::to_string_pretty(jobs_file)?;

        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let temp_path = self.path.with_extension("json.tmp");
        std::fs::write(&temp_path, &content)?;
        std::fs::rename(&temp_path, &self.path)?;

        tracing::debug!("Saved {} jobs to {:?}", jobs_file.jobs.len(), self.path);
        Ok(())
    }

    pub fn load_jobs(&self) -> Result<Vec<CronJobSpec>, RepoError> {
        let jobs_file = self.load()?;
        Ok(jobs_file.jobs.into_values().collect())
    }

    pub fn save_jobs(&self, jobs: Vec<CronJobSpec>) -> Result<(), RepoError> {
        let mut jobs_file = self.load().unwrap_or_default();
        jobs_file.jobs = jobs
            .into_iter()
            .filter(|spec| !spec.id.is_empty())
            .map(|spec| (spec.id.clone(), spec))
            .collect();
        self.save(&jobs_file)
    }

    pub fn get_job(&self, job_id: &str) -> Result<Option<CronJobSpec>, RepoError> {
        let jobs_file = self.load()?;
        Ok(jobs_file.jobs.get(job_id).cloned())
    }

    pub fn upsert_job(&self, job: CronJobSpec) -> Result<bool, RepoError> {
        let mut jobs_file = self.load()?;
        let replaced = jobs_file.jobs.insert(job.id.clone(), job).is_some();
        self.save(&jobs_file)?;
        Ok(replaced)
    }

    pub fn delete_job(&self, job_id: &str) -> Result<Option<CronJobSpec>, RepoError> {
        let mut jobs_file = self.load()?;
        let removed = jobs_file.jobs.remove(job_id);
        if removed.is_some() {
            self.save(&jobs_file)?;
        }
        Ok(removed)
    }
}
