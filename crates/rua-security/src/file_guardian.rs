use std::path::{Component, Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};

use rua_config::dirs::Paths;
use rua_config::types::security::FileGuardConfig;

use crate::models::{Severity, ToolCallContext, ToolGuardFinding};
use crate::redaction::redact_path;

const DEFAULT_SENSITIVE_PATTERNS: &[&str] = &[
    "**/.env",
    "**/.env.*",
    "**/id_rsa",
    "**/id_ed25519",
    "**/id_ecdsa",
    "**/.ssh/authorized_keys",
    "**/.ssh/config",
    "**/credentials.json",
    "**/service-account*.json",
    "**/secrets.yaml",
    "**/secrets.yml",
    "**/.git-credentials",
    "**/.netrc",
    "**/.npmrc",
    "**/.pypirc",
    "**/kubeconfig",
    "**/.kube/config",
    "**/token.json",
    "**/keystore.jks",
    "**/*.pem",
    "**/*.key",
    "**/*.p12",
    "**/*.pfx",
];

const SYSTEM_SENSITIVE_PATHS: &[&str] = &[
    "/etc/passwd",
    "/etc/shadow",
    "/etc/sudoers",
    "/etc/ssl/private",
    "/proc/",
    "/sys/",
];

pub fn default_sensitive_patterns() -> &'static [&'static str] {
    DEFAULT_SENSITIVE_PATTERNS
}

pub fn system_sensitive_paths() -> &'static [&'static str] {
    SYSTEM_SENSITIVE_PATHS
}

const MAX_JSON_DEPTH: usize = 10;

pub struct FileGuardian {
    sensitive_globs: GlobSet,

    _sensitive_patterns: Vec<String>,

    secret_dir: Option<PathBuf>,

    disabled_patterns: Vec<String>,
}

impl FileGuardian {
    pub fn new(config: &FileGuardConfig) -> Self {
        Self::build(config, None)
    }

    pub fn with_paths(config: &FileGuardConfig, paths: &Paths) -> Self {
        Self::build(config, Some(paths.secret_dir.clone()))
    }

    fn build(config: &FileGuardConfig, secret_dir: Option<PathBuf>) -> Self {
        let disabled = &config.disabled_patterns;
        let mut builder = GlobSetBuilder::new();
        let mut patterns = Vec::new();

        for pattern in DEFAULT_SENSITIVE_PATTERNS {
            if disabled.iter().any(|d| d == pattern) {
                continue;
            }
            if let Ok(glob) = Glob::new(pattern) {
                builder.add(glob);
                patterns.push(pattern.to_string());
            }
        }

        for pattern in &config.sensitive_files {
            if disabled.iter().any(|d| d == pattern) {
                continue;
            }
            if let Ok(glob) = Glob::new(pattern) {
                builder.add(glob);
                patterns.push(pattern.clone());
            } else {
                tracing::warn!(pattern = %pattern, "invalid sensitive file glob pattern, skipping");
            }
        }

        let sensitive_globs = builder.build().unwrap_or_else(|_| {
            tracing::error!("failed to build sensitive file glob set, using empty set");
            GlobSetBuilder::new().build().unwrap()
        });

        Self {
            sensitive_globs,
            _sensitive_patterns: patterns,
            secret_dir,
            disabled_patterns: disabled.clone(),
        }
    }

    pub fn evaluate(&self, ctx: &ToolCallContext) -> Vec<ToolGuardFinding> {
        let mut findings = Vec::new();

        let mut paths = extract_paths(&ctx.tool_input, MAX_JSON_DEPTH);

        if is_bash_tool_name(&ctx.tool_name) {
            if let Some(cmd) = ctx.tool_input.get("command").and_then(|v| v.as_str()) {
                let redirect_paths = extract_bash_redirect_paths(cmd);
                paths.extend(redirect_paths);
            }
        }

        for path_str in &paths {
            if let Some(finding) = self.check_secret_dir_for_tool(path_str, &ctx.tool_name) {
                findings.push(finding);
            }

            if let Some(finding) =
                self.check_path_traversal_for_tool(path_str, &ctx.working_dir, &ctx.tool_name)
            {
                findings.push(finding);
            }

            if let Some(finding) = self.check_sensitive_file_for_tool(path_str, &ctx.tool_name) {
                findings.push(finding);
            }

            if let Some(finding) = self.check_system_path_for_tool(path_str, &ctx.tool_name) {
                findings.push(finding);
            }
        }

        findings
    }

    pub fn check_path_traversal(&self, path: &str, _working_dir: &str) -> Option<ToolGuardFinding> {
        self.check_path_traversal_for_tool(path, _working_dir, "read")
    }

    fn check_path_traversal_for_tool(
        &self,
        path: &str,
        _working_dir: &str,
        tool_name: &str,
    ) -> Option<ToolGuardFinding> {
        let normalized = normalize_path(path);

        if has_traversal(path) {
            let message = format!("Path traversal detected in path: {}", redact_path(path));
            return Some(ToolGuardFinding {
                rule_id: "FILE-TRAVERSAL".into(),
                category: "path_traversal".into(),
                severity: Severity::High,
                title: "Path traversal detected".into(),
                description: message.clone(),
                tool_name: tool_name.into(),
                guardian: "file_guardian".into(),
                message,
                remediation: "Use absolute paths or paths relative to the working directory without '..' components".into(),
                matched_pattern: Some("..".into()),
                matched_value: Some(redact_path(path)),
            });
        }

        if path.contains('\0') {
            return Some(ToolGuardFinding {
                rule_id: "FILE-NULLBYTE".into(),
                category: "path_traversal".into(),
                severity: Severity::Critical,
                title: "Null byte detected".into(),
                description: "Null byte detected in file path".into(),
                tool_name: tool_name.into(),
                guardian: "file_guardian".into(),
                message: "Null byte detected in file path".into(),
                remediation: "Remove null bytes from the file path".into(),
                matched_pattern: Some("\\0".into()),
                matched_value: Some(redact_path(path)),
            });
        }

        if path.contains("%2e%2e")
            || path.contains("%2E%2E")
            || path.contains("..%2f")
            || path.contains("..%2F")
        {
            return Some(ToolGuardFinding {
                rule_id: "FILE-ENCODED-TRAVERSAL".into(),
                category: "path_traversal".into(),
                severity: Severity::High,
                title: "URL-encoded path traversal".into(),
                description: "URL-encoded path traversal detected".into(),
                tool_name: tool_name.into(),
                guardian: "file_guardian".into(),
                message: "URL-encoded path traversal detected".into(),
                remediation: "Do not use URL encoding in file paths".into(),
                matched_pattern: Some("%2e%2e".into()),
                matched_value: Some(redact_path(path)),
            });
        }

        let _ = normalized;
        None
    }

    pub fn check_sensitive_file(&self, path: &str) -> Option<ToolGuardFinding> {
        self.check_sensitive_file_for_tool(path, "read")
    }

    fn check_sensitive_file_for_tool(
        &self,
        path: &str,
        tool_name: &str,
    ) -> Option<ToolGuardFinding> {
        let p = Path::new(path);
        if self.sensitive_globs.is_match(p) {
            let message = format!("Access to sensitive file: {}", redact_path(path));
            return Some(ToolGuardFinding {
                rule_id: "FILE-SENSITIVE".into(),
                category: "sensitive_file".into(),
                severity: Severity::High,
                title: "Sensitive file access".into(),
                description: message.clone(),
                tool_name: tool_name.into(),
                guardian: "file_guardian".into(),
                message,
                remediation: "Avoid accessing sensitive files; use environment variables or secrets managers instead".into(),
                matched_pattern: None,
                matched_value: Some(redact_path(path)),
            });
        }
        None
    }

    pub fn check_system_path(&self, path: &str) -> Option<ToolGuardFinding> {
        self.check_system_path_for_tool(path, "read")
    }

    fn check_system_path_for_tool(&self, path: &str, tool_name: &str) -> Option<ToolGuardFinding> {
        for sys_path in SYSTEM_SENSITIVE_PATHS {
            if self.disabled_patterns.iter().any(|d| d == sys_path) {
                continue;
            }
            if path.starts_with(sys_path) || path == sys_path.trim_end_matches('/') {
                let message = format!("Access to system-sensitive path: {}", redact_path(path));
                return Some(ToolGuardFinding {
                    rule_id: "FILE-SYSTEM".into(),
                    category: "sensitive_file".into(),
                    severity: Severity::High,
                    title: "System-sensitive path access".into(),
                    description: message.clone(),
                    tool_name: tool_name.into(),
                    guardian: "file_guardian".into(),
                    message,
                    remediation: "Avoid accessing system files directly".into(),
                    matched_pattern: Some(sys_path.to_string()),
                    matched_value: Some(redact_path(path)),
                });
            }
        }
        None
    }

    pub fn check_secret_dir(&self, path: &str) -> Option<ToolGuardFinding> {
        self.check_secret_dir_for_tool(path, "read")
    }

    fn check_secret_dir_for_tool(&self, path: &str, tool_name: &str) -> Option<ToolGuardFinding> {
        let secret = match &self.secret_dir {
            Some(d) => d,
            None => return None,
        };
        let secret_str = secret.to_string_lossy();
        let normalized = normalize_path(path);
        let normalized_secret = normalize_path(&secret_str);
        if normalized.starts_with(&normalized_secret) {
            let message = format!(
                "Access to secret directory is denied: {}",
                redact_path(path)
            );
            return Some(ToolGuardFinding {
                rule_id: "FILE-SECRET-DIR".into(),
                category: "secret_dir".into(),
                severity: Severity::Critical,
                title: "Secret directory access".into(),
                description: message.clone(),
                tool_name: tool_name.into(),
                guardian: "file_guardian".into(),
                message,
                remediation: "Do not access the secret directory directly".into(),
                matched_pattern: Some(normalized_secret),
                matched_value: Some(redact_path(path)),
            });
        }
        None
    }
}

fn is_bash_tool_name(tool_name: &str) -> bool {
    tool_name == "bash"
}

pub(crate) fn normalize_path(path: &str) -> String {
    let p = PathBuf::from(path);
    let mut components = Vec::new();
    let mut has_root = false;

    for component in p.components() {
        match component {
            Component::RootDir => {
                has_root = true;
                components.clear();
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !components.is_empty() {
                    components.pop();
                }
            }
            Component::Normal(c) => {
                components.push(c.to_string_lossy().to_string());
            }
            Component::Prefix(p) => {
                components.push(p.as_os_str().to_string_lossy().to_string());
            }
        }
    }

    if has_root {
        format!("/{}", components.join("/"))
    } else if components.is_empty() {
        ".".into()
    } else {
        components.join("/")
    }
}

pub fn has_traversal(path: &str) -> bool {
    let p = Path::new(path);
    for component in p.components() {
        if matches!(component, Component::ParentDir) {
            return true;
        }
    }

    if path.contains("..\\") {
        return true;
    }
    false
}

pub fn extract_paths(value: &serde_json::Value, max_depth: usize) -> Vec<String> {
    let mut paths = Vec::new();
    extract_paths_recursive(value, max_depth, 0, None, &mut paths);
    paths
}

fn extract_paths_recursive(
    value: &serde_json::Value,
    max_depth: usize,
    current_depth: usize,
    key_hint: Option<&str>,
    paths: &mut Vec<String>,
) {
    if current_depth >= max_depth {
        return;
    }

    match value {
        serde_json::Value::String(s) if looks_like_path(s) || key_hint_is_path(key_hint) => {
            paths.push(s.clone());
        }
        serde_json::Value::Object(map) => {
            let mut entries = map.iter().collect::<Vec<_>>();
            entries.sort_by_key(|(key, _)| *key);
            for (key, v) in entries {
                extract_paths_recursive(v, max_depth, current_depth + 1, Some(key), paths);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                extract_paths_recursive(v, max_depth, current_depth + 1, key_hint, paths);
            }
        }
        _ => {}
    }
}

fn key_hint_is_path(key_hint: Option<&str>) -> bool {
    let Some(key) = key_hint else {
        return false;
    };
    let key = key.to_ascii_lowercase();
    matches!(
        key.as_str(),
        "path" | "file" | "file_path" | "filepath" | "target_path" | "source_path"
    ) || key.ends_with("_path")
}

pub fn looks_like_path(s: &str) -> bool {
    if s.is_empty() || s.len() > 4096 {
        return false;
    }
    if looks_like_url_or_uri(s) || looks_like_mime_type(s) {
        return false;
    }

    s.starts_with('/')
        || s.starts_with("./")
        || s.starts_with("../")
        || s.starts_with("~/")
        || (s.contains('/') && !s.starts_with("http"))
}

fn looks_like_url_or_uri(s: &str) -> bool {
    if s.starts_with("data:") {
        return true;
    }
    let Some((scheme, _)) = s.split_once("://") else {
        return false;
    };
    !scheme.is_empty()
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

fn looks_like_mime_type(s: &str) -> bool {
    let Some((ty, subtype)) = s.split_once('/') else {
        return false;
    };
    if ty.is_empty() || subtype.is_empty() || subtype.contains('/') {
        return false;
    }
    if !matches!(
        ty,
        "application"
            | "audio"
            | "font"
            | "image"
            | "message"
            | "model"
            | "multipart"
            | "text"
            | "video"
    ) {
        return false;
    }
    ty.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && subtype
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

pub fn extract_bash_redirect_paths(command: &str) -> Vec<String> {
    let tokens = match shlex::split(command) {
        Some(t) => t,
        None => return Vec::new(),
    };

    let mut paths = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let token = &tokens[i];
        if token.starts_with("<<") {
            i += 1;
            continue;
        }

        if matches!(
            token.as_str(),
            ">" | ">>" | "&>" | "&>>" | "2>" | "2>>" | "<"
        ) && i + 1 < tokens.len()
        {
            paths.push(tokens[i + 1].clone());
            i += 2;
            continue;
        }

        for op in &["&>>", "2>>", ">>", "&>", "2>", ">", "<"] {
            if let Some(rest) = token.strip_prefix(op) {
                if !rest.is_empty() {
                    paths.push(rest.to_string());
                    break;
                }
            }
        }
        i += 1;
    }
    paths
}
