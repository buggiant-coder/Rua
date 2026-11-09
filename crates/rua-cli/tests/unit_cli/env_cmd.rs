use std::collections::HashMap;

use clap::Parser;
use rua_cli::env_cmd::{run, EnvArgs, EnvCommands};
use rua_cli::{Cli, Commands};
use rua_config::dirs::Paths;

fn paths_for(temp: &tempfile::TempDir) -> Paths {
    Paths {
        rua_root: temp.path().join("data"),
        secret_dir: temp.path().join("secret"),
        cache_dir: temp.path().join("cache"),
    }
}

fn read_envs(paths: &Paths) -> HashMap<String, String> {
    let path = paths.secret_dir.join("envs.json");
    serde_json::from_slice(&std::fs::read(path).expect("envs.json should exist"))
        .expect("envs.json should be an object")
}

#[test]
fn env_cli_parses_rust_canonical_list_set_and_delete_commands() {
    for args in [
        ["rua", "env", "list"].as_slice(),
        ["rua", "env", "set", "OPENAI_API_KEY", "sk-test"].as_slice(),
        ["rua", "env", "delete", "OPENAI_API_KEY"].as_slice(),
    ] {
        let cli = Cli::try_parse_from(args).expect("env command should parse");
        assert!(
            matches!(cli.command, Commands::Env(_)),
            "expected Env command for args {args:?}"
        );
    }
}

#[test]
fn env_set_writes_secret_envs_json_and_preserves_existing_keys() {
    let temp = tempfile::tempdir().expect("tempdir");
    let paths = paths_for(&temp);

    run(
        EnvArgs {
            command: EnvCommands::Set {
                key: "OPENAI_API_KEY".to_string(),
                value: Some("sk-test-secret".to_string()),
            },
        },
        &paths,
    )
    .expect("set OPENAI_API_KEY");

    run(
        EnvArgs {
            command: EnvCommands::Set {
                key: "VISIBLE_FLAG".to_string(),
                value: Some("enabled".to_string()),
            },
        },
        &paths,
    )
    .expect("set VISIBLE_FLAG");

    let envs = read_envs(&paths);
    assert_eq!(
        envs.get("OPENAI_API_KEY").map(String::as_str),
        Some("sk-test-secret")
    );
    assert_eq!(
        envs.get("VISIBLE_FLAG").map(String::as_str),
        Some("enabled")
    );
    assert!(!paths.rua_root.join("envs.json").exists());
}

#[test]
fn env_delete_removes_only_requested_key_and_errors_for_missing_key() {
    let temp = tempfile::tempdir().expect("tempdir");
    let paths = paths_for(&temp);

    for (key, value) in [("DELETE_ME", "x"), ("KEEP_ME", "y")] {
        run(
            EnvArgs {
                command: EnvCommands::Set {
                    key: key.to_string(),
                    value: Some(value.to_string()),
                },
            },
            &paths,
        )
        .expect("seed env");
    }

    run(
        EnvArgs {
            command: EnvCommands::Delete {
                key: "DELETE_ME".to_string(),
            },
        },
        &paths,
    )
    .expect("delete existing env");

    let envs = read_envs(&paths);
    assert!(!envs.contains_key("DELETE_ME"));
    assert_eq!(envs.get("KEEP_ME").map(String::as_str), Some("y"));

    let err = run(
        EnvArgs {
            command: EnvCommands::Delete {
                key: "DELETE_ME".to_string(),
            },
        },
        &paths,
    )
    .expect_err("deleting a missing env key should fail");
    assert!(err.to_string().contains("Env var 'DELETE_ME' not found"));
}
