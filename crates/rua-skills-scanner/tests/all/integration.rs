use std::path::Path;
use std::sync::{Mutex, OnceLock};

use rua_config::SkillScannerConfig;
use rua_skills::scanner_integration::ScannerHandle as ScannerHandleTrait;
use rua_skills::SkillError;
use rua_skills_scanner::integration::{default_scanner_handle, ScannerHandle};
use tempfile::TempDir;

const CLEAN_SKILL_MD: &str = "---
name: demo-clean
description: harmless skill used in scanner tests
---

# Demo body
";

const MALICIOUS_SKILL_MD: &str = "---
name: demo-evil
description: skill containing dangerous python code
---

# Demo body — see scripts/danger.py
";

const MALICIOUS_PY: &str = "import subprocess\nsubprocess.call('echo hi', shell=True)\n";

fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

struct EnvGuard {
    _working_dir: TempDir,
    old_home: Option<String>,
}

impl EnvGuard {
    fn install() -> Self {
        let dir = tempfile::tempdir().expect("tempdir for HOME redirect");
        let old_home = std::env::var("HOME").ok();

        unsafe {
            std::env::set_var("HOME", dir.path());
            std::env::remove_var("RUA_SKILL_SCAN_MODE");
        }
        std::fs::create_dir_all(dir.path().join(".rua")).expect("create .rua dir");
        Self {
            _working_dir: dir,
            old_home,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.old_home {
                Some(h) => std::env::set_var("HOME", h),
                None => std::env::remove_var("HOME"),
            }
            std::env::remove_var("RUA_SKILL_SCAN_MODE");
        }
    }
}

fn write_clean_skill(dir: &Path) {
    std::fs::write(dir.join("SKILL.md"), CLEAN_SKILL_MD).unwrap();
}

fn write_malicious_skill(dir: &Path) {
    std::fs::write(dir.join("SKILL.md"), MALICIOUS_SKILL_MD).unwrap();
    std::fs::write(dir.join("main.py"), MALICIOUS_PY).unwrap();
}

fn config(mode: &str) -> SkillScannerConfig {
    SkillScannerConfig {
        mode: mode.to_string(),
        timeout: 30,
        whitelist: Vec::new(),
        policy: None,
        extra: serde_json::Map::new(),
    }
}

#[test]
fn block_mode_rejects_malicious_skill_with_findings() {
    let _guard = test_lock();
    let _env = EnvGuard::install();

    let skill_dir = tempfile::tempdir().unwrap();
    write_malicious_skill(skill_dir.path());

    let handle = ScannerHandle::with_config(config("block"));
    let err = handle
        .scan_blocking(skill_dir.path(), "demo-evil")
        .expect_err("block mode must reject malicious skill");

    match err {
        SkillError::ScanBlocked {
            skill_name,
            max_severity,
            findings,
            summary,
        } => {
            assert_eq!(skill_name, "demo-evil");
            assert!(
                max_severity == "HIGH" || max_severity == "CRITICAL",
                "expected HIGH or CRITICAL, got {max_severity}"
            );
            assert!(
                !findings.is_empty(),
                "ScanBlocked must carry per-finding summaries"
            );
            assert!(
                findings
                    .iter()
                    .any(|f| f.contains("HIGH") || f.contains("CRITICAL")),
                "expected at least one HIGH/CRITICAL finding string, got {findings:?}"
            );
            assert!(
                summary.contains("demo-evil"),
                "summary should reference skill name, got: {summary}"
            );
        }
        other => panic!("expected ScanBlocked, got {other:?}"),
    }
}

#[test]
fn warn_mode_passes_malicious_skill() {
    let _guard = test_lock();
    let _env = EnvGuard::install();

    let skill_dir = tempfile::tempdir().unwrap();
    write_malicious_skill(skill_dir.path());

    let handle = ScannerHandle::with_config(config("warn"));
    handle
        .scan_blocking(skill_dir.path(), "demo-evil")
        .expect("warn mode must pass even malicious skills");
}

#[test]
fn off_mode_passes_malicious_skill() {
    let _guard = test_lock();
    let _env = EnvGuard::install();

    let skill_dir = tempfile::tempdir().unwrap();
    write_malicious_skill(skill_dir.path());

    let handle = ScannerHandle::with_config(config("off"));
    handle
        .scan_blocking(skill_dir.path(), "demo-evil")
        .expect("off mode must skip scanning entirely");
}

#[test]
fn block_mode_passes_clean_skill() {
    let _guard = test_lock();
    let _env = EnvGuard::install();

    let skill_dir = tempfile::tempdir().unwrap();
    write_clean_skill(skill_dir.path());

    let handle = ScannerHandle::with_config(config("block"));
    handle
        .scan_blocking(skill_dir.path(), "demo-clean")
        .expect("clean skill must pass even in block mode");
}

#[test]
fn env_override_beats_config_mode() {
    let _guard = test_lock();
    let _env = EnvGuard::install();

    unsafe {
        std::env::set_var("RUA_SKILL_SCAN_MODE", "off");
    }
    let skill_dir = tempfile::tempdir().unwrap();
    write_malicious_skill(skill_dir.path());

    let handle = ScannerHandle::with_config(config("block"));
    handle
        .scan_blocking(skill_dir.path(), "demo-evil")
        .expect("env mode=off must override config mode=block");
}

#[test]
fn default_scanner_handle_is_callable() {
    let _guard = test_lock();
    let _env = EnvGuard::install();

    let h = default_scanner_handle();

    let clean_dir = tempfile::tempdir().unwrap();
    write_clean_skill(clean_dir.path());
    h.scan_blocking(clean_dir.path(), "demo-clean")
        .expect("default handle must accept a clean skill");

    let dirty_dir = tempfile::tempdir().unwrap();
    write_malicious_skill(dirty_dir.path());
    h.scan_blocking(dirty_dir.path(), "demo-evil")
        .expect("default warn-mode handle must pass malicious skill without erroring");
}
