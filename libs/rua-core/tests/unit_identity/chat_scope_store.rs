use rua_core::{ChatAddress, ChatScopeStore};

#[test]
fn scope_store_resolves_agent_and_project_roots_without_collision() {
    let data_dir = tempfile::tempdir().expect("data dir");
    let agent = ChatScopeStore::resolve(
        data_dir.path(),
        ChatAddress::agent("owner-a", "shared-chat"),
    );
    let project = ChatScopeStore::resolve(
        data_dir.path(),
        ChatAddress::project("owner-a", "shared-chat"),
    );

    assert_eq!(
        agent.root(),
        data_dir.path().join("workspaces").join("owner-a")
    );
    assert_eq!(
        project.root(),
        data_dir.path().join("projects").join("owner-a")
    );
    assert_ne!(agent.chat_index_path(), project.chat_index_path());
    assert_ne!(agent.evergreen_chat_root(), project.evergreen_chat_root());
}

#[test]
fn agent_workspace_adapter_rejects_project_scope() {
    let workspace = tempfile::tempdir().expect("workspace");
    let error = ChatScopeStore::for_agent_workspace(
        workspace.path(),
        ChatAddress::project("project-a", "chat-a"),
    )
    .expect_err("Project Scope must not resolve through an Agent workspace");

    assert!(error
        .to_string()
        .contains("Agent workspace cannot own a Project-Scoped Chat"));
}
