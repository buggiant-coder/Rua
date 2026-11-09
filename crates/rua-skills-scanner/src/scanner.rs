use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use futures_util::FutureExt;
use tracing::{debug, warn};

use crate::analyzers::{Analyzer, PatternAnalyzer};
use crate::policy::ScanPolicy;
use crate::types::{AnalyzerFailure, ScanResult, Severity, SkillFile, SkillScanError};
use crate::wasm::{self, WasmScanResult};

pub struct SkillScanner {
    policy: Arc<ScanPolicy>,
    analyzers: Vec<Arc<dyn Analyzer>>,
}

impl Default for SkillScanner {
    fn default() -> Self {
        Self::new(Arc::new(ScanPolicy::default()))
    }
}

impl SkillScanner {
    pub fn new(policy: Arc<ScanPolicy>) -> Self {
        let pattern: Arc<dyn Analyzer> = Arc::new(PatternAnalyzer::new(Arc::clone(&policy)));
        Self {
            policy,
            analyzers: vec![pattern],
        }
    }

    pub fn with_analyzers(policy: Arc<ScanPolicy>, analyzers: Vec<Arc<dyn Analyzer>>) -> Self {
        Self { policy, analyzers }
    }

    pub fn policy(&self) -> &ScanPolicy {
        &self.policy
    }

    pub fn scan_wasm_bytes(&self, bytes: &[u8]) -> WasmScanResult {
        wasm::scan_wasm_bytes(bytes, Severity::High)
    }

    pub fn scan_bytes(&self, bytes: &[u8]) -> WasmScanResult {
        wasm::scan_wasm_bytes(bytes, Severity::High)
    }

    pub async fn scan_skill(
        &self,
        skill_dir: &Path,
        skill_name: &str,
    ) -> Result<ScanResult, SkillScanError> {
        let started = Instant::now();
        let resolved = skill_dir
            .canonicalize()
            .unwrap_or_else(|_| skill_dir.to_path_buf());

        if !resolved.is_dir() {
            warn!(dir = %resolved.display(), "skill directory does not exist");
            return Ok(ScanResult::compute(
                skill_name.to_string(),
                resolved.display().to_string(),
                Vec::new(),
                started.elapsed().as_secs_f64(),
                Vec::new(),
                Vec::new(),
            ));
        }

        let mut files = self.discover_files(&resolved);
        debug!(count = files.len(), dir = %resolved.display(), "discovered scannable files");

        let mut all_findings = Vec::new();
        let mut analyzers_used = Vec::new();
        let mut analyzers_failed = Vec::new();
        for analyzer in &self.analyzers {
            let name = analyzer.name().to_string();
            match AssertUnwindSafe(analyzer.analyze(&mut files))
                .catch_unwind()
                .await
            {
                Ok(findings) => {
                    all_findings.extend(findings);
                    analyzers_used.push(name);
                }
                Err(panic) => {
                    let error = panic_payload_to_string(&panic);
                    warn!(
                        analyzer = %name,
                        error = %error,
                        "skill scanner analyzer panicked; continuing scan"
                    );
                    analyzers_failed.push(AnalyzerFailure {
                        analyzer: name,
                        error,
                    });
                }
            }
        }

        if self.policy.rule_scoping.dedupe_duplicate_findings {
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            all_findings.retain(|f| seen.insert(f.id.clone()));
        }

        let elapsed = started.elapsed().as_secs_f64();
        let result = ScanResult::compute(
            skill_name.to_string(),
            resolved.display().to_string(),
            all_findings,
            elapsed,
            analyzers_used,
            analyzers_failed,
        );

        Ok(result)
    }

    fn discover_files(&self, root: &Path) -> Vec<SkillFile> {
        let mut out: Vec<SkillFile> = Vec::new();
        let limits = &self.policy.file_limits;
        walk(
            root,
            root,
            &mut out,
            limits.max_file_count,
            limits.max_file_size_bytes,
            &self.policy,
        );
        out
    }
}

fn panic_payload_to_string(panic: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = panic.downcast_ref::<&'static str>() {
        (*message).to_string()
    } else if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else {
        "analyzer panicked".to_string()
    }
}

fn walk(
    base: &Path,
    dir: &Path,
    out: &mut Vec<SkillFile>,
    max_files: usize,
    max_size: u64,
    policy: &ScanPolicy,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(err) => {
            warn!(dir = %dir.display(), error = %err, "cannot walk dir");
            return;
        }
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.path());

    for entry in entries {
        if out.len() >= max_files {
            warn!(
                cap = max_files,
                dir = %base.display(),
                "hit max file cap during discovery",
            );
            return;
        }
        let path: PathBuf = entry.path();
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if file_type.is_symlink() {
            warn!(path = %path.display(), "skipping symlink");
            continue;
        }
        if file_type.is_dir() {
            walk(base, &path, out, max_files, max_size, policy);
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let resolved = match path.canonicalize() {
            Ok(p) => p,
            Err(_) => continue,
        };
        let base_resolved = base.canonicalize().unwrap_or_else(|_| base.to_path_buf());
        if !resolved.starts_with(&base_resolved) {
            warn!(path = %path.display(), real = %resolved.display(), "skipping file outside skill dir");
            continue;
        }
        let ext_with_dot = path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| format!(".{}", s.to_lowercase()))
            .unwrap_or_default();
        if !ext_with_dot.is_empty() && policy.should_skip_extension(&ext_with_dot) {
            continue;
        }
        let size = match resolved.metadata() {
            Ok(m) => m.len(),
            Err(_) => continue,
        };
        if size > max_size {
            debug!(path = %path.display(), size, "skipping oversized file");
            continue;
        }
        out.push(SkillFile::from_path(&path, base));
    }
}
