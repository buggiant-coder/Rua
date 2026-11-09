use clap::Parser;
use rua_cli::providers::ProvidersCommands;

#[derive(Parser)]
struct TestCli {
    #[command(subcommand)]
    cmd: ProvidersCommands,
}

#[test]
fn providers_subcommands_parse_rust_canonical_surface() {
    let cli = TestCli::parse_from(["test", "list"]);
    assert!(matches!(cli.cmd, ProvidersCommands::List));

    let cli = TestCli::parse_from([
        "test",
        "add",
        "my-provider",
        "-n",
        "My Provider",
        "-u",
        "https://api.example.com",
    ]);
    assert!(matches!(cli.cmd, ProvidersCommands::Add { .. }));

    let cli = TestCli::parse_from(["test", "get", "dashscope"]);
    assert!(matches!(cli.cmd, ProvidersCommands::Get { .. }));

    let cli = TestCli::parse_from(["test", "set-api-key", "dashscope", "sk-test"]);
    assert!(matches!(cli.cmd, ProvidersCommands::SetApiKey { .. }));
}
