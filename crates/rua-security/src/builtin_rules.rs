use crate::ast_rule::{ArgMatcher, AstAction, AstRule, LexicalCheck, PathMatcher, StructuralFlag};
use crate::models::Severity;

pub fn builtin_structured_rules() -> Vec<AstRule> {
    vec![
        AstRule {
            id: "TOOL_CMD_DANGEROUS_RM".into(),
            category: "command_injection".into(),
            severity: Severity::High,
            action: AstAction::NeedsApproval,
            command_set: ["rm", "del", "Remove-Item"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            ..Default::default()
        },
        AstRule {
            id: "TOOL_CMD_DANGEROUS_MV".into(),
            category: "command_injection".into(),
            severity: Severity::High,
            action: AstAction::NeedsApproval,
            command_set: vec!["mv".into()],
            ..Default::default()
        },
        AstRule {
            id: "TOOL_CMD_FS_DESTRUCTION".into(),
            category: "command_injection".into(),
            severity: Severity::Critical,
            action: AstAction::NeedsApproval,
            command_set: ["mkfs", "mke2fs"].iter().map(|s| s.to_string()).collect(),
            arg_matchers: vec![ArgMatcher {
                command: Some("dd".into()),
                flag: None,
                value_contains: Some("of=/dev/".into()),
                value_in: vec![],
                exclude_args: vec![],
            }],
            path_matchers: vec![PathMatcher {
                contains: vec![],
                prefix: vec!["/dev/sd".into(), "/dev/vd".into(), "/dev/nvme".into()],
                glob: None,
                on_redirect: true,
            }],
            ..Default::default()
        },
        AstRule {
            id: "TOOL_CMD_DOS_FORK_BOMB".into(),
            category: "resource_abuse".into(),
            severity: Severity::Critical,
            action: AstAction::NeedsApproval,
            structural: vec![StructuralFlag::ForkBomb],
            arg_matchers: vec![ArgMatcher {
                command: Some("kill".into()),
                flag: Some("-9".into()),
                value_contains: None,
                value_in: vec!["-1".into(), "1".into()],
                exclude_args: vec![],
            }],
            ..Default::default()
        },
        AstRule {
            id: "TOOL_CMD_PIPE_TO_SHELL".into(),
            category: "code_execution".into(),
            severity: Severity::Critical,
            action: AstAction::NeedsApproval,
            structural: vec![StructuralFlag::PipeToShell],
            ..Default::default()
        },
        AstRule {
            id: "TOOL_CMD_REVERSE_SHELL".into(),
            category: "network_abuse".into(),
            severity: Severity::Critical,
            action: AstAction::NeedsApproval,
            arg_matchers: vec![
                ArgMatcher {
                    command: Some("nc".into()),
                    flag: Some("-e".into()),
                    value_contains: None,
                    value_in: vec![],
                    exclude_args: vec![],
                },
                ArgMatcher {
                    command: Some("ncat".into()),
                    flag: Some("-e".into()),
                    value_contains: None,
                    value_in: vec![],
                    exclude_args: vec![],
                },
                ArgMatcher {
                    command: Some("socat".into()),
                    flag: None,
                    value_contains: Some("EXEC:".into()),
                    value_in: vec![],
                    exclude_args: vec![],
                },
            ],
            path_matchers: vec![PathMatcher {
                contains: vec!["/dev/tcp/".into(), "/dev/udp/".into()],
                prefix: vec![],
                glob: None,
                on_redirect: false,
            }],
            ..Default::default()
        },
        AstRule {
            id: "TOOL_CMD_SYSTEM_TAMPERING".into(),
            category: "sensitive_file_access".into(),
            severity: Severity::Critical,
            action: AstAction::NeedsApproval,
            command_set: vec!["crontab".into()],
            path_matchers: vec![PathMatcher {
                contains: vec![
                    "authorized_keys".into(),
                    "/etc/sudoers".into(),
                    "/etc/crontab".into(),
                ],
                prefix: vec![],
                glob: None,
                on_redirect: false,
            }],
            ..Default::default()
        },
        AstRule {
            id: "TOOL_CMD_UNSAFE_PERMISSIONS".into(),
            category: "privilege_escalation".into(),
            severity: Severity::Critical,
            action: AstAction::NeedsApproval,
            arg_matchers: vec![
                ArgMatcher {
                    command: Some("chmod".into()),
                    flag: Some("-R".into()),
                    value_contains: None,
                    value_in: vec!["777".into(), "a+rwx".into()],
                    exclude_args: vec![],
                },
                ArgMatcher {
                    command: Some("chattr".into()),
                    flag: None,
                    value_contains: Some("+i".into()),
                    value_in: vec![],
                    exclude_args: vec![],
                },
            ],
            ..Default::default()
        },
        AstRule {
            id: "TOOL_CMD_OBFUSCATED_EXEC".into(),
            category: "code_execution".into(),
            severity: Severity::Critical,
            action: AstAction::NeedsApproval,
            structural: vec![StructuralFlag::Base64ToShell],
            ..Default::default()
        },
        AstRule {
            id: "TOOL_CMD_SYSTEM_REBOOT".into(),
            category: "resource_abuse".into(),
            severity: Severity::Critical,
            action: AstAction::NeedsApproval,
            command_set: [
                "reboot",
                "shutdown",
                "halt",
                "poweroff",
                "Shutdown-Computer",
                "Restart-Computer",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            arg_matchers: vec![
                ArgMatcher {
                    command: Some("init".into()),
                    flag: None,
                    value_contains: None,
                    value_in: vec!["0".into(), "6".into()],
                    exclude_args: vec![],
                },
                ArgMatcher {
                    command: Some("telinit".into()),
                    flag: None,
                    value_contains: None,
                    value_in: vec!["0".into(), "6".into()],
                    exclude_args: vec![],
                },
            ],
            ..Default::default()
        },
        AstRule {
            id: "TOOL_CMD_SERVICE_RESTART".into(),
            category: "resource_abuse".into(),
            severity: Severity::High,
            action: AstAction::NeedsApproval,
            arg_matchers: vec![
                ArgMatcher {
                    command: Some("systemctl".into()),
                    flag: None,
                    value_contains: None,
                    value_in: vec![
                        "restart".into(),
                        "stop".into(),
                        "start".into(),
                        "reload".into(),
                        "kill".into(),
                    ],
                    exclude_args: vec![],
                },
                ArgMatcher {
                    command: Some("service".into()),
                    flag: None,
                    value_contains: None,
                    value_in: vec![
                        "restart".into(),
                        "stop".into(),
                        "start".into(),
                        "reload".into(),
                    ],
                    exclude_args: vec![],
                },
                ArgMatcher {
                    command: Some("launchctl".into()),
                    flag: None,
                    value_contains: None,
                    value_in: vec![
                        "load".into(),
                        "unload".into(),
                        "stop".into(),
                        "start".into(),
                        "kickstart".into(),
                        "kill".into(),
                    ],
                    exclude_args: vec![],
                },
                ArgMatcher {
                    command: Some("rc-service".into()),
                    flag: None,
                    value_contains: None,
                    value_in: vec!["restart".into(), "stop".into(), "start".into()],
                    exclude_args: vec![],
                },
                ArgMatcher {
                    command: Some("sc".into()),
                    flag: None,
                    value_contains: None,
                    value_in: vec!["start".into(), "stop".into(), "restart".into()],
                    exclude_args: vec![],
                },
                ArgMatcher {
                    command: Some("net".into()),
                    flag: None,
                    value_contains: None,
                    value_in: vec!["start".into(), "stop".into(), "restart".into()],
                    exclude_args: vec![],
                },
            ],
            ..Default::default()
        },
        AstRule {
            id: "TOOL_CMD_PROCESS_KILL".into(),
            category: "resource_abuse".into(),
            severity: Severity::High,
            action: AstAction::NeedsApproval,
            command_set: ["pkill", "killall", "Stop-Process"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            arg_matchers: vec![
                ArgMatcher {
                    command: Some("kill".into()),
                    flag: None,
                    value_contains: None,
                    value_in: vec![],
                    exclude_args: vec!["$$".into()],
                },
                ArgMatcher {
                    command: Some("taskkill".into()),
                    flag: Some("/F".into()),
                    value_contains: None,
                    value_in: vec![],
                    exclude_args: vec![],
                },
            ],
            ..Default::default()
        },
        AstRule {
            id: "TOOL_CMD_PRIVILEGE_ESCALATION".into(),
            category: "privilege_escalation".into(),
            severity: Severity::Critical,
            action: AstAction::NeedsApproval,
            command_set: ["sudo", "su", "doas", "pkexec"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            arg_matchers: vec![ArgMatcher {
                command: Some("runas".into()),
                flag: None,
                value_contains: Some("/user:".into()),
                value_in: vec![],
                exclude_args: vec![],
            }],
            ..Default::default()
        },
        AstRule {
            id: "TOOL_CMD_IFS_INJECTION".into(),
            category: "code_execution".into(),
            severity: Severity::High,
            action: AstAction::NeedsApproval,
            lexical: vec![LexicalCheck::IfsReference],
            ..Default::default()
        },
        AstRule {
            id: "TOOL_CMD_CONTROL_CHARS".into(),
            category: "code_execution".into(),
            severity: Severity::Critical,
            action: AstAction::NeedsApproval,
            lexical: vec![LexicalCheck::ControlChars],
            ..Default::default()
        },
        AstRule {
            id: "TOOL_CMD_UNICODE_WHITESPACE".into(),
            category: "code_execution".into(),
            severity: Severity::High,
            action: AstAction::NeedsApproval,
            lexical: vec![LexicalCheck::UnicodeWhitespace],
            ..Default::default()
        },
        AstRule {
            id: "TOOL_CMD_PROC_ENVIRON".into(),
            category: "sensitive_file_access".into(),
            severity: Severity::High,
            action: AstAction::NeedsApproval,
            path_matchers: vec![PathMatcher {
                contains: vec![],
                prefix: vec![],
                glob: Some("/proc/*/environ".into()),
                on_redirect: false,
            }],
            ..Default::default()
        },
        AstRule {
            id: "TOOL_CMD_JQ_SYSTEM".into(),
            category: "code_execution".into(),
            severity: Severity::High,
            action: AstAction::NeedsApproval,
            arg_matchers: vec![ArgMatcher {
                command: Some("jq".into()),
                flag: None,
                value_contains: Some("system(".into()),
                value_in: vec![],
                exclude_args: vec![],
            }],
            ..Default::default()
        },
        AstRule {
            id: "TOOL_CMD_JQ_FILE_FLAGS".into(),
            category: "code_execution".into(),
            severity: Severity::High,
            action: AstAction::NeedsApproval,
            arg_matchers: vec![
                ArgMatcher {
                    command: Some("jq".into()),
                    flag: Some("-f".into()),
                    value_contains: None,
                    value_in: vec![],
                    exclude_args: vec![],
                },
                ArgMatcher {
                    command: Some("jq".into()),
                    flag: Some("--from-file".into()),
                    value_contains: None,
                    value_in: vec![],
                    exclude_args: vec![],
                },
                ArgMatcher {
                    command: Some("jq".into()),
                    flag: Some("--rawfile".into()),
                    value_contains: None,
                    value_in: vec![],
                    exclude_args: vec![],
                },
                ArgMatcher {
                    command: Some("jq".into()),
                    flag: Some("--slurpfile".into()),
                    value_contains: None,
                    value_in: vec![],
                    exclude_args: vec![],
                },
                ArgMatcher {
                    command: Some("jq".into()),
                    flag: Some("-L".into()),
                    value_contains: None,
                    value_in: vec![],
                    exclude_args: vec![],
                },
                ArgMatcher {
                    command: Some("jq".into()),
                    flag: Some("--library-path".into()),
                    value_contains: None,
                    value_in: vec![],
                    exclude_args: vec![],
                },
            ],
            ..Default::default()
        },
        AstRule {
            id: "TOOL_CMD_ZSH_DANGEROUS".into(),
            category: "command_obfuscation".into(),
            severity: Severity::High,
            action: AstAction::NeedsApproval,
            command_set: [
                "zmodload", "zpty", "ztcp", "zsocket", "sysopen", "sysread", "syswrite", "sysseek",
                "zf_rm", "zf_mv", "zf_ln", "zf_chmod", "zf_chown", "zf_mkdir", "zf_rmdir",
                "zf_chgrp",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            arg_matchers: vec![
                ArgMatcher {
                    command: Some("emulate".into()),
                    flag: Some("-c".into()),
                    value_contains: None,
                    value_in: vec![],
                    exclude_args: vec![],
                },
                ArgMatcher {
                    command: Some("fc".into()),
                    flag: Some("-e".into()),
                    value_contains: None,
                    value_in: vec![],
                    exclude_args: vec![],
                },
            ],
            ..Default::default()
        },
        AstRule {
            id: "SHELL_EVASION_COMMAND_SUBSTITUTION".into(),
            category: "command_obfuscation".into(),
            severity: Severity::High,
            action: AstAction::NeedsApproval,
            structural: vec![StructuralFlag::CommandSubstitution],
            ..Default::default()
        },
        AstRule {
            id: "SHELL_EVASION_OBFUSCATED_FLAGS".into(),
            category: "command_obfuscation".into(),
            severity: Severity::High,
            action: AstAction::NeedsApproval,
            lexical: vec![LexicalCheck::AnsiCQuoting],
            ..Default::default()
        },
        AstRule {
            id: "SHELL_EVASION_BACKSLASH_WHITESPACE".into(),
            category: "command_obfuscation".into(),
            severity: Severity::High,
            action: AstAction::NeedsApproval,
            lexical: vec![LexicalCheck::BackslashWhitespace],
            ..Default::default()
        },
        AstRule {
            id: "SHELL_EVASION_BACKSLASH_OPERATOR".into(),
            category: "command_obfuscation".into(),
            severity: Severity::High,
            action: AstAction::NeedsApproval,
            lexical: vec![LexicalCheck::BackslashOperator],
            ..Default::default()
        },
        AstRule {
            id: "SHELL_EVASION_NEWLINE".into(),
            category: "command_obfuscation".into(),
            severity: Severity::High,
            action: AstAction::NeedsApproval,
            lexical: vec![LexicalCheck::EmbeddedNewline],
            ..Default::default()
        },
        AstRule {
            id: "SHELL_EVASION_COMMENT_QUOTE_DESYNC".into(),
            category: "command_obfuscation".into(),
            severity: Severity::High,
            action: AstAction::NeedsApproval,
            lexical: vec![LexicalCheck::CommentQuoteDesync],
            ..Default::default()
        },
        AstRule {
            id: "SHELL_EVASION_QUOTED_NEWLINE".into(),
            category: "command_obfuscation".into(),
            severity: Severity::High,
            action: AstAction::NeedsApproval,
            lexical: vec![LexicalCheck::QuotedNewline],
            ..Default::default()
        },
    ]
}
