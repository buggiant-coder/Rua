#[test]
fn init_creates_readable_current_configuration_and_preserves_invalid_existing_file() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("rua");
    rua_cli::init::run(rua_cli::init::InitArgs {}, &root).unwrap();
    let config = rua_config::load_config(&root.join("config.json"), &root).unwrap();
    assert!(config.agents.profiles.contains_key("default"));
    let agent = rua_config::load_agent_config("default", &root).unwrap();
    assert_eq!(agent.id, "default");
    assert!(agent.tools.is_some());
    std::fs::write(root.join("config.json"), "invalid JSON").unwrap();
    assert!(rua_cli::init::run(rua_cli::init::InitArgs {}, &root).is_err());
    assert_eq!(
        std::fs::read_to_string(root.join("config.json")).unwrap(),
        "invalid JSON"
    );
}
