use rua_core::{ChatAddress, ChatScopeStore, RuaRoot};

#[test]
fn chat_scope_store_uses_the_same_workspace_roots_as_rua_root() {
    let temp = tempfile::tempdir().expect("tempdir");
    let rua_root = RuaRoot::new(temp.path());
    let agent = rua_root
        .agent_workspace("agent-a")
        .expect("Agent Workspace");
    let project = rua_root
        .project_workspace("project-a")
        .expect("Project Workspace");

    assert_eq!(
        ChatScopeStore::resolve(temp.path(), ChatAddress::agent("agent-a", "chat-a")).root(),
        agent.as_path()
    );
    assert_eq!(
        ChatScopeStore::resolve(temp.path(), ChatAddress::project("project-a", "chat-a")).root(),
        project.as_path()
    );
}

#[test]
fn workspace_roots_are_lazy_and_share_standard_relative_paths() {
    let temp = tempfile::tempdir().expect("tempdir");
    let rua_root = RuaRoot::new(temp.path());
    let agent = rua_root
        .agent_workspace("agent-a")
        .expect("Agent Workspace");
    let project = rua_root
        .project_workspace("project-a")
        .expect("Project Workspace");

    assert!(!agent.as_path().exists());
    assert!(!project.as_path().exists());
    assert_eq!(
        agent.resolve("media/image.png").unwrap(),
        agent.media_dir().join("image.png")
    );
    assert_eq!(
        project.resolve("media/image.png").unwrap(),
        project.media_dir().join("image.png")
    );
    for invalid in ["", "/tmp/file", "../file", "media/../file", r"media\file"] {
        assert!(agent.resolve(invalid).is_err(), "accepted {invalid:?}");
    }
}
