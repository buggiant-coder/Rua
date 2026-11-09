use clap::Parser;
use rua_cli::agent::{build_create_agent_body, CreateAgentBodyInput};
use rua_cli::{Cli, Commands};
use serde_json::json;

#[test]
fn create_body_uses_rust_canonical_http_payload_shape() {
    let body = build_create_agent_body(CreateAgentBodyInput {
        id: Some("research".to_string()),
        name: "Research Bot".to_string(),
        description: "research helper".to_string(),
        workspace_dir: Some("/tmp/research".to_string()),
        template_id: Some("qa".to_string()),
        language: Some("zh".to_string()),
        skills: vec!["calendar".to_string(), "search".to_string()],
        provider_id: Some("dashscope".to_string()),
        model_id: Some("qwen-max".to_string()),
    });

    assert_eq!(
        body,
        json!({
            "id": "research",
            "name": "Research Bot",
            "description": "research helper",
            "workspace_dir": "/tmp/research",
            "template_id": "qa",
            "language": "zh",
            "skill_names": ["calendar", "search"],
            "provider_id": "dashscope",
            "model_id": "qwen-max",
        })
    );
    assert!(
        body.get("default_model").is_none(),
        "agent create forwards provider/model fields and does not construct default_model"
    );
}

#[test]
fn create_body_omits_absent_optional_fields() {
    let body = build_create_agent_body(CreateAgentBodyInput {
        name: "Research Bot".to_string(),
        description: String::new(),
        ..Default::default()
    });

    assert_eq!(
        body,
        json!({
            "name": "Research Bot",
            "description": "",
        })
    );
    assert!(body.get("skill_names").is_none());
    assert!(body.get("id").is_none());
    assert!(body.get("workspace_dir").is_none());
    assert!(body.get("template_id").is_none());
    assert!(body.get("provider_id").is_none());
    assert!(body.get("model_id").is_none());
}

#[test]
fn create_cli_requires_name_and_treats_global_agent_id_as_scope_selector() {
    let missing_name = Cli::try_parse_from(["rua", "agent", "create"]).unwrap_err();
    assert!(missing_name.to_string().contains("--name"));

    let cli = Cli::try_parse_from([
        "rua",
        "agent",
        "create",
        "--name",
        "Research Bot",
        "--agent-id",
        "research",
    ])
    .expect("global --agent-id is accepted after subcommands");

    assert_eq!(cli.agent_id, "research");
    assert!(matches!(cli.command, Commands::Agent(_)));

    let body = build_create_agent_body(CreateAgentBodyInput {
        name: "Research Bot".to_string(),
        ..Default::default()
    });
    assert!(
        body.get("id").is_none(),
        "Rust agent create uses explicit --id for creation and leaves global --agent-id as API scope"
    );
}

#[test]
fn create_cli_supports_explicit_rust_id_field_for_custom_agent_ids() {
    let cli = Cli::try_parse_from([
        "rua",
        "agent",
        "create",
        "--id",
        "research",
        "--name",
        "Research Bot",
    ])
    .expect("agent create --id should parse");
    assert!(matches!(cli.command, Commands::Agent(_)));

    let body = build_create_agent_body(CreateAgentBodyInput {
        id: Some("research".to_string()),
        name: "Research Bot".to_string(),
        ..Default::default()
    });

    assert_eq!(
        body,
        json!({
            "id": "research",
            "name": "Research Bot",
            "description": "",
        })
    );
}

#[test]
fn create_body_ignores_blank_explicit_id_so_server_can_generate_one() {
    let body = build_create_agent_body(CreateAgentBodyInput {
        id: Some("   ".to_string()),
        name: "Research Bot".to_string(),
        ..Default::default()
    });

    assert!(body.get("id").is_none());
}

#[test]
fn create_cli_help_uses_rust_names_and_does_not_expose_qwenpaw_template_alias() {
    let err = Cli::try_parse_from(["rua", "agent", "create", "--help"]).unwrap_err();
    let help = err.to_string();

    assert!(help.contains("--id"));
    assert!(help.contains("--template-id"));
    assert!(!help.contains("--template <"));
}

#[test]
fn create_cli_accepts_provider_or_model_independently_for_current_rust_contract() {
    let provider_only = Cli::try_parse_from([
        "rua",
        "agent",
        "create",
        "--name",
        "Research Bot",
        "--provider-id",
        "dashscope",
    ])
    .expect("provider-only create currently parses and is forwarded to the server");
    assert!(matches!(provider_only.command, Commands::Agent(_)));

    let model_only = Cli::try_parse_from([
        "rua",
        "agent",
        "create",
        "--name",
        "Research Bot",
        "--model-id",
        "qwen-max",
    ])
    .expect("model-only create currently parses and is forwarded to the server");
    assert!(matches!(model_only.command, Commands::Agent(_)));
}
