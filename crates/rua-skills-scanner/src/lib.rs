pub mod analyzers;
pub mod cache;
pub mod history;
pub mod integration;
pub mod policy;
pub mod scanner;
pub mod types;
pub mod wasm;
pub mod whitelist;

use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tokio::time::timeout;
use tracing::{debug, warn};

use rua_config::SkillScannerConfig;

pub use analyzers::{Analyzer, PatternAnalyzer, PatternRule};
pub use cache::{ScanCache, MAX_CACHE_ENTRIES};
pub use history::{
    blocked_history_path, clear_blocked_history, get_blocked_history, record_blocked_skill,
    remove_blocked_entry, BlockedSkillRecord, BLOCKED_HISTORY_FILENAME,
};
pub use policy::{
    CredentialsPolicy, FileClassPolicy, FileLimits, RuleScoping, ScanPolicy, ScanPolicyError,
};
pub use scanner::SkillScanner;
pub use types::{
    AnalyzerFailure, Finding, ScanMode, ScanResult, Severity, SkillFile, SkillScanError,
    ThreatCategory,
};
pub use whitelist::{compute_skill_content_hash, is_skill_whitelisted};

pub use integration::{default_scanner_handle, scanner_handle_with_config, ScannerHandle};
pub use wasm::{
    scan_wasm_bytes, scan_wasm_bytes_to_findings, WasmFinding, WasmScanResult, WASM_MAGIC,
};

const DEFAULT_TIMEOUT_SECONDS: u64 = 30;
const ENV_SCAN_MODE: &str = "RUA_SKILL_SCAN_MODE";

pub fn shared_scanner() -> Arc<SkillScanner> {
    static SCANNER: OnceLock<Arc<SkillScanner>> = OnceLock::new();
    Arc::clone(SCANNER.get_or_init(|| Arc::new(SkillScanner::default())))
}

pub fn shared_cache() -> &'static ScanCache {
    static CACHE: OnceLock<ScanCache> = OnceLock::new();
    CACHE.get_or_init(ScanCache::default)
}

pub fn current_scan_mode(cfg: Option<&SkillScannerConfig>) -> ScanMode {
    if let Ok(env_val) = std::env::var(ENV_SCAN_MODE) {
        if let Some(parsed) = ScanMode::parse(&env_val) {
            return parsed;
        }
    }
    if let Some(cfg) = cfg {
        if let Some(parsed) = ScanMode::parse(&cfg.mode) {
            return parsed;
        }
    }
    ScanMode::Warn
}

pub fn current_scan_timeout(cfg: Option<&SkillScannerConfig>) -> Duration {
    let secs = cfg
        .map(|c| c.timeout as u64)
        .unwrap_or(DEFAULT_TIMEOUT_SECONDS);
    Duration::from_secs(secs.max(1))
}

pub async fn scan_skill_directory(
    skill_dir: &Path,
    skill_name: &str,
    block: Option<bool>,
    timeout_override: Option<Duration>,
) -> Result<Option<ScanResult>, SkillScanError> {
    scan_skill_directory_with_config(skill_dir, skill_name, block, timeout_override, None).await
}

pub async fn scan_skill_directory_with_config(
    skill_dir: &Path,
    skill_name: &str,
    block: Option<bool>,
    timeout_override: Option<Duration>,
    cfg: Option<&SkillScannerConfig>,
) -> Result<Option<ScanResult>, SkillScanError> {
    scan_skill_directory_with(
        shared_scanner(),
        shared_cache(),
        skill_dir,
        skill_name,
        block,
        timeout_override,
        cfg,
    )
    .await
}

pub async fn scan_skill_directory_with(
    scanner: Arc<SkillScanner>,
    cache: &ScanCache,
    skill_dir: &Path,
    skill_name: &str,
    block: Option<bool>,
    timeout_override: Option<Duration>,
    cfg: Option<&SkillScannerConfig>,
) -> Result<Option<ScanResult>, SkillScanError> {
    let mode = current_scan_mode(cfg);
    if matches!(mode, ScanMode::Off) {
        debug!(skill = skill_name, "skill scan mode=off, skipping");
        return Ok(None);
    }

    let resolved = skill_dir
        .canonicalize()
        .unwrap_or_else(|_| skill_dir.to_path_buf());
    let effective_name = if skill_name.is_empty() {
        resolved
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("skill")
            .to_string()
    } else {
        skill_name.to_string()
    };

    if let Some(cfg) = cfg {
        if is_skill_whitelisted(cfg, &effective_name, Some(&resolved)) {
            debug!(
                skill = effective_name,
                "skill is whitelisted, skipping scan"
            );
            return Ok(None);
        }
    }

    if let Some(cached) = cache.get(&resolved) {
        return finalize_result(cached, &resolved, mode, block);
    }

    let effective_timeout = timeout_override.unwrap_or_else(|| current_scan_timeout(cfg));
    let scan_name = effective_name.clone();
    let scan_dir = resolved.clone();
    let scanner_for_task = Arc::clone(&scanner);
    let scan_result = timeout(
        effective_timeout,
        tokio::spawn(async move { scanner_for_task.scan_skill(&scan_dir, &scan_name).await }),
    )
    .await;

    let result = match scan_result {
        Ok(Ok(Ok(r))) => r,
        Ok(Ok(Err(err))) => return Err(err),
        Ok(Err(join_err)) => {
            warn!(skill = effective_name, error = %join_err, "scan task join failed");
            return Ok(None);
        }
        Err(_) => {
            warn!(
                skill = effective_name,
                timeout_ms = effective_timeout.as_millis(),
                "skill scan timed out",
            );
            return Ok(None);
        }
    };

    cache.put(&resolved, result.clone());
    finalize_result(result, &resolved, mode, block)
}

fn finalize_result(
    result: ScanResult,
    skill_dir: &Path,
    mode: ScanMode,
    block: Option<bool>,
) -> Result<Option<ScanResult>, SkillScanError> {
    if result.is_safe {
        return Ok(Some(result));
    }
    let should_block = block.unwrap_or(matches!(mode, ScanMode::Block));
    if should_block {
        record_blocked_skill(&result, skill_dir, "blocked");
        return Err(SkillScanError::from_result(&result));
    }
    record_blocked_skill(&result, skill_dir, "warned");
    warn!(
        skill = result.skill_name,
        max_severity = result.max_severity.as_str(),
        findings = result.findings.len(),
        "skill flagged but blocking disabled",
    );
    Ok(Some(result))
}
