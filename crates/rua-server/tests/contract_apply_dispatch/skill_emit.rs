#[test]
fn skill_update_paths_publish_skill_facts() {
    let src = include_str!("../../src/endpoints/skill_update.rs");

    assert!(
        src.contains("publish_workspace_skill_committed"),
        "workspace skill update must publish a workspace skill fact after writing updated skill files"
    );
    assert!(
        src.contains("publish_skill_pool_committed"),
        "pool skill update must publish a pool skill fact after writing updated skill files"
    );
    assert!(
        src.contains("State(state): State<AppState>"),
        "skill update handlers must extract AppState so updates can append v4 sync changes"
    );
}

#[test]
fn synchronous_hub_install_paths_publish_skill_facts() {
    let src = include_str!("../../src/endpoints/skills_hub.rs");

    assert!(
        src.contains("pub async fn hub_install_handler(\n    State(state): State<AppState>,"),
        "workspace hub install must extract AppState so it can publish a workspace skill fact"
    );
    assert!(
        src.contains("publish_workspace_skill_committed(&state, &agent_id, &result.name).await"),
        "workspace hub install must publish a workspace skill fact after installing files"
    );
    assert!(
        src.contains("pub async fn hub_install_pool_handler(\n    State(state): State<AppState>,"),
        "pool hub install must extract AppState so it can append a pool skill v4 manifest"
    );
    assert!(
        src.contains("let pool_dir = state.rua_root.join(\"skill_pool\");"),
        "pool hub install must use the server data_dir skill pool, not the process HOME default"
    );
    assert!(
        src.contains("publish_skill_pool_committed(&state, &result.name).await"),
        "pool hub install must publish a pool skill fact after installing files"
    );
}

#[test]
fn async_hub_install_paths_have_server_side_sync_publication_watcher() {
    let hub_src = include_str!("../../src/endpoints/skills_hub.rs");
    let skills_src = include_str!("../../src/endpoints/skills.rs");

    assert!(
        hub_src.contains("pub(crate) fn spawn_hub_install_sync_publication_watcher"),
        "async hub installs must have a server-side watcher so sync does not depend on frontend polling"
    );
    assert!(
        hub_src.contains("mark_hub_install_sync_published(agent_id, task_id)"),
        "async hub install publication watcher and status polling must share the same idempotency marker"
    );
    let agent_start = skills_src
        .split("async fn agent_hub_install_start")
        .nth(1)
        .and_then(|tail| tail.split("async fn").next())
        .expect("agent_hub_install_start should exist");
    assert!(
        agent_start.contains("spawn_hub_install_sync_publication_watcher(")
            && agent_start.contains("agent_id,")
            && agent_start.contains("status.task_id.clone()"),
        "agent-scoped async hub install start must spawn the sync publication watcher for that agent"
    );
}
