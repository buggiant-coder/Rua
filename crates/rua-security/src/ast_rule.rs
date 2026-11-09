use serde::{Deserialize, Serialize};

use crate::bash_analyzer::BashAnalysis;
use crate::file_guardian::normalize_path;
use crate::models::{Severity, ToolGuardFinding};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AstAction {
    Allow,
    NeedsApproval,
    Deny,
    Audit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StructuralFlag {
    PipeToShell,
    CommandSubstitution,
    SensitiveRedirect,
    ForkBomb,
    Base64ToShell,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArgMatcher {
    pub command: Option<String>,
    pub flag: Option<String>,
    pub value_contains: Option<String>,
    pub value_in: Vec<String>,
    #[serde(default)]
    pub exclude_args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathMatcher {
    pub contains: Vec<String>,
    pub prefix: Vec<String>,
    pub glob: Option<String>,
    pub on_redirect: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LexicalCheck {
    ControlChars,
    UnicodeWhitespace,
    IfsReference,
    AnsiCQuoting,
    BackslashWhitespace,
    BackslashOperator,
    EmbeddedNewline,
    CommentQuoteDesync,
    QuotedNewline,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AstRule {
    pub id: String,
    pub category: String,
    pub severity: Severity,
    pub action: AstAction,
    pub command_set: Vec<String>,
    pub structural: Vec<StructuralFlag>,
    pub arg_matchers: Vec<ArgMatcher>,
    pub path_matchers: Vec<PathMatcher>,
    pub lexical: Vec<LexicalCheck>,
}

pub struct AstRuleEvaluation {
    pub findings: Vec<ToolGuardFinding>,
    pub has_explicit_deny: bool,
}

impl Default for AstRule {
    fn default() -> Self {
        Self {
            id: String::new(),
            category: String::new(),
            severity: Severity::Medium,
            action: AstAction::NeedsApproval,
            command_set: Vec::new(),
            structural: Vec::new(),
            arg_matchers: Vec::new(),
            path_matchers: Vec::new(),
            lexical: Vec::new(),
        }
    }
}

const SENSITIVE_REDIRECT_PREFIXES: &[&str] = &[
    "/etc/passwd",
    "/etc/shadow",
    "/etc/sudoers",
    "/dev/sda",
    "/dev/vda",
    "/dev/nvme",
];

const FIND_DANGEROUS_FLAGS: &[&str] = &["-delete", "-exec", "-execdir"];

pub fn evaluate_ast_rules(rules: &[AstRule], analysis: &BashAnalysis) -> Vec<ToolGuardFinding> {
    evaluate_ast_rules_full(rules, analysis, "", "")
}

pub fn evaluate_ast_rules_full(
    rules: &[AstRule],
    analysis: &BashAnalysis,
    raw_command: &str,
    working_dir: &str,
) -> Vec<ToolGuardFinding> {
    evaluate_ast_rules_full_with_decision(rules, analysis, raw_command, working_dir).findings
}

pub fn evaluate_ast_rules_full_with_decision(
    rules: &[AstRule],
    analysis: &BashAnalysis,
    raw_command: &str,
    working_dir: &str,
) -> AstRuleEvaluation {
    let mut findings = Vec::new();

    for rule in rules {
        if rule.action == AstAction::Allow {
            continue;
        }

        if !rule.command_set.is_empty() {
            for cmd in &analysis.commands {
                let matches = rule
                    .command_set
                    .iter()
                    .any(|s| s == &cmd.name || cmd.name.starts_with(&format!("{}.", s)));
                if matches {
                    findings.push(make_finding(
                        rule,
                        &format!("Dangerous command detected: {}", cmd.name),
                        &cmd.name,
                    ));
                }
            }
        }

        for flag in &rule.structural {
            match flag {
                StructuralFlag::PipeToShell => {
                    if analysis.has_pipe_to_shell {
                        findings.push(make_finding(
                            rule,
                            "Pipeline into shell interpreter detected",
                            "pipe_to_shell",
                        ));
                    }
                }
                StructuralFlag::CommandSubstitution => {
                    if analysis.has_command_substitution {
                        findings.push(make_finding(
                            rule,
                            "Command substitution detected",
                            "command_substitution",
                        ));
                    }
                }
                StructuralFlag::SensitiveRedirect => {
                    for redir in &analysis.redirects {
                        if SENSITIVE_REDIRECT_PREFIXES
                            .iter()
                            .any(|p| redir.target.starts_with(p))
                        {
                            findings.push(make_finding(
                                rule,
                                &format!("Redirect to sensitive target: {}", redir.target),
                                &redir.target,
                            ));
                        }
                    }
                }
                StructuralFlag::ForkBomb => {
                    if analysis.has_fork_bomb {
                        findings.push(make_finding(
                            rule,
                            "Fork bomb pattern detected",
                            "fork_bomb",
                        ));
                    }
                }
                StructuralFlag::Base64ToShell => {
                    if analysis.has_base64_to_shell {
                        findings.push(make_finding(
                            rule,
                            "Base64 decode piped to shell interpreter",
                            "base64_to_shell",
                        ));
                    }
                }
            }
        }

        for matcher in &rule.arg_matchers {
            for cmd in &analysis.commands {
                if let Some(ref target_cmd) = matcher.command {
                    if cmd.name != *target_cmd {
                        continue;
                    }
                }

                if let Some(ref flag) = matcher.flag {
                    let has_flag = cmd.args.iter().any(|a| {
                        a == flag
                            || a.starts_with(&format!("{flag}="))
                            || (flag.len() == 2
                                && flag.starts_with('-')
                                && a.starts_with('-')
                                && !a.starts_with("--")
                                && a.contains(flag.chars().nth(1).unwrap()))
                    });
                    if !has_flag {
                        continue;
                    }
                }

                if let Some(ref val_contains) = matcher.value_contains {
                    let has_val = cmd.args.iter().any(|a| a.contains(val_contains.as_str()));
                    if !has_val {
                        continue;
                    }
                }

                if !matcher.value_in.is_empty() {
                    let has_val = cmd.args.iter().any(|a| matcher.value_in.contains(a));
                    if !has_val {
                        continue;
                    }
                }

                if !matcher.exclude_args.is_empty() {
                    let non_flag_args: Vec<&String> =
                        cmd.args.iter().filter(|a| !a.starts_with('-')).collect();
                    if !non_flag_args.is_empty()
                        && non_flag_args
                            .iter()
                            .all(|a| matcher.exclude_args.contains(a))
                    {
                        continue;
                    }
                }

                let matched_desc = matcher.command.as_deref().unwrap_or(&cmd.name);
                findings.push(make_finding(
                    rule,
                    &format!("Argument pattern matched on: {}", matched_desc),
                    matched_desc,
                ));
                break;
            }
        }

        for matcher in &rule.path_matchers {
            let paths_to_check: Vec<&str> = if matcher.on_redirect {
                analysis
                    .redirects
                    .iter()
                    .map(|r| r.target.as_str())
                    .collect()
            } else {
                analysis
                    .commands
                    .iter()
                    .flat_map(|c| c.path_args.iter().chain(c.args.iter()))
                    .map(|s| s.as_str())
                    .collect()
            };

            for path in &paths_to_check {
                let matched = (!matcher.contains.is_empty()
                    && matcher.contains.iter().any(|c| path.contains(c.as_str())))
                    || (!matcher.prefix.is_empty()
                        && matcher.prefix.iter().any(|p| path.starts_with(p.as_str())))
                    || matcher.glob.as_ref().is_some_and(|g| glob_match(g, path));

                if matched {
                    findings.push(make_finding(
                        rule,
                        &format!("Path pattern matched: {}", path),
                        path,
                    ));
                    break;
                }
            }
        }

        for check in &rule.lexical {
            match check {
                LexicalCheck::ControlChars => {
                    if raw_command.bytes().any(is_control_char) {
                        findings.push(make_finding(
                            rule,
                            "Command contains non-printable control characters",
                            "control_chars",
                        ));
                    }
                }
                LexicalCheck::UnicodeWhitespace => {
                    if raw_command.chars().any(is_unicode_whitespace) {
                        findings.push(make_finding(
                            rule,
                            "Command contains Unicode whitespace characters",
                            "unicode_whitespace",
                        ));
                    }
                }
                LexicalCheck::IfsReference => {
                    if has_ifs_reference(raw_command) {
                        findings.push(make_finding(
                            rule,
                            "Command references $IFS variable",
                            "ifs_reference",
                        ));
                    }
                }
                LexicalCheck::AnsiCQuoting => {
                    if has_ansi_c_quoting(raw_command) {
                        findings.push(make_finding(
                            rule,
                            "Command contains ANSI-C or locale quoting ($' or $\") which can hide characters",
                            "ansi_c_quoting",
                        ));
                    }
                }
                LexicalCheck::BackslashWhitespace => {
                    if has_backslash_whitespace(raw_command) {
                        findings.push(make_finding(
                            rule,
                            "Command contains backslash-escaped whitespace that could alter command parsing",
                            "backslash_whitespace",
                        ));
                    }
                }
                LexicalCheck::BackslashOperator => {
                    if has_backslash_operator(raw_command) {
                        findings.push(make_finding(
                            rule,
                            "Command contains backslash before shell operator which can hide command structure",
                            "backslash_operator",
                        ));
                    }
                }
                LexicalCheck::EmbeddedNewline => {
                    if has_embedded_newline(raw_command) {
                        findings.push(make_finding(
                            rule,
                            "Command contains embedded newlines that could separate hidden commands",
                            "embedded_newline",
                        ));
                    }
                }
                LexicalCheck::CommentQuoteDesync => {
                    if has_comment_quote_desync(raw_command) {
                        findings.push(make_finding(
                            rule,
                            "Command contains quote characters inside a # comment which can desync quote tracking",
                            "comment_quote_desync",
                        ));
                    }
                }
                LexicalCheck::QuotedNewline => {
                    if has_quoted_newline(raw_command) {
                        findings.push(make_finding(
                            rule,
                            "Command contains quoted newline followed by #-prefixed line, which can hide arguments",
                            "quoted_newline",
                        ));
                    }
                }
            }
        }
    }

    for cmd in &analysis.commands {
        if cmd.name == "find" {
            if let Some(flag) = cmd
                .args
                .iter()
                .find(|a| FIND_DANGEROUS_FLAGS.contains(&a.as_str()))
            {
                findings.push(ToolGuardFinding {
                    rule_id: "AST_DANGEROUS_FIND".into(),
                    category: "ast_analysis".into(),
                    severity: Severity::High,
                    title: "Dangerous find flag".into(),
                    description: format!("find with dangerous flag: {}", flag),
                    tool_name: "bash".into(),
                    guardian: "ast_rule_guardian".into(),
                    message: format!("find with dangerous flag: {}", flag),
                    remediation: "Confirm with user before proceeding".into(),
                    matched_pattern: Some("ast:AST_DANGEROUS_FIND".into()),
                    matched_value: Some(flag.clone()),
                });
            }
        }
    }

    if !working_dir.is_empty() {
        let workspace_norm = normalize_path(working_dir);
        for cmd in &analysis.commands {
            if cmd.name == "rm" || cmd.name == "del" || cmd.name == "Remove-Item" {
                for path_arg in &cmd.path_args {
                    if !path_arg.starts_with('/') {
                        continue;
                    }
                    let norm = normalize_path(path_arg);
                    if !norm.starts_with(&workspace_norm) {
                        findings.push(ToolGuardFinding {
                            rule_id: "TOOL_CMD_DANGEROUS_RM".into(),
                            category: "command_injection".into(),
                            severity: Severity::Critical,
                            title: "Dangerous rm target".into(),
                            description: format!(
                                "rm target '{}' is outside workspace boundary",
                                path_arg
                            ),
                            tool_name: "bash".into(),
                            guardian: "ast_rule_guardian".into(),
                            message: format!(
                                "rm target '{}' is outside workspace boundary",
                                path_arg
                            ),
                            remediation: "Confirm with user before proceeding".into(),
                            matched_pattern: Some("ast:rm_workspace_boundary".into()),
                            matched_value: Some(path_arg.clone()),
                        });
                    }
                }
            }
        }
    }

    let has_explicit_deny = findings.iter().any(|finding| {
        rules
            .iter()
            .any(|rule| rule.id == finding.rule_id && rule.action == AstAction::Deny)
    });

    AstRuleEvaluation {
        findings,
        has_explicit_deny,
    }
}

fn make_finding(rule: &AstRule, message: &str, matched: &str) -> ToolGuardFinding {
    let severity = match rule.action {
        AstAction::Deny => Severity::Critical,
        AstAction::NeedsApproval => rule.severity,
        AstAction::Audit => Severity::Info,
        AstAction::Allow => unreachable!(),
    };

    let category = if rule.category.is_empty() {
        "ast_analysis".to_string()
    } else {
        rule.category.clone()
    };

    ToolGuardFinding {
        rule_id: rule.id.clone(),
        category,
        severity,
        title: rule.id.clone(),
        description: message.to_string(),
        tool_name: "bash".into(),
        guardian: "ast_rule_guardian".into(),
        message: message.to_string(),
        remediation: "Confirm with user before proceeding".into(),
        matched_pattern: Some(format!("ast:{}", rule.id)),
        matched_value: Some(matched.to_string()),
    }
}

fn is_control_char(b: u8) -> bool {
    matches!(b, 0x00..=0x08 | 0x0b | 0x0c | 0x0e..=0x1f | 0x7f)
}

fn is_unicode_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{00a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

fn has_ifs_reference(s: &str) -> bool {
    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i < len {
        if bytes[i] == b'$' {
            if i + 1 < len && bytes[i + 1] == b'{' {
                let brace_start = i + 2;
                if let Some(brace_end) = s[brace_start..].find('}') {
                    let inner = &s[brace_start..brace_start + brace_end];
                    if inner.contains("IFS") {
                        return true;
                    }
                }
            }

            if i + 4 <= len && &s[i..i + 4] == "$IFS" {
                if i + 4 >= len {
                    return true;
                }
                let next = bytes[i + 4];
                if !next.is_ascii_alphanumeric() && next != b'_' {
                    return true;
                }
            }
        }
        i += 1;
    }
    false
}

struct QuoteState {
    in_single: bool,
    in_double: bool,
    escaped: bool,
}

impl QuoteState {
    fn new() -> Self {
        Self {
            in_single: false,
            in_double: false,
            escaped: false,
        }
    }

    fn in_any_quote(&self) -> bool {
        self.in_single || self.in_double
    }

    fn feed(&mut self, ch: char) {
        if self.escaped {
            self.escaped = false;
            return;
        }
        if ch == '\\' && !self.in_single {
            self.escaped = true;
            return;
        }
        if ch == '\'' && !self.in_double {
            self.in_single = !self.in_single;
            return;
        }
        if ch == '"' && !self.in_single {
            self.in_double = !self.in_double;
        }
    }
}

fn has_ansi_c_quoting(s: &str) -> bool {
    let bytes = s.as_bytes();
    for i in 0..bytes.len().saturating_sub(1) {
        if bytes[i] == b'$' && (bytes[i + 1] == b'\'' || bytes[i + 1] == b'"') {
            return true;
        }
    }
    false
}

fn has_backslash_whitespace(s: &str) -> bool {
    let mut state = QuoteState::new();
    for ch in s.chars() {
        if state.escaped {
            if !state.in_double && (ch == ' ' || ch == '\t') {
                return true;
            }
            state.feed(ch);
            continue;
        }
        state.feed(ch);
    }
    false
}

const SHELL_OPERATORS: &[char] = &[';', '|', '&', '<', '>'];

fn has_backslash_operator(s: &str) -> bool {
    let chars: Vec<char> = s.chars().collect();
    let mut state = QuoteState::new();
    for (i, &ch) in chars.iter().enumerate() {
        if state.escaped {
            if !state.in_double && SHELL_OPERATORS.contains(&ch) {
                if ch == ';' && looks_like_find_exec_terminator(s, i) {
                    state.feed(ch);
                    continue;
                }
                return true;
            }
            state.feed(ch);
            continue;
        }
        state.feed(ch);
    }
    false
}

fn looks_like_find_exec_terminator(s: &str, backslash_semicolon_pos: usize) -> bool {
    let prefix = &s[..=backslash_semicolon_pos];
    prefix.contains("-exec") && prefix.contains("{}")
}

fn has_embedded_newline(s: &str) -> bool {
    if looks_like_heredoc(s) {
        return false;
    }

    let mut state = QuoteState::new();
    for ch in s.chars() {
        if state.escaped {
            state.feed(ch);
            continue;
        }
        state.feed(ch);
        if ch == '\r' && !state.in_double {
            return true;
        }
    }

    let mut state = QuoteState::new();
    let chars: Vec<char> = s.chars().collect();
    for (i, &ch) in chars.iter().enumerate() {
        if state.escaped {
            state.feed(ch);
            continue;
        }
        state.feed(ch);
        if (ch == '\n' || ch == '\r') && !state.in_any_quote() {
            let rest: String = chars[i + 1..].iter().collect();
            if !rest.trim().is_empty() {
                return true;
            }
        }
    }
    false
}

fn looks_like_heredoc(s: &str) -> bool {
    let lines: Vec<&str> = s.lines().collect();
    if lines.len() < 2 {
        return false;
    }
    for (i, line) in lines.iter().enumerate() {
        if let Some(pos) = line.find("<<") {
            let after = line[pos + 2..].trim_start_matches('-').trim();
            let delim = after.trim_matches(|c: char| c == '\'' || c == '"');
            if !delim.is_empty() && delim.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                for next_line in &lines[i + 1..] {
                    if next_line.trim() == delim {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn has_comment_quote_desync(s: &str) -> bool {
    if !s.contains('#') {
        return false;
    }
    let mut state = QuoteState::new();
    let chars: Vec<char> = s.chars().collect();
    for (i, &ch) in chars.iter().enumerate() {
        if state.escaped {
            state.feed(ch);
            continue;
        }
        state.feed(ch);
        if ch == '#' && !state.in_any_quote() {
            let line_end = chars[i + 1..].iter().position(|&c| c == '\n');
            let comment: String = if let Some(end) = line_end {
                chars[i + 1..i + 1 + end].iter().collect()
            } else {
                chars[i + 1..].iter().collect()
            };
            if comment.contains('\'') || comment.contains('"') {
                return true;
            }
            if line_end.is_none() {
                break;
            }
        }
    }
    false
}

fn has_quoted_newline(s: &str) -> bool {
    if !s.contains('\n') || !s.contains('#') {
        return false;
    }
    let mut state = QuoteState::new();
    let chars: Vec<char> = s.chars().collect();
    for (i, &ch) in chars.iter().enumerate() {
        if state.escaped {
            state.feed(ch);
            continue;
        }
        state.feed(ch);
        if ch == '\n' && state.in_any_quote() {
            let line_start = i + 1;
            let next_nl = chars[line_start..].iter().position(|&c| c == '\n');
            let line_end = if let Some(end) = next_nl {
                line_start + end
            } else {
                chars.len()
            };
            let next_line: String = chars[line_start..line_end].iter().collect();
            if next_line.trim().starts_with('#') {
                return true;
            }
        }
    }
    false
}

fn glob_match(pattern: &str, path: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return path == pattern;
    }
    if !path.starts_with(parts[0]) {
        return false;
    }
    let mut remaining = &path[parts[0].len()..];
    for part in &parts[1..] {
        if part.is_empty() {
            continue;
        }
        match remaining.find(part) {
            Some(pos) => remaining = &remaining[pos + part.len()..],
            None => return false,
        }
    }
    true
}
