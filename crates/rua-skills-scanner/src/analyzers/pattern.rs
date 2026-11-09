use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use fancy_regex::Regex;
use serde::Deserialize;
use tracing::warn;

use crate::policy::ScanPolicy;
use crate::types::{Finding, Severity, SkillFile, ThreatCategory};

use super::Analyzer;

#[derive(Debug, Deserialize)]
pub struct PatternRule {
    pub id: String,
    pub category: ThreatCategory,
    pub severity: Severity,
    pub patterns: Vec<String>,
    #[serde(default)]
    pub exclude_patterns: Vec<String>,

    #[serde(default)]
    pub file_types: Vec<String>,
    pub description: String,
    #[serde(default)]
    pub remediation: Option<String>,

    #[serde(skip)]
    pub compiled_patterns: Vec<Regex>,
    #[serde(skip)]
    pub compiled_exclude_patterns: Vec<Regex>,
}

impl PatternRule {
    pub fn compile(&mut self) {
        self.compiled_patterns = compile_many(&self.id, &self.patterns, "pattern");
        self.compiled_exclude_patterns =
            compile_many(&self.id, &self.exclude_patterns, "exclude_pattern");
    }

    pub fn matches_file_type(&self, file_type: &str) -> bool {
        self.file_types.is_empty() || self.file_types.iter().any(|t| t == file_type)
    }
}

fn compile_many(rule_id: &str, raw: &[String], kind: &str) -> Vec<Regex> {
    let mut out = Vec::with_capacity(raw.len());
    for pat in raw {
        match Regex::new(pat) {
            Ok(rx) => out.push(rx),
            Err(err) => {
                warn!(
                    rule = %rule_id,
                    kind,
                    pattern = %pat,
                    error = %err,
                    "failed to compile regex, skipping",
                );
            }
        }
    }
    out
}

pub fn load_rules_from_dir(rules_dir: &Path) -> std::io::Result<Vec<PatternRule>> {
    let mut yaml_files: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(rules_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("yaml") {
            yaml_files.push(path);
        }
    }
    yaml_files.sort();

    let mut rules: Vec<PatternRule> = Vec::new();
    for yaml in yaml_files {
        let raw = match std::fs::read_to_string(&yaml) {
            Ok(s) => s,
            Err(err) => {
                warn!(file = %yaml.display(), error = %err, "failed to read rule file");
                continue;
            }
        };
        match serde_yaml::from_str::<Vec<PatternRule>>(&raw) {
            Ok(parsed) => {
                for mut rule in parsed {
                    rule.compile();
                    if rule.compiled_patterns.is_empty() {
                        warn!(rule = %rule.id, file = %yaml.display(), "all patterns failed to compile, skipping rule");
                        continue;
                    }
                    rules.push(rule);
                }
            }
            Err(err) => {
                warn!(file = %yaml.display(), error = %err, "failed to parse YAML rule file");
            }
        }
    }
    Ok(rules)
}

pub fn default_rules_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("rules")
}

pub struct PatternAnalyzer {
    rules: Vec<PatternRule>,
    policy: Arc<ScanPolicy>,
}

impl PatternAnalyzer {
    pub fn new(policy: Arc<ScanPolicy>) -> Self {
        Self::with_rules_dir(policy, &default_rules_dir())
    }

    pub fn with_rules_dir(policy: Arc<ScanPolicy>, rules_dir: &Path) -> Self {
        let rules = load_rules_from_dir(rules_dir).unwrap_or_else(|err| {
            warn!(dir = %rules_dir.display(), error = %err, "failed to enumerate rules directory");
            Vec::new()
        });
        Self { rules, policy }
    }

    pub fn from_rules(policy: Arc<ScanPolicy>, rules: Vec<PatternRule>) -> Self {
        Self { rules, policy }
    }

    pub fn rules(&self) -> &[PatternRule] {
        &self.rules
    }
}

#[async_trait]
impl Analyzer for PatternAnalyzer {
    fn name(&self) -> &str {
        "pattern"
    }

    async fn analyze(&self, files: &mut [SkillFile]) -> Vec<Finding> {
        let mut findings: Vec<Finding> = Vec::new();
        for sf in files.iter_mut() {
            let content = sf.read_content().to_string();
            if content.is_empty() {
                continue;
            }
            let is_doc = self.policy.is_doc_path(&sf.relative_path);
            for rule in &self.rules {
                if self.policy.is_rule_disabled(&rule.id) {
                    continue;
                }
                if !rule.matches_file_type(&sf.file_type) {
                    continue;
                }
                if is_doc && self.policy.rule_scoping.skip_in_docs.contains(&rule.id) {
                    continue;
                }
                if self.policy.rule_scoping.code_only.contains(&rule.id)
                    && !matches!(
                        sf.file_type.as_str(),
                        "python" | "bash" | "javascript" | "typescript"
                    )
                {
                    continue;
                }
                scan_rule(
                    rule,
                    &content,
                    &sf.relative_path,
                    &self.policy,
                    &mut findings,
                );
            }
        }

        findings.retain(|f| !is_known_test_credential(f, &self.policy));
        if self.policy.rule_scoping.dedupe_duplicate_findings {
            dedupe_findings(&mut findings);
        }
        findings
    }
}

fn scan_rule(
    rule: &PatternRule,
    content: &str,
    rel_path: &str,
    policy: &ScanPolicy,
    out: &mut Vec<Finding>,
) {
    let lines: Vec<&str> = content.split('\n').collect();
    for (idx, line) in lines.iter().enumerate() {
        let line_no = (idx as u32) + 1;
        if rule
            .compiled_exclude_patterns
            .iter()
            .any(|rx| rx.is_match(line).unwrap_or(false))
        {
            continue;
        }
        for rx in &rule.compiled_patterns {
            if let Ok(Some(m)) = rx.find(line) {
                push_finding(rule, rel_path, line_no, line, rx, m.as_str(), policy, out);
            }
        }
    }

    for rx in &rule.compiled_patterns {
        if !pattern_is_multiline(rx.as_str()) {
            continue;
        }
        for m in rx.find_iter(content).flatten() {
            let matched = m.as_str();
            if rule
                .compiled_exclude_patterns
                .iter()
                .any(|ex| ex.is_match(matched).unwrap_or(false))
            {
                continue;
            }
            let start_line = content[..m.start()].matches('\n').count() as u32 + 1;
            let snippet_line = lines
                .get(start_line as usize - 1)
                .copied()
                .unwrap_or_default();
            let truncated: String = matched.chars().take(200).collect();
            push_finding(
                rule,
                rel_path,
                start_line,
                snippet_line,
                rx,
                truncated.as_str(),
                policy,
                out,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn push_finding(
    rule: &PatternRule,
    rel_path: &str,
    line_no: u32,
    line: &str,
    rx: &Regex,
    matched_text: &str,
    policy: &ScanPolicy,
    out: &mut Vec<Finding>,
) {
    let severity = policy
        .get_severity_override(&rule.id)
        .unwrap_or(rule.severity);
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        "matched_pattern".into(),
        serde_json::Value::String(rx.as_str().to_string()),
    );
    metadata.insert(
        "matched_text".into(),
        serde_json::Value::String(matched_text.to_string()),
    );
    out.push(Finding {
        id: format!("{}:{}:{}", rule.id, rel_path, line_no),
        rule_id: rule.id.clone(),
        category: rule.category,
        severity,
        title: rule.description.clone(),
        description: rule.description.clone(),
        file_path: Some(rel_path.to_string()),
        line_number: Some(line_no),
        snippet: Some(line.trim().to_string()),
        remediation: rule.remediation.clone(),
        analyzer: Some("pattern".into()),
        metadata,
    });
}

fn pattern_is_multiline(pattern: &str) -> bool {
    let stripped = strip_char_classes(pattern);
    stripped.contains("\\n")
}

fn strip_char_classes(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut in_class = false;
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(next) = chars.next() {
                    if !in_class {
                        out.push(c);
                        out.push(next);
                    }
                }
            }
            '[' if !in_class => {
                in_class = true;
            }
            ']' if in_class => {
                in_class = false;
            }
            _ => {
                if !in_class {
                    out.push(c);
                }
            }
        }
    }
    out
}

fn is_known_test_credential(f: &Finding, policy: &ScanPolicy) -> bool {
    if !matches!(f.category, ThreatCategory::HardcodedSecrets) {
        return false;
    }
    let snippet = f.snippet.as_deref().unwrap_or("").to_lowercase();
    for cred in &policy.credentials.known_test_values {
        if snippet.contains(&cred.to_lowercase()) {
            return true;
        }
    }
    for marker in &policy.credentials.placeholder_markers {
        if snippet.contains(&marker.to_lowercase()) {
            return true;
        }
    }
    false
}

fn dedupe_findings(findings: &mut Vec<Finding>) {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    findings.retain(|f| {
        let key = format!(
            "{}:{}:{}",
            f.rule_id,
            f.file_path.as_deref().unwrap_or(""),
            f.line_number.unwrap_or(0),
        );
        seen.insert(key)
    });
}
