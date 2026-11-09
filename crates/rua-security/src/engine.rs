use rua_config::dirs::Paths;
use rua_config::types::security::{SecurityConfig, ToolGuardRuleConfig};

use crate::ast_rule::{
    evaluate_ast_rules_full_with_decision, ArgMatcher, AstAction, AstRule, LexicalCheck,
    PathMatcher, StructuralFlag,
};
use crate::audit::SecurityAuditEvent;
use crate::bash_analyzer::BashAnalyzer;
use crate::builtin_rules::builtin_structured_rules;
use crate::file_guardian::FileGuardian;
use crate::models::{GuardDecision, Severity, ToolCallContext, ToolGuardFinding, ToolGuardResult};

pub struct ToolGuardEngine {
    config: SecurityConfig,
    file_guardian: FileGuardian,
    ast_rules: Vec<AstRule>,
    custom_ast_rules: Vec<CustomAstRule>,
    bash_analyzer: BashAnalyzer,

    enabled: bool,

    file_guard_enabled: bool,
}

struct CustomAstRule {
    tools: Vec<String>,
    rule: AstRule,
}

impl ToolGuardEngine {
    pub fn new(config: &SecurityConfig, paths: Option<&Paths>) -> Self {
        let file_guardian = match paths {
            Some(p) => FileGuardian::with_paths(&config.file_guard, p),
            None => FileGuardian::new(&config.file_guard),
        };

        let ast_rules: Vec<AstRule> = builtin_structured_rules()
            .into_iter()
            .filter(|r| !config.tool_guard.disabled_rules.contains(&r.id))
            .collect();

        let custom_ast_rules = config
            .tool_guard
            .custom_rules
            .iter()
            .filter(|custom| !config.tool_guard.disabled_rules.contains(&custom.id))
            .map(|custom| CustomAstRule {
                tools: custom.tools.clone(),
                rule: custom_rule_to_ast(custom),
            })
            .collect();

        Self {
            config: config.clone(),
            file_guardian,
            ast_rules,
            custom_ast_rules,
            bash_analyzer: BashAnalyzer::new(),
            enabled: config.tool_guard.enabled,
            file_guard_enabled: config.file_guard.enabled,
        }
    }

    pub fn evaluate(&self, ctx: &ToolCallContext) -> ToolGuardResult {
        if !self.enabled {
            return ToolGuardResult::allow();
        }

        let mut findings = Vec::new();
        if let Some(finding) = crate::project_resource_policy::tool_finding(&self.config, ctx) {
            findings.push(finding);
        }
        if self.file_guard_enabled {
            let file_findings = self.file_guardian.evaluate(ctx);
            findings.extend(file_findings);
        }

        let mut explicit_deny = false;
        if is_bash_tool(&ctx.tool_name) {
            if let Some(command) = ctx.tool_input.get("command").and_then(|v| v.as_str()) {
                let analysis = self.bash_analyzer.analyze(command);
                let ast_evaluation = evaluate_ast_rules_full_with_decision(
                    &self.ast_rules,
                    &analysis,
                    command,
                    &ctx.working_dir,
                );
                explicit_deny |= ast_evaluation.has_explicit_deny;
                findings.extend(ast_evaluation.findings);

                for custom in &self.custom_ast_rules {
                    if !custom.applies_to_tool(&ctx.tool_name) {
                        continue;
                    }
                    let custom_evaluation = evaluate_ast_rules_full_with_decision(
                        std::slice::from_ref(&custom.rule),
                        &analysis,
                        command,
                        &ctx.working_dir,
                    );
                    explicit_deny |= custom_evaluation.has_explicit_deny;
                    findings.extend(custom_evaluation.findings);
                }
            }
        }

        let decision = if explicit_deny {
            GuardDecision::Deny
        } else {
            aggregate_decision(&findings)
        };

        let result = ToolGuardResult {
            decision,
            findings: findings.clone(),
            guarded: self.is_guarded(&ctx.tool_name),
            evaluated_at: chrono::Utc::now(),
        };

        SecurityAuditEvent::from_evaluation(
            &ctx.agent_id,
            &ctx.session_id,
            &ctx.tool_name,
            decision,
            &findings,
        )
        .emit();

        result
    }

    pub fn is_guarded(&self, tool_name: &str) -> bool {
        let _ = tool_name;
        true
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
}

impl CustomAstRule {
    fn applies_to_tool(&self, tool_name: &str) -> bool {
        self.tools.is_empty() || self.tools.iter().any(|tool| tool == tool_name)
    }
}

fn parse_severity(s: &str) -> Severity {
    match s.to_uppercase().as_str() {
        "CRITICAL" => Severity::Critical,
        "HIGH" => Severity::High,
        "MEDIUM" => Severity::Medium,
        "LOW" => Severity::Low,
        "INFO" => Severity::Info,
        _ => Severity::High,
    }
}

fn parse_action(s: &str) -> AstAction {
    match s.trim().to_ascii_lowercase().as_str() {
        "allow" => AstAction::Allow,
        "deny" => AstAction::Deny,
        "audit" => AstAction::Audit,
        "needs_approval" | "needsapproval" | "requires_approval" | "ask_user" => {
            AstAction::NeedsApproval
        }
        _ => AstAction::NeedsApproval,
    }
}

fn parse_lexical(s: &str) -> Option<LexicalCheck> {
    match s {
        "ControlChars" => Some(LexicalCheck::ControlChars),
        "UnicodeWhitespace" => Some(LexicalCheck::UnicodeWhitespace),
        "IfsReference" => Some(LexicalCheck::IfsReference),
        "AnsiCQuoting" => Some(LexicalCheck::AnsiCQuoting),
        "BackslashWhitespace" => Some(LexicalCheck::BackslashWhitespace),
        "BackslashOperator" => Some(LexicalCheck::BackslashOperator),
        "EmbeddedNewline" => Some(LexicalCheck::EmbeddedNewline),
        "CommentQuoteDesync" => Some(LexicalCheck::CommentQuoteDesync),
        "QuotedNewline" => Some(LexicalCheck::QuotedNewline),
        _ => None,
    }
}

fn parse_structural(s: &str) -> Option<StructuralFlag> {
    match s {
        "PipeToShell" => Some(StructuralFlag::PipeToShell),
        "CommandSubstitution" => Some(StructuralFlag::CommandSubstitution),
        "SensitiveRedirect" => Some(StructuralFlag::SensitiveRedirect),
        "ForkBomb" => Some(StructuralFlag::ForkBomb),
        "Base64ToShell" => Some(StructuralFlag::Base64ToShell),
        _ => None,
    }
}

fn custom_rule_to_ast(cfg: &ToolGuardRuleConfig) -> AstRule {
    AstRule {
        id: cfg.id.clone(),
        category: cfg.category.clone(),
        severity: parse_severity(&cfg.severity),
        action: parse_action(&cfg.action),
        command_set: cfg.command_set.clone(),
        arg_matchers: cfg
            .arg_matchers
            .iter()
            .map(|m| ArgMatcher {
                command: m.command.clone(),
                flag: m.flag.clone(),
                value_contains: m.value_contains.clone(),
                value_in: m.value_in.clone(),
                exclude_args: vec![],
            })
            .collect(),
        path_matchers: cfg
            .path_matchers
            .iter()
            .map(|m| PathMatcher {
                contains: m.contains.clone(),
                prefix: m.prefix.clone(),
                glob: m.glob.clone(),
                on_redirect: m.on_redirect,
            })
            .collect(),
        lexical: cfg
            .lexical
            .iter()
            .filter_map(|s| parse_lexical(s))
            .collect(),
        structural: cfg
            .structural
            .iter()
            .filter_map(|s| parse_structural(s))
            .collect(),
    }
}

pub fn aggregate_decision(findings: &[ToolGuardFinding]) -> GuardDecision {
    if findings.is_empty() {
        return GuardDecision::Allow;
    }

    if findings.iter().all(|f| f.severity == Severity::Info) {
        return GuardDecision::AuditOnly;
    }

    GuardDecision::RequiresApproval
}

fn is_bash_tool(tool_name: &str) -> bool {
    tool_name == "bash"
}
