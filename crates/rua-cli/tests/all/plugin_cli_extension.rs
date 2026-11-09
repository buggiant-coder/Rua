use clap::{CommandFactory, FromArgMatches};
use rua_cli::{Cli, Commands};
use rua_plugin_runtime::PluginCliCommand;
use rua_plugin_runtime::{CliArgSpec, CliCommandSpec};

fn make_plugin_cmd(name: &str, desc: &str) -> PluginCliCommand {
    PluginCliCommand {
        plugin_id: "test-plugin".to_string(),
        spec: CliCommandSpec {
            name: name.to_string(),
            description: desc.to_string(),
            args: vec![CliArgSpec {
                name: "target".to_string(),
                description: "Target name".to_string(),
                required: false,
            }],
            callback: format!("handle_{name}"),
        },
    }
}

fn build_augmented_cli(plugin_cmds: &[PluginCliCommand]) -> clap::Command {
    let builtin_cmd = Cli::command();
    let builtin_names: Vec<String> = builtin_cmd
        .get_subcommands()
        .map(|c| c.get_name().to_string())
        .collect();

    let mut cmd = builtin_cmd;
    for pc in plugin_cmds {
        if builtin_names.contains(&pc.spec.name) {
            continue;
        }
        let mut sub = clap::Command::new(pc.spec.name.clone()).about(pc.spec.description.clone());
        for arg_spec in &pc.spec.args {
            let mut arg = clap::Arg::new(arg_spec.name.clone())
                .long(arg_spec.name.clone())
                .help(arg_spec.description.clone());
            if arg_spec.required {
                arg = arg.required(true);
            }
            sub = sub.arg(arg);
        }
        cmd = cmd.subcommand(sub);
    }
    cmd
}

#[test]
fn plugin_subcommand_appears_in_help() {
    let plugin_cmds = vec![make_plugin_cmd("greet", "Say hello from plugin")];
    let cmd = build_augmented_cli(&plugin_cmds);

    let sub_names: Vec<&str> = cmd.get_subcommands().map(|c| c.get_name()).collect();
    assert!(
        sub_names.contains(&"greet"),
        "plugin subcommand not found in CLI"
    );
}

#[test]
fn builtin_subcommands_still_present() {
    let plugin_cmds = vec![make_plugin_cmd("greet", "Say hello")];
    let cmd = build_augmented_cli(&plugin_cmds);

    let sub_names: Vec<&str> = cmd.get_subcommands().map(|c| c.get_name()).collect();
    assert!(sub_names.contains(&"agent"));
    assert!(sub_names.contains(&"config"));
    assert!(sub_names.contains(&"plugin"));
}

#[test]
fn plugin_subcommand_does_not_collide_with_builtin() {
    let plugin_cmds = vec![make_plugin_cmd("agent", "Conflict with built-in")];
    let cmd = build_augmented_cli(&plugin_cmds);

    let agent_count = cmd
        .get_subcommands()
        .filter(|c| c.get_name() == "agent")
        .count();
    assert_eq!(
        agent_count, 1,
        "conflicting plugin command should be skipped"
    );
}

#[test]
fn plugin_subcommand_parsed_from_args() {
    let plugin_cmds = vec![make_plugin_cmd("greet", "Say hello")];
    let cmd = build_augmented_cli(&plugin_cmds);

    let matches = cmd
        .try_get_matches_from(["rua", "greet", "--target", "world"])
        .expect("should parse plugin subcommand");

    let (sub_name, sub_matches) = matches.subcommand().unwrap();
    assert_eq!(sub_name, "greet");
    assert_eq!(
        sub_matches.get_one::<String>("target").map(|s| s.as_str()),
        Some("world")
    );
}

#[test]
fn builtin_subcommand_still_parseable() {
    let plugin_cmds = vec![make_plugin_cmd("greet", "Say hello")];
    let cmd = build_augmented_cli(&plugin_cmds);

    let matches = cmd
        .try_get_matches_from(["rua", "config", "show"])
        .expect("should parse built-in subcommand");

    let cli = Cli::from_arg_matches(&matches).expect("should match built-in Commands");
    assert!(matches!(cli.command, Commands::Config(_)));
}

#[test]
fn plugin_subcommand_dispatch_identified_correctly() {
    let plugin_cmds = vec![
        make_plugin_cmd("greet", "Say hello"),
        make_plugin_cmd("farewell", "Say bye"),
    ];
    let cmd = build_augmented_cli(&plugin_cmds);

    let matches = cmd
        .try_get_matches_from(["rua", "farewell"])
        .expect("should parse");

    if let Some((sub_name, _)) = matches.subcommand() {
        let found = plugin_cmds.iter().find(|c| c.spec.name == sub_name);
        assert!(found.is_some(), "plugin command should be identified");
        assert_eq!(found.unwrap().spec.name, "farewell");
    } else {
        panic!("expected a subcommand");
    }
}
