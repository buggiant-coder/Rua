use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use rua_config::SkillScannerConfig;
use rua_skills::scanner_integration::ScannerHandle as ScannerHandleTrait;
use rua_skills::SkillError;

use crate::scan_skill_directory_with_config;
use crate::types::{Finding, SkillScanError};

#[derive(Debug, Clone, Default)]
pub struct ScannerHandle {
    config: Option<Arc<SkillScannerConfig>>,
}

impl ScannerHandle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config(config: SkillScannerConfig) -> Self {
        Self {
            config: Some(Arc::new(config)),
        }
    }
}

impl ScannerHandleTrait for ScannerHandle {
    fn scan_blocking(&self, dir: &Path, skill_name: &str) -> Result<(), SkillError> {
        let cfg = self.config.clone();
        let dir_owned: PathBuf = dir.to_path_buf();
        let name_owned = skill_name.to_string();

        let scan_result = run_scan_blocking(async move {
            scan_skill_directory_with_config(&dir_owned, &name_owned, None, None, cfg.as_deref())
                .await
        });

        match scan_result {
            Ok(_) => Ok(()),
            Err(scan_err) => Err(scan_err_to_skill(scan_err, skill_name)),
        }
    }
}

pub fn default_scanner_handle() -> Arc<dyn ScannerHandleTrait> {
    Arc::new(ScannerHandle::new())
}

pub fn scanner_handle_with_config(config: SkillScannerConfig) -> Arc<dyn ScannerHandleTrait> {
    Arc::new(ScannerHandle::with_config(config))
}

fn scan_err_to_skill(err: SkillScanError, skill_name: &str) -> SkillError {
    let max_severity = err
        .findings
        .iter()
        .map(|f| f.severity)
        .max()
        .map(|s| s.as_str().to_string())
        .unwrap_or_else(|| "UNKNOWN".to_string());
    let findings = err.findings.iter().map(format_finding).collect();
    SkillError::ScanBlocked {
        skill_name: skill_name.to_string(),
        max_severity,
        summary: err.message,
        findings,
    }
}

fn format_finding(f: &Finding) -> String {
    let loc = match (f.file_path.as_deref(), f.line_number) {
        (Some(p), Some(l)) => format!(" ({p}:{l})"),
        (Some(p), None) => format!(" ({p})"),
        _ => String::new(),
    };
    format!("[{}] {}{loc}", f.severity.as_str(), f.title)
}

fn scanner_runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .thread_name("rua-skill-scan")
            .build()
            .expect("failed to build rua-skill-scan runtime")
    })
}

fn run_scan_blocking<F, T>(fut: F) -> T
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    scanner_runtime().spawn(async move {
        let _ = tx.send(fut.await);
    });
    rx.recv().expect("scanner runtime task channel closed")
}
