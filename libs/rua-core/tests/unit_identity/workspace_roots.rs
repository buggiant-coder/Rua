use rua_core::{ChatAddress, ChatStore, RuaRoot};

#[test]
fn chat_workspace_is_independent_of_agent_and_project_workspaces() {
    let temp = tempfile::tempdir().unwrap();
    let rua_root = RuaRoot::new(temp.path());
    let chats = rua_root.chat_workspace();
    assert_eq!(chats.as_path(), temp.path().join("chats"));
    assert_eq!(
        ChatStore::resolve(temp.path(), ChatAddress::new("chat-a")).root(),
        chats.as_path()
    );
    assert_ne!(chats, rua_root.agent_workspace("chats").unwrap());
    assert_ne!(chats, rua_root.project_workspace("chats").unwrap());
    assert_eq!(
        rua_root.agent_workspace("agent-a").unwrap().as_path(),
        temp.path().join("workspaces/agent-a")
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
