use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::error::SkillError;

pub trait ScannerHandle: Send + Sync + std::fmt::Debug {
    fn scan_blocking(&self, dir: &Path, skill_name: &str) -> Result<(), SkillError>;
}

pub fn maybe_scan_skill(
    handle: Option<&Arc<dyn ScannerHandle>>,
    dir: &Path,
    skill_name: &str,
) -> Result<(), SkillError> {
    match handle {
        None => Ok(()),
        Some(h) => h.scan_blocking(dir, skill_name),
    }
}

#[derive(Debug)]
pub struct InMemoryScannerHandle {
    mode: InMemoryMode,
    calls: Mutex<Vec<ScanCall>>,
}

#[derive(Debug, Clone)]
enum InMemoryMode {
    Allow,
    Block {
        max_severity: String,
        summary: String,
        findings: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanCall {
    pub skill_name: String,
    pub dir: std::path::PathBuf,
}

impl InMemoryScannerHandle {
    pub fn allow() -> Self {
        Self {
            mode: InMemoryMode::Allow,
            calls: Mutex::new(Vec::new()),
        }
    }

    pub fn block_with(
        max_severity: impl Into<String>,
        summary: impl Into<String>,
        findings: Vec<String>,
    ) -> Self {
        Self {
            mode: InMemoryMode::Block {
                max_severity: max_severity.into(),
                summary: summary.into(),
                findings,
            },
            calls: Mutex::new(Vec::new()),
        }
    }

    pub fn calls(&self) -> Vec<ScanCall> {
        self.calls
            .lock()
            .expect("InMemoryScannerHandle mutex")
            .clone()
    }
}

impl Default for InMemoryScannerHandle {
    fn default() -> Self {
        Self::allow()
    }
}

impl ScannerHandle for InMemoryScannerHandle {
    fn scan_blocking(&self, dir: &Path, skill_name: &str) -> Result<(), SkillError> {
        self.calls
            .lock()
            .expect("InMemoryScannerHandle mutex")
            .push(ScanCall {
                skill_name: skill_name.to_string(),
                dir: dir.to_path_buf(),
            });
        match &self.mode {
            InMemoryMode::Allow => Ok(()),
            InMemoryMode::Block {
                max_severity,
                summary,
                findings,
            } => Err(SkillError::ScanBlocked {
                skill_name: skill_name.to_string(),
                max_severity: max_severity.clone(),
                summary: summary.clone(),
                findings: findings.clone(),
            }),
        }
    }
}
