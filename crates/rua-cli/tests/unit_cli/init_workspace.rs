use rua_cli::init::{run, InitArgs};

#[test]
fn init_creates_workspace_structure() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("test-workspace");

    run(InitArgs {}, &target).unwrap();

    assert!(target.join("workspaces").is_dir());
    assert!(target.join("local_models").is_dir());
    assert!(target.join("envs.json").is_file());
    assert!(target.join("workspaces/default/agent.json").is_file());

    let secret = dir.path().join("test-workspace.secret");
    assert!(secret.is_dir());

    let agent: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(target.join("workspaces/default/agent.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(agent["id"], "default");
}

#[test]
fn init_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("ws");

    for _ in 0..2 {
        run(InitArgs {}, &target).unwrap();
    }

    assert!(target.join("workspaces/default/agent.json").is_file());
}
