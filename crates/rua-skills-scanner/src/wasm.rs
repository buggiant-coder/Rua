use crate::types::{Finding, Severity, ThreatCategory};

pub const WASM_MAGIC: [u8; 4] = [0x00, 0x61, 0x73, 0x6D];

struct DangerPattern {
    id: &'static str,
    severity: Severity,
    description: &'static str,
    bytes: &'static [u8],
}

const DANGER_PATTERNS: &[DangerPattern] = &[
    DangerPattern {
        id: "wasm-import-wasi-proc-exit",
        severity: Severity::High,
        description: "WASI proc_exit import detected — skill can terminate the host process",
        bytes: b"proc_exit",
    },
    DangerPattern {
        id: "wasm-import-wasi-fd-write",
        severity: Severity::Medium,
        description: "WASI fd_write import detected — skill can write to host file descriptors",
        bytes: b"fd_write",
    },
    DangerPattern {
        id: "wasm-import-wasi-fd-read",
        severity: Severity::Medium,
        description: "WASI fd_read import detected — skill can read host file descriptors",
        bytes: b"fd_read",
    },
    DangerPattern {
        id: "wasm-import-wasi-path-open",
        severity: Severity::High,
        description: "WASI path_open import detected — skill can access host filesystem",
        bytes: b"path_open",
    },
    DangerPattern {
        id: "wasm-import-wasi-sock-accept",
        severity: Severity::Critical,
        description: "WASI sock_accept import detected — skill can accept network connections",
        bytes: b"sock_accept",
    },
    DangerPattern {
        id: "wasm-import-wasi-sock-recv",
        severity: Severity::High,
        description: "WASI sock_recv import detected — skill can receive network data",
        bytes: b"sock_recv",
    },
    DangerPattern {
        id: "wasm-import-wasi-sock-send",
        severity: Severity::High,
        description: "WASI sock_send import detected — skill can send network data",
        bytes: b"sock_send",
    },
    DangerPattern {
        id: "wasm-import-env-exec",
        severity: Severity::Critical,
        description: "Environment exec import detected — skill can execute arbitrary commands",
        bytes: b"env\x00exec",
    },
    DangerPattern {
        id: "wasm-import-env-system",
        severity: Severity::Critical,
        description: "Environment system import detected — skill can run shell commands",
        bytes: b"env\x00system",
    },
];

const TEXT_PATTERNS: &[(&str, &str, Severity, &str)] = &[
    (
        "text-eval-usage",
        "eval(",
        Severity::High,
        "eval() usage detected — potential code injection vector",
    ),
    (
        "text-exec-usage",
        "exec(",
        Severity::High,
        "exec() usage detected — potential command execution",
    ),
    (
        "text-system-call",
        "system(",
        Severity::Critical,
        "system() call detected — arbitrary command execution",
    ),
    (
        "text-shell-injection",
        "$((",
        Severity::High,
        "Shell arithmetic expansion detected — potential shell injection",
    ),
];

#[derive(Debug, Clone)]
pub struct WasmScanResult {
    pub passed: bool,
    pub findings: Vec<WasmFinding>,
}

#[derive(Debug, Clone)]
pub struct WasmFinding {
    pub rule_id: String,
    pub severity: Severity,
    pub description: String,
    pub offset: Option<usize>,
}

pub fn scan_wasm_bytes(bytes: &[u8], block_severity: Severity) -> WasmScanResult {
    let mut findings = Vec::new();

    if bytes.len() >= 4 && bytes[..4] == WASM_MAGIC {
        scan_wasm_patterns(bytes, &mut findings);
    } else {
        scan_text_patterns(bytes, &mut findings);
    }

    let passed = !findings.iter().any(|f| f.severity >= block_severity);
    WasmScanResult { passed, findings }
}

pub fn scan_wasm_bytes_to_findings(bytes: &[u8]) -> Vec<Finding> {
    let mut raw = Vec::new();

    if bytes.len() >= 4 && bytes[..4] == WASM_MAGIC {
        scan_wasm_patterns(bytes, &mut raw);
    } else {
        scan_text_patterns(bytes, &mut raw);
    }

    raw.into_iter()
        .map(|f| Finding {
            id: format!("{}::wasm::{}", f.rule_id, f.offset.unwrap_or(0)),
            rule_id: f.rule_id,
            category: ThreatCategory::Malware,
            severity: f.severity,
            title: f.description.clone(),
            description: f.description,
            file_path: None,
            line_number: None,
            snippet: None,
            remediation: None,
            analyzer: Some("wasm_byte_scanner".to_string()),
            metadata: serde_json::Map::new(),
        })
        .collect()
}

fn scan_wasm_patterns(bytes: &[u8], findings: &mut Vec<WasmFinding>) {
    for pattern in DANGER_PATTERNS {
        if let Some(offset) = find_subsequence(bytes, pattern.bytes) {
            findings.push(WasmFinding {
                rule_id: pattern.id.to_string(),
                severity: pattern.severity,
                description: pattern.description.to_string(),
                offset: Some(offset),
            });
        }
    }
}

fn scan_text_patterns(bytes: &[u8], findings: &mut Vec<WasmFinding>) {
    let content = String::from_utf8_lossy(bytes);
    for (id, needle, severity, desc) in TEXT_PATTERNS {
        if let Some(pos) = content.find(needle) {
            findings.push(WasmFinding {
                rule_id: id.to_string(),
                severity: *severity,
                description: desc.to_string(),
                offset: Some(pos),
            });
        }
    }
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
