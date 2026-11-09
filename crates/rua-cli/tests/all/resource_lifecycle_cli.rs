use clap::Parser;
use rua_cli::{Cli, Commands};

#[test]
fn mcp_lifecycle_commands_and_aliases_parse() {
    for args in [
        vec![
            "rua",
            "mcp",
            "install",
            "remote",
            "--url",
            "https://example.test/mcp",
        ],
        vec!["rua", "mcp", "add", "stdio", "--", "npx", "-y", "demo-mcp"],
        vec!["rua", "mcp", "import", "-", "--shared", "--replace"],
        vec!["rua", "mcp", "enable", "demo"],
        vec!["rua", "mcp", "tools", "demo"],
        vec!["rua", "mcp", "call", "demo", "search", "--args", "{}"],
        vec!["rua", "mcp", "login", "remote"],
        vec!["rua", "mcp", "remove", "demo", "--yes"],
    ] {
        let cli = Cli::try_parse_from(args).expect("MCP lifecycle command should parse");
        assert!(matches!(cli.command, Commands::Mcp(_)));
    }
}

#[test]
fn skill_lifecycle_commands_and_aliases_parse() {
    for args in [
        vec!["rua", "skills", "search", "pdf", "--provider", "clawhub"],
        vec!["rua", "skills", "install", "https://clawhub.ai/skills/pdf"],
        vec![
            "rua",
            "skills",
            "import",
            "./pdf-skill",
            "--pool",
            "--replace",
        ],
        vec!["rua", "skills", "check-update", "pdf"],
        vec!["rua", "skills", "update", "pdf", "--force"],
        vec!["rua", "skills", "info", "pdf"],
        vec!["rua", "skills", "remove", "pdf", "--yes"],
    ] {
        let cli = Cli::try_parse_from(args).expect("Skill lifecycle command should parse");
        assert!(matches!(cli.command, Commands::Skills(_)));
    }
}
