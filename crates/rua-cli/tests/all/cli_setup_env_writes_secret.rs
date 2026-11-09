use std::collections::BTreeMap;

use rua_config::EnvStore;

#[test]
fn non_interactive_writes_envs_json_from_env_vars() {
    let tmp = tempfile::tempdir().unwrap();
    let secret_dir = tmp.path().to_path_buf();
    let store = EnvStore::new(secret_dir.clone());

    store
        .set("OPENAI_API_KEY", "sk-test-openai-key".to_string())
        .unwrap();
    store
        .set("ANTHROPIC_API_KEY", "sk-ant-test-anthropic-key".to_string())
        .unwrap();

    let envs = store.list().unwrap();
    assert_eq!(envs.get("OPENAI_API_KEY").unwrap(), "sk-test-openai-key");
    assert_eq!(
        envs.get("ANTHROPIC_API_KEY").unwrap(),
        "sk-ant-test-anthropic-key"
    );

    let path = secret_dir.join("envs.json");
    assert!(path.is_file());
    let content: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(content["OPENAI_API_KEY"], "sk-test-openai-key");
    assert_eq!(content["ANTHROPIC_API_KEY"], "sk-ant-test-anthropic-key");
}

#[test]
fn existing_fields_preserved_on_merge() {
    let tmp = tempfile::tempdir().unwrap();
    let secret_dir = tmp.path().to_path_buf();
    let store = EnvStore::new(secret_dir.clone());

    let mut initial = BTreeMap::new();
    initial.insert("EXISTING_KEY".to_string(), "existing-value".to_string());
    initial.insert("OPENAI_API_KEY".to_string(), "old-openai-key".to_string());
    store.save(initial).unwrap();

    store
        .set("ANTHROPIC_API_KEY", "new-anthropic-key".to_string())
        .unwrap();
    store
        .set("OPENAI_API_KEY", "new-openai-key".to_string())
        .unwrap();

    let envs = store.list().unwrap();
    assert_eq!(
        envs.get("EXISTING_KEY").unwrap(),
        "existing-value",
        "pre-existing key must be preserved"
    );
    assert_eq!(
        envs.get("OPENAI_API_KEY").unwrap(),
        "new-openai-key",
        "updated key must reflect new value"
    );
    assert_eq!(
        envs.get("ANTHROPIC_API_KEY").unwrap(),
        "new-anthropic-key",
        "new key must be added"
    );
}

#[test]
fn setup_env_subcommand_parses() {
    use clap::Parser;
    use rua_cli::{Cli, Commands};

    let cli = Cli::try_parse_from(["rua", "setup-env", "--non-interactive"])
        .expect("rua setup-env --non-interactive should parse");
    assert!(matches!(cli.command, Commands::SetupEnv(_)));
}

#[test]
fn setup_env_without_flag_parses() {
    use clap::Parser;
    use rua_cli::{Cli, Commands};

    let cli = Cli::try_parse_from(["rua", "setup-env"]).expect("rua setup-env should parse");
    assert!(matches!(cli.command, Commands::SetupEnv(_)));
}
