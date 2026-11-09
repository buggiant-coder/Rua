use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tempfile::TempDir;

use rua_config::types::security::SkillScannerWhitelistEntry;
use rua_config::SkillScannerConfig;
use rua_skills_scanner::{
    compute_skill_content_hash, is_skill_whitelisted, scan_skill_directory_with, Analyzer, Finding,
    PatternAnalyzer, ScanCache, ScanPolicy, Severity, SkillFile, SkillScanner, ThreatCategory,
    MAX_CACHE_ENTRIES,
};

fn write(dir: &Path, name: &str, body: &str) {
    std::fs::write(dir.join(name), body).unwrap();
}

fn make_skill_dir(body: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "SKILL.md", "# example\n");
    write(dir.path(), "main.py", body);
    dir
}

fn warn_cfg() -> SkillScannerConfig {
    SkillScannerConfig {
        mode: "warn".into(),
        timeout: 30,
        whitelist: Vec::new(),
        policy: None,
        extra: serde_json::Map::new(),
    }
}

fn block_cfg() -> SkillScannerConfig {
    SkillScannerConfig {
        mode: "block".into(),
        timeout: 30,
        whitelist: Vec::new(),
        policy: None,
        extra: serde_json::Map::new(),
    }
}

#[tokio::test]
async fn scans_clean_skill_returns_safe_result() {
    let dir = make_skill_dir("def add(a, b):\n    return a + b\n");
    let scanner = Arc::new(SkillScanner::default());
    let cache = ScanCache::default();
    let cfg = warn_cfg();

    let result = scan_skill_directory_with(
        scanner,
        &cache,
        dir.path(),
        "clean_skill",
        None,
        None,
        Some(&cfg),
    )
    .await
    .expect("scan should succeed");

    let result = result.expect("scan returned a result");
    assert!(result.is_safe, "expected safe scan, got {result:?}");
}

#[tokio::test]
async fn scanner_with_no_analyzers_returns_safe_result() {
    let dir = make_skill_dir("def add(a, b):\n    return a + b\n");
    let scanner = SkillScanner::with_analyzers(Arc::new(ScanPolicy::default()), Vec::new());

    let result = scanner
        .scan_skill(dir.path(), "no-analyzer-skill")
        .await
        .expect("scan should succeed without analyzers");

    assert!(result.is_safe);
    assert!(result.findings.is_empty());
    assert!(result.analyzers_used.is_empty());
    assert!(result.analyzers_failed.is_empty());
}

#[tokio::test]
async fn scanner_dedupes_duplicate_findings_across_analyzers_by_default() {
    let dir = make_skill_dir("danger\n");
    let policy = Arc::new(ScanPolicy::default());
    let scanner = SkillScanner::with_analyzers(
        policy,
        vec![
            Arc::new(FixedFindingAnalyzer::new("first")),
            Arc::new(FixedFindingAnalyzer::new("second")),
        ],
    );

    let result = scanner
        .scan_skill(dir.path(), "dedupe-skill")
        .await
        .expect("scan should aggregate analyzers");

    assert_eq!(result.analyzers_used, vec!["first", "second"]);
    assert_eq!(
        result.findings.len(),
        1,
        "duplicate finding ids from different analyzers should be collapsed"
    );
}

#[tokio::test]
async fn scanner_preserves_duplicate_findings_when_policy_disables_dedupe() {
    let dir = make_skill_dir("danger\n");
    let mut policy = ScanPolicy::default();
    policy.rule_scoping.dedupe_duplicate_findings = false;
    let scanner = SkillScanner::with_analyzers(
        Arc::new(policy),
        vec![
            Arc::new(FixedFindingAnalyzer::new("first")),
            Arc::new(FixedFindingAnalyzer::new("second")),
        ],
    );

    let result = scanner
        .scan_skill(dir.path(), "no-dedupe-skill")
        .await
        .expect("scan should aggregate analyzers");

    assert_eq!(result.analyzers_used, vec!["first", "second"]);
    assert_eq!(
        result.findings.len(),
        2,
        "dedupe=false should keep duplicate finding ids from separate analyzers"
    );
}

#[tokio::test]
async fn scanner_records_analyzer_panics_and_continues_other_analyzers() {
    let dir = make_skill_dir("danger\n");
    let policy = Arc::new(ScanPolicy::default());
    let scanner = SkillScanner::with_analyzers(
        policy,
        vec![
            Arc::new(PanicAnalyzer),
            Arc::new(FixedFindingAnalyzer::new("after-panic")),
        ],
    );

    let result = scanner
        .scan_skill(dir.path(), "panic-skill")
        .await
        .expect("scan should survive analyzer panic");

    assert_eq!(result.analyzers_used, vec!["after-panic"]);
    assert_eq!(result.analyzers_failed.len(), 1);
    assert_eq!(result.analyzers_failed[0].analyzer, "panic");
    assert!(
        result.analyzers_failed[0].error.contains("boom"),
        "panic payload should be surfaced for diagnostics: {:?}",
        result.analyzers_failed
    );
    assert_eq!(
        result.findings.len(),
        1,
        "healthy analyzers should still contribute findings after a panic"
    );
}

#[tokio::test]
async fn block_mode_returns_error_on_critical_findings() {
    let body = "import subprocess\nsubprocess.call('echo hi', shell=True)\n";
    let dir = make_skill_dir(body);
    let scanner = Arc::new(SkillScanner::default());
    let cache = ScanCache::default();
    let cfg = block_cfg();

    let err =
        scan_skill_directory_with(scanner, &cache, dir.path(), "evil", None, None, Some(&cfg))
            .await
            .expect_err("block mode should propagate SkillScanError");
    assert!(!err.findings.is_empty());
}

#[tokio::test]
async fn warn_mode_returns_unsafe_result_without_error() {
    let body = "import subprocess\nsubprocess.call('echo hi', shell=True)\n";
    let dir = make_skill_dir(body);
    let scanner = Arc::new(SkillScanner::default());
    let cache = ScanCache::default();
    let cfg = warn_cfg();

    let result =
        scan_skill_directory_with(scanner, &cache, dir.path(), "evil", None, None, Some(&cfg))
            .await
            .expect("warn mode should not error");
    let result = result.expect("warn mode returns Some");
    assert!(!result.is_safe, "expected unsafe result");
}

#[tokio::test]
async fn cache_hit_skips_second_scan() {
    let dir = make_skill_dir("def hello():\n    pass\n");
    let counter: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));
    let scanner = make_counting_scanner(counter.clone());
    let cache = ScanCache::default();
    let cfg = warn_cfg();

    let _ = scan_skill_directory_with(
        scanner.clone(),
        &cache,
        dir.path(),
        "skill_a",
        None,
        None,
        Some(&cfg),
    )
    .await
    .unwrap();
    let _ = scan_skill_directory_with(
        scanner.clone(),
        &cache,
        dir.path(),
        "skill_a",
        None,
        None,
        Some(&cfg),
    )
    .await
    .unwrap();

    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "cache hit should skip the second scan",
    );
}

#[tokio::test]
async fn whitelist_skips_scan_entirely() {
    let dir = make_skill_dir("def hello():\n    pass\n");
    let counter: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));
    let scanner = make_counting_scanner(counter.clone());
    let cache = ScanCache::default();
    let mut cfg = warn_cfg();
    cfg.whitelist.push(SkillScannerWhitelistEntry {
        skill_name: "trusted".into(),
        content_hash: String::new(),
        added_at: String::new(),
        extra: serde_json::Map::new(),
    });

    let result = scan_skill_directory_with(
        scanner.clone(),
        &cache,
        dir.path(),
        "trusted",
        None,
        None,
        Some(&cfg),
    )
    .await
    .unwrap();

    assert!(result.is_none(), "whitelisted skill should return None");
    assert_eq!(
        counter.load(Ordering::SeqCst),
        0,
        "whitelist must short-circuit the scanner",
    );
}

#[tokio::test]
async fn whitelist_with_matching_hash_is_honoured() {
    let dir = make_skill_dir("def hello():\n    pass\n");
    let hash = compute_skill_content_hash(dir.path()).unwrap();
    let counter: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));
    let scanner = make_counting_scanner(counter.clone());
    let cache = ScanCache::default();
    let mut cfg = warn_cfg();
    cfg.whitelist.push(SkillScannerWhitelistEntry {
        skill_name: "trusted".into(),
        content_hash: hash,
        added_at: String::new(),
        extra: serde_json::Map::new(),
    });

    let result = scan_skill_directory_with(
        scanner,
        &cache,
        dir.path(),
        "trusted",
        None,
        None,
        Some(&cfg),
    )
    .await
    .unwrap();
    assert!(result.is_none());
    assert_eq!(counter.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn whitelist_hash_mismatch_falls_through_to_scan() {
    let dir = make_skill_dir("def hello():\n    pass\n");
    let counter: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));
    let scanner = make_counting_scanner(counter.clone());
    let cache = ScanCache::default();
    let mut cfg = warn_cfg();
    cfg.whitelist.push(SkillScannerWhitelistEntry {
        skill_name: "trusted".into(),
        content_hash: "0000".into(),
        added_at: String::new(),
        extra: serde_json::Map::new(),
    });
    let _ = scan_skill_directory_with(
        scanner,
        &cache,
        dir.path(),
        "trusted",
        None,
        None,
        Some(&cfg),
    )
    .await
    .unwrap();
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "hash mismatch must not skip scan",
    );
}

#[test]
fn content_hash_is_stable_for_missing_sorted_and_symlinked_skill_dirs() {
    let missing = tempfile::tempdir().unwrap().path().join("missing-skill");
    let missing_hash = compute_skill_content_hash(&missing).unwrap();
    assert_eq!(missing_hash.len(), 64);

    let a = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(a.path().join("nested")).unwrap();
    write(a.path(), "b.txt", "two");
    std::fs::write(a.path().join("nested").join("a.txt"), "one").unwrap();

    let b = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(b.path().join("nested")).unwrap();
    std::fs::write(b.path().join("nested").join("a.txt"), "one").unwrap();
    write(b.path(), "b.txt", "two");

    assert_eq!(
        compute_skill_content_hash(a.path()).unwrap(),
        compute_skill_content_hash(b.path()).unwrap(),
        "hashing must sort paths so whitelist identity is independent of directory iteration order"
    );

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            a.path().join("nested").join("a.txt"),
            a.path().join("nested").join("link.txt"),
        )
        .unwrap();
        assert_eq!(
            compute_skill_content_hash(a.path()).unwrap(),
            compute_skill_content_hash(b.path()).unwrap(),
            "symlink targets must not affect skill whitelist hashes"
        );
    }
}

#[test]
fn whitelist_hash_without_skill_dir_is_honoured_by_name() {
    let mut cfg = warn_cfg();
    cfg.whitelist.push(SkillScannerWhitelistEntry {
        skill_name: "trusted".into(),
        content_hash: "stored-hash-from-previous-scan".into(),
        added_at: String::new(),
        extra: serde_json::Map::new(),
    });

    assert!(is_skill_whitelisted(&cfg, "trusted", None));
    assert!(!is_skill_whitelisted(&cfg, "other", None));
}

#[tokio::test]
async fn timeout_returns_none_and_warns() {
    let policy = Arc::new(ScanPolicy::default());
    let slow: Arc<dyn Analyzer> = Arc::new(SleepyAnalyzer::new(Duration::from_secs(60)));
    let scanner = Arc::new(SkillScanner::with_analyzers(policy, vec![slow]));
    let cache = ScanCache::default();
    let dir = make_skill_dir("def hi(): pass\n");

    let result = scan_skill_directory_with(
        scanner,
        &cache,
        dir.path(),
        "slow",
        None,
        Some(Duration::from_millis(50)),
        Some(&warn_cfg()),
    )
    .await
    .expect("timeout path returns Ok(None)");
    assert!(result.is_none(), "expected None on timeout");
}

#[tokio::test]
async fn cache_lru_evicts_oldest_past_capacity() {
    let cache = ScanCache::default();
    assert_eq!(cache.capacity(), MAX_CACHE_ENTRIES);

    let counter: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));
    let scanner = make_counting_scanner(counter.clone());
    let cfg = warn_cfg();

    let mut dirs: Vec<TempDir> = Vec::new();
    for _ in 0..MAX_CACHE_ENTRIES {
        dirs.push(make_skill_dir("def hi(): pass\n"));
    }

    for (i, d) in dirs.iter().enumerate() {
        let _ = scan_skill_directory_with(
            scanner.clone(),
            &cache,
            d.path(),
            &format!("skill_{i}"),
            None,
            None,
            Some(&cfg),
        )
        .await
        .unwrap();
    }
    assert_eq!(cache.len(), MAX_CACHE_ENTRIES);

    let extra = make_skill_dir("def extra(): pass\n");
    let _ = scan_skill_directory_with(
        scanner.clone(),
        &cache,
        extra.path(),
        "extra_skill",
        None,
        None,
        Some(&cfg),
    )
    .await
    .unwrap();

    assert_eq!(cache.len(), MAX_CACHE_ENTRIES, "cache must stay at cap");
    let oldest = dirs[0].path().canonicalize().unwrap();
    assert!(!cache.contains(&oldest), "oldest entry must be evicted");
}

#[tokio::test]
async fn off_mode_returns_none() {
    let dir = make_skill_dir("def hi(): pass\n");
    let scanner = Arc::new(SkillScanner::default());
    let cache = ScanCache::default();
    let mut cfg = warn_cfg();
    cfg.mode = "off".into();
    let result =
        scan_skill_directory_with(scanner, &cache, dir.path(), "x", None, None, Some(&cfg))
            .await
            .unwrap();
    assert!(result.is_none(), "off mode returns None");
}

#[tokio::test]
async fn discovery_skips_inert_extensions() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "SKILL.md", "# demo\n");
    write(dir.path(), "main.py", "def run(): pass\n");
    write(dir.path(), "logo.png", "not really an image");
    write(dir.path(), "font.woff2", "font");
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let scanner = make_recording_scanner(seen.clone(), ScanPolicy::default());

    let result = scanner
        .scan_skill(dir.path(), "asset-skill")
        .await
        .expect("scan");

    assert!(result.is_safe);
    let seen = seen.lock().unwrap();
    assert!(seen.iter().any(|p| p == "SKILL.md"), "seen={seen:?}");
    assert!(seen.iter().any(|p| p == "main.py"), "seen={seen:?}");
    assert!(!seen.iter().any(|p| p == "logo.png"), "seen={seen:?}");
    assert!(!seen.iter().any(|p| p == "font.woff2"), "seen={seen:?}");
}

#[cfg(unix)]
#[tokio::test]
async fn discovery_skips_symlinks() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "SKILL.md", "# demo\n");
    write(dir.path(), "real.py", "def run(): pass\n");
    std::os::unix::fs::symlink(dir.path().join("real.py"), dir.path().join("link.py")).unwrap();
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let scanner = make_recording_scanner(seen.clone(), ScanPolicy::default());

    scanner
        .scan_skill(dir.path(), "symlink-skill")
        .await
        .expect("scan");

    let seen = seen.lock().unwrap();
    assert!(seen.iter().any(|p| p == "real.py"), "seen={seen:?}");
    assert!(!seen.iter().any(|p| p == "link.py"), "seen={seen:?}");
}

#[tokio::test]
async fn discovery_respects_file_size_limit() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "small.py", "x");
    write(dir.path(), "large.py", "0123456789");
    let mut policy = ScanPolicy::default();
    policy.file_limits.max_file_size_bytes = 4;
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let scanner = make_recording_scanner(seen.clone(), policy);

    scanner
        .scan_skill(dir.path(), "size-limit-skill")
        .await
        .expect("scan");

    let seen = seen.lock().unwrap();
    assert!(seen.iter().any(|p| p == "small.py"), "seen={seen:?}");
    assert!(!seen.iter().any(|p| p == "large.py"), "seen={seen:?}");
}

#[tokio::test]
async fn discovery_respects_file_count_limit() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.py", "a");
    write(dir.path(), "b.py", "b");
    write(dir.path(), "c.py", "c");
    let mut policy = ScanPolicy::default();
    policy.file_limits.max_file_count = 2;
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let scanner = make_recording_scanner(seen.clone(), policy);

    scanner
        .scan_skill(dir.path(), "count-limit-skill")
        .await
        .expect("scan");

    let seen = seen.lock().unwrap();
    assert!(
        seen.len() <= 2,
        "scanner must not pass more files than max_file_count; seen={seen:?}"
    );
}

#[tokio::test]
async fn discovery_applies_file_count_limit_after_stable_path_sorting() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "c.py", "c");
    write(dir.path(), "a.py", "a");
    write(dir.path(), "b.py", "b");
    let mut policy = ScanPolicy::default();
    policy.file_limits.max_file_count = 2;
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let scanner = make_recording_scanner(seen.clone(), policy);

    scanner
        .scan_skill(dir.path(), "sorted-count-limit-skill")
        .await
        .expect("scan");

    let seen = seen.lock().unwrap();
    assert_eq!(
        seen.as_slice(),
        ["a.py", "b.py"],
        "max_file_count should select deterministic lexicographic paths"
    );
}

fn make_counting_scanner(counter: Arc<AtomicUsize>) -> Arc<SkillScanner> {
    let policy = Arc::new(ScanPolicy::default());
    let analyzer: Arc<dyn Analyzer> = Arc::new(CountingAnalyzer::new(
        PatternAnalyzer::new(Arc::clone(&policy)),
        counter,
    ));
    Arc::new(SkillScanner::with_analyzers(policy, vec![analyzer]))
}

fn make_recording_scanner(seen: Arc<Mutex<Vec<String>>>, policy: ScanPolicy) -> SkillScanner {
    let policy = Arc::new(policy);
    let analyzer: Arc<dyn Analyzer> = Arc::new(RecordingAnalyzer { seen });
    SkillScanner::with_analyzers(policy, vec![analyzer])
}

struct CountingAnalyzer {
    inner: PatternAnalyzer,
    counter: Arc<AtomicUsize>,
}

impl CountingAnalyzer {
    fn new(inner: PatternAnalyzer, counter: Arc<AtomicUsize>) -> Self {
        Self { inner, counter }
    }
}

#[async_trait]
impl Analyzer for CountingAnalyzer {
    fn name(&self) -> &str {
        "counting"
    }
    async fn analyze(&self, files: &mut [SkillFile]) -> Vec<Finding> {
        self.counter.fetch_add(1, Ordering::SeqCst);
        self.inner.analyze(files).await
    }
}

struct SleepyAnalyzer {
    sleep: Duration,
}

impl SleepyAnalyzer {
    fn new(sleep: Duration) -> Self {
        Self { sleep }
    }
}

#[async_trait]
impl Analyzer for SleepyAnalyzer {
    fn name(&self) -> &str {
        "sleepy"
    }
    async fn analyze(&self, _files: &mut [SkillFile]) -> Vec<Finding> {
        tokio::time::sleep(self.sleep).await;
        Vec::new()
    }
}

struct RecordingAnalyzer {
    seen: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl Analyzer for RecordingAnalyzer {
    fn name(&self) -> &str {
        "recording"
    }

    async fn analyze(&self, files: &mut [SkillFile]) -> Vec<Finding> {
        let mut seen = self.seen.lock().unwrap();
        seen.extend(files.iter().map(|f| f.relative_path.clone()));
        Vec::new()
    }
}

struct FixedFindingAnalyzer {
    name: &'static str,
}

impl FixedFindingAnalyzer {
    fn new(name: &'static str) -> Self {
        Self { name }
    }
}

struct PanicAnalyzer;

#[async_trait]
impl Analyzer for PanicAnalyzer {
    fn name(&self) -> &str {
        "panic"
    }

    async fn analyze(&self, _files: &mut [SkillFile]) -> Vec<Finding> {
        panic!("boom from analyzer");
    }
}

#[async_trait]
impl Analyzer for FixedFindingAnalyzer {
    fn name(&self) -> &str {
        self.name
    }

    async fn analyze(&self, _files: &mut [SkillFile]) -> Vec<Finding> {
        vec![Finding {
            id: "DUPLICATE_RULE:main.py:1".into(),
            rule_id: "DUPLICATE_RULE".into(),
            category: ThreatCategory::CommandInjection,
            severity: Severity::High,
            title: "Duplicate finding".into(),
            description: "same finding reported by multiple analyzers".into(),
            file_path: Some("main.py".into()),
            line_number: Some(1),
            snippet: Some("danger".into()),
            remediation: None,
            analyzer: Some(self.name.into()),
            metadata: serde_json::Map::new(),
        }]
    }
}
