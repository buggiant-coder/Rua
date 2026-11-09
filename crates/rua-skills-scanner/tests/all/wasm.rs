use rua_skills_scanner::types::Severity;
use rua_skills_scanner::wasm::WASM_MAGIC;
use rua_skills_scanner::SkillScanner;

#[test]
fn scan_wasm_bytes_detects_proc_exit() {
    let mut bytes = WASM_MAGIC.to_vec();
    bytes.extend_from_slice(b"\x00\x00\x00proc_exit\x00");
    let scanner = SkillScanner::default();
    let result = scanner.scan_wasm_bytes(&bytes);
    assert!(!result.passed);
    assert!(result
        .findings
        .iter()
        .any(|f| f.rule_id == "wasm-import-wasi-proc-exit"));
}

#[test]
fn scan_wasm_bytes_detects_sock_accept() {
    let mut bytes = WASM_MAGIC.to_vec();
    bytes.extend_from_slice(b"\x00sock_accept\x00");
    let scanner = SkillScanner::default();
    let result = scanner.scan_wasm_bytes(&bytes);
    assert!(!result.passed);
    let finding = result
        .findings
        .iter()
        .find(|f| f.rule_id == "wasm-import-wasi-sock-accept")
        .unwrap();
    assert_eq!(finding.severity, Severity::Critical);
}

#[test]
fn scan_wasm_bytes_clean_wasm_passes() {
    let mut bytes = WASM_MAGIC.to_vec();
    bytes.extend_from_slice(b"\x01\x00\x00\x00nothing_dangerous_here");
    let scanner = SkillScanner::default();
    let result = scanner.scan_wasm_bytes(&bytes);
    assert!(result.passed);
    assert!(result.findings.is_empty());
}

#[test]
fn scan_bytes_text_detects_eval() {
    let content = b"function run() { eval('rm -rf /') }";
    let scanner = SkillScanner::default();
    let result = scanner.scan_bytes(content);
    assert!(!result.passed);
    assert!(result
        .findings
        .iter()
        .any(|f| f.rule_id == "text-eval-usage"));
}

#[test]
fn scan_bytes_text_detects_system() {
    let content = b"int main() { system(\"whoami\"); }";
    let scanner = SkillScanner::default();
    let result = scanner.scan_bytes(content);
    assert!(!result.passed);
    let finding = result
        .findings
        .iter()
        .find(|f| f.rule_id == "text-system-call")
        .unwrap();
    assert_eq!(finding.severity, Severity::Critical);
}

#[test]
fn scan_bytes_clean_text_passes() {
    let content = b"def hello():\n    print('world')\n";
    let scanner = SkillScanner::default();
    let result = scanner.scan_bytes(content);
    assert!(result.passed);
    assert!(result.findings.is_empty());
}

#[test]
fn scan_wasm_bytes_multiple_findings() {
    let mut bytes = WASM_MAGIC.to_vec();
    bytes.extend_from_slice(b"some code fd_write more code fd_read end");
    let scanner = SkillScanner::default();
    let result = scanner.scan_wasm_bytes(&bytes);
    assert!(result.passed);
    assert_eq!(result.findings.len(), 2);
    assert!(result
        .findings
        .iter()
        .all(|f| f.severity == Severity::Medium));
}

#[test]
fn scan_wasm_bytes_to_findings_produces_finding_type() {
    let mut bytes = WASM_MAGIC.to_vec();
    bytes.extend_from_slice(b"\x00env\x00exec\x00");
    let findings = rua_skills_scanner::scan_wasm_bytes_to_findings(&bytes);
    assert!(!findings.is_empty());
    let f = &findings[0];
    assert_eq!(f.rule_id, "wasm-import-env-exec");
    assert_eq!(f.severity, Severity::Critical);
    assert_eq!(f.analyzer.as_deref(), Some("wasm_byte_scanner"));
}
