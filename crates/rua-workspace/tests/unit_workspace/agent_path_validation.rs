use rua_workspace::{
    ComponentState, DefaultWorkspaceFs, PlatformProfile, RuaRoot, WorkspaceFs,
    WorkspaceLifecycleState, WorkspaceRuntime,
};
use tempfile::TempDir;

use super::test_helpers;

fn workspace_root(tmp: &TempDir) -> RuaRoot {
    RuaRoot::new(tmp.path().join("rua"))
}

#[test]
fn workspace_owner_ids_reject_path_escape_without_creating_paths() {
    let tmp = TempDir::new().expect("tempdir");
    let root = workspace_root(&tmp);
    let outside = root.rua_root.join("outside-agent");

    root.agent_workspace("../outside-agent")
        .expect_err("invalid Agent ID must be rejected");
    root.project_workspace("../outside-agent")
        .expect_err("invalid Project ID must be rejected");

    assert!(!outside.exists());
}

#[test]
fn agent_and_project_workspaces_share_layout_without_eager_creation() {
    let tmp = TempDir::new().expect("tempdir");
    let root = workspace_root(&tmp);

    let agent = root.agent_workspace("agent-a").expect("Agent Workspace");
    let project = root
        .project_workspace("project-a")
        .expect("Project Workspace");

    assert_eq!(agent.as_path(), root.rua_root.join("workspaces/agent-a"));
    assert_eq!(project.as_path(), root.rua_root.join("projects/project-a"));
    assert_eq!(agent.media_dir(), agent.as_path().join("media"));
    assert_eq!(project.media_dir(), project.as_path().join("media"));
    assert_eq!(agent.sessions_dir(), agent.as_path().join("sessions"));
    assert_eq!(project.sessions_dir(), project.as_path().join("sessions"));
    assert_eq!(
        agent.chat_index_path(),
        agent.as_path().join("chat-index.sqlite")
    );
    assert_eq!(
        project.chat_index_path(),
        project.as_path().join("chat-index.sqlite")
    );
    assert!(!agent.as_path().exists());
    assert!(!project.as_path().exists());
}

#[test]
fn workspace_creation_is_idempotent() {
    let tmp = TempDir::new().expect("tempdir");
    let root = workspace_root(&tmp);

    let workspace = root.agent_workspace("default").expect("Agent Workspace");
    workspace.ensure().expect("first create");
    workspace.ensure().expect("idempotent create");
    assert!(workspace.as_path().is_dir());
}

#[test]
fn workspace_relative_paths_reject_absolute_parent_and_windows_escape_forms() {
    let tmp = TempDir::new().expect("tempdir");
    let root = workspace_root(&tmp);
    let workspace = root.agent_workspace("default").expect("Agent Workspace");
    let fs = DefaultWorkspaceFs::new(workspace.clone()).with_secret_dir(&root.secret_dir);

    assert_eq!(
        fs.resolve_user_path("media/image.png")
            .expect("valid relative"),
        workspace.as_path().join("media/image.png")
    );
    for invalid in [
        "",
        "/etc/passwd",
        "../outside",
        "media/../outside",
        r"..\outside",
    ] {
        fs.resolve_user_path(invalid)
            .expect_err("invalid workspace-relative path must be rejected");
    }
}

#[tokio::test]
async fn new_runtime_preserves_agent_identity_and_stays_uninitialized_until_start() {
    let tmp = TempDir::new().expect("tempdir");
    let working = tmp.path().join("work");
    let secret = tmp.path().join("secret");
    std::fs::create_dir_all(&working).expect("working dir");
    std::fs::create_dir_all(&secret).expect("secret dir");
    let root = RuaRoot::from_paths(&working, &secret);
    let agent_workspace = root
        .agent_workspace("default")
        .expect("valid Agent Workspace");
    let ctx = test_helpers::test_context(working.clone());

    let runtime = WorkspaceRuntime::new(
        "default",
        "Default Agent",
        agent_workspace.clone(),
        PlatformProfile::Desktop,
        ctx,
    );

    assert_eq!(runtime.agent_id, "default");
    assert_eq!(runtime.agent_name, "Default Agent");
    assert_eq!(runtime.workspace_dir(), agent_workspace.as_path());
    assert_eq!(
        runtime.lifecycle_state().await,
        WorkspaceLifecycleState::New
    );
    assert!(!runtime.can_accept_requests().await);
    assert_eq!(runtime.active_session_count(), 0);
    assert_eq!(runtime.background_task_count(), 0);

    let health = runtime.health().await;
    assert_eq!(health.agent_id, "default");
    assert_eq!(health.lifecycle, WorkspaceLifecycleState::New);
    assert!(health
        .components
        .values()
        .all(|state| matches!(state, ComponentState::NotInitialized)));
    assert!(
        !runtime.workspace_dir().exists(),
        "directory creation belongs to initialize_core, not construction"
    );
}
