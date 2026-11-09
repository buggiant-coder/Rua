use clap::Parser;
use rua_cli::{Cli, Commands};

#[test]
fn app_alias_parses_as_server() {
    let cli = Cli::try_parse_from(["rua", "app"]).expect("rua app should parse");
    assert!(matches!(cli.command, Commands::Server(_)));
}

#[test]
fn app_alias_with_host_port() {
    let cli = Cli::try_parse_from(["rua", "app", "--host", "0.0.0.0", "--port", "9999"])
        .expect("rua app --host --port should parse");
    assert!(matches!(cli.command, Commands::Server(_)));
}

#[test]
fn server_still_works() {
    let cli = Cli::try_parse_from(["rua", "server"]).expect("rua server should parse");
    assert!(matches!(cli.command, Commands::Server(_)));
}

#[test]
fn help_text_mentions_app_alias() {
    let err = Cli::try_parse_from(["rua", "--help"]).unwrap_err();
    let help = err.to_string();
    assert!(help.contains("app"), "help should mention 'app' alias");
}
