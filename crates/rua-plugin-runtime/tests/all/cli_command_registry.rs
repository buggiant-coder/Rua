use rua_plugin_runtime::{CliArgSpec, CliCommandSpec, CliError, PluginRegistry};

fn make_cli_spec(name: &str) -> CliCommandSpec {
    CliCommandSpec {
        name: name.to_string(),
        description: format!("Command {name}"),
        args: vec![],
        callback: format!("handle_{name}"),
    }
}

fn make_cli_spec_with_args(name: &str) -> CliCommandSpec {
    CliCommandSpec {
        name: name.to_string(),
        description: format!("Command {name}"),
        args: vec![
            CliArgSpec {
                name: "query".to_string(),
                description: "Search query".to_string(),
                required: true,
            },
            CliArgSpec {
                name: "limit".to_string(),
                description: "Max results".to_string(),
                required: false,
            },
        ],
        callback: format!("handle_{name}"),
    }
}

#[test]
fn register_and_get_cli_command() {
    let registry = PluginRegistry::new();
    let spec = make_cli_spec("greet");

    registry.register_cli("p1", spec.clone()).unwrap();

    let got = registry.get_cli_command("p1", "greet").unwrap();
    assert_eq!(got, spec);
}

#[test]
fn register_cli_duplicate() {
    let registry = PluginRegistry::new();
    registry.register_cli("p1", make_cli_spec("greet")).unwrap();

    let err = registry
        .register_cli("p1", make_cli_spec("greet"))
        .unwrap_err();
    assert!(matches!(err, CliError::AlreadyRegistered { .. }));
}

#[test]
fn get_cli_command_not_found() {
    let registry = PluginRegistry::new();
    let err = registry.get_cli_command("p1", "nope").unwrap_err();
    assert!(matches!(err, CliError::NotFound { .. }));
}

#[test]
fn list_cli_commands_for_plugin() {
    let registry = PluginRegistry::new();
    registry.register_cli("p1", make_cli_spec("a")).unwrap();
    registry.register_cli("p1", make_cli_spec("b")).unwrap();

    let mut cmds = registry.list_cli_commands("p1");
    cmds.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(cmds.len(), 2);
    assert_eq!(cmds[0].name, "a");
    assert_eq!(cmds[1].name, "b");
}

#[test]
fn list_cli_commands_empty() {
    let registry = PluginRegistry::new();
    assert!(registry.list_cli_commands("nonexistent").is_empty());
}

#[test]
fn list_all_cli_commands() {
    let registry = PluginRegistry::new();
    registry.register_cli("p1", make_cli_spec("a")).unwrap();
    registry.register_cli("p2", make_cli_spec("b")).unwrap();

    let all = registry.list_all_cli_commands();
    assert_eq!(all.len(), 2);
}

#[test]
fn remove_plugin_cli_commands() {
    let registry = PluginRegistry::new();
    registry.register_cli("p1", make_cli_spec("a")).unwrap();
    registry.register_cli("p1", make_cli_spec("b")).unwrap();
    assert_eq!(registry.list_cli_commands("p1").len(), 2);

    registry.remove_plugin_cli_commands("p1");
    assert!(registry.list_cli_commands("p1").is_empty());
}

#[test]
fn cli_command_spec_with_args() {
    let spec = make_cli_spec_with_args("search");
    assert_eq!(spec.args.len(), 2);
    assert!(spec.args[0].required);
    assert!(!spec.args[1].required);
}

#[test]
fn cli_command_spec_serde_roundtrip() {
    let spec = make_cli_spec_with_args("search");
    let json = serde_json::to_string(&spec).unwrap();
    let parsed: CliCommandSpec = serde_json::from_str(&json).unwrap();
    assert_eq!(spec, parsed);
}

#[test]
fn register_cli_from_json() {
    let registry = PluginRegistry::new();
    let json = r#"{"name":"greet","description":"Greet user","args":[],"callback":"handle_greet"}"#;
    let spec: CliCommandSpec = serde_json::from_str(json).unwrap();
    registry.register_cli("p1", spec).unwrap();

    let got = registry.get_cli_command("p1", "greet").unwrap();
    assert_eq!(got.name, "greet");
    assert_eq!(got.callback, "handle_greet");
}

#[test]
fn register_cli_invalid_json() {
    let result = serde_json::from_str::<CliCommandSpec>("not json");
    assert!(result.is_err());
}
