use super::test_helpers;

use serde_json::json;
use std::time::Duration;

use rua_workspace::chats_store::ensure_chat_entry;
use rua_workspace::lifecycle::PlatformProfile;
use rua_workspace::root::RuaRoot;
use rua_workspace::runtime::WorkspaceRuntime;

fn set_modified_age(path: &std::path::Path, age: Duration) {
    let new_mtime = std::time::SystemTime::now()
        .checked_sub(age)
        .expect("age within range");
    let f = std::fs::File::open(path).expect("open for set_times");
    let times = std::fs::FileTimes::new()
        .set_modified(new_mtime)
        .set_accessed(new_mtime);
    f.set_times(times).expect("set mtime");
}

#[tokio::test]
async fn maintenance_configuration_change_replaces_the_loop_without_replacing_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let root = RuaRoot::new(dir.path());
    let workspace = test_helpers::agent_workspace(&root, "maintenance-change");
    workspace.ensure().unwrap();
    let context = test_helpers::test_context(dir.path().to_path_buf());
    let rt = WorkspaceRuntime::new(
        "maintenance-change",
        "Maintenance",
        workspace,
        PlatformProfile::Desktop,
        context.clone(),
    );
    rt.initialize_for_platform().await.unwrap();
    context
        .agent_cells
        .section::<serde_json::Value>("maintenance-change", "maintenance_interval_hours")
        .write(json!(0.00001))
        .await
        .unwrap();
    rt.apply_maintenance_configuration().await.unwrap();
    rt.apply_maintenance_configuration().await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while rt.maintenance_tick_count() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let failures = rt.stop().await;
    assert!(!failures.contains(&"maintenance".to_string()));
    let stopped = rt.maintenance_tick_count();
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(stopped, rt.maintenance_tick_count());
}

#[tokio::test]
async fn maintenance_task_fires_periodically() {
    let dir = tempfile::tempdir().unwrap();
    let root = RuaRoot::new(dir.path())
        .agent_workspace("test-agent")
        .expect("valid Agent Workspace");
    root.ensure().unwrap();

    let ctx = test_helpers::test_context(dir.path().to_path_buf());
    let rt = WorkspaceRuntime::new("test-agent", "Test", root, PlatformProfile::Desktop, ctx);
    rt.spawn_maintenance_task(Duration::from_millis(100));

    tokio::time::sleep(Duration::from_millis(350)).await;

    let count = rt.maintenance_tick_count();
    assert!(count >= 2, "Expected >= 2 maintenance ticks, got {count}");

    // These fixtures install no work authority, so a shutdown honestly reports
    // that work termination is unconfirmed. What this test owns is the
    // maintenance loop, so that is what it asserts on.
    let failed = rt.stop().await;
    assert!(
        !failed.iter().any(|component| component == "maintenance"),
        "Stop reported maintenance failures: {failed:?}"
    );
}

#[tokio::test]
async fn maintenance_task_aborts_on_stop() {
    let dir = tempfile::tempdir().unwrap();
    let root = RuaRoot::new(dir.path())
        .agent_workspace("test-agent")
        .expect("valid Agent Workspace");
    root.ensure().unwrap();

    let ctx = test_helpers::test_context(dir.path().to_path_buf());
    let rt = WorkspaceRuntime::new("test-agent", "Test", root, PlatformProfile::Desktop, ctx);
    rt.spawn_maintenance_task(Duration::from_millis(50));

    tokio::time::sleep(Duration::from_millis(120)).await;

    let failed = rt.stop().await;
    assert!(
        !failed.iter().any(|component| component == "maintenance"),
        "Stop reported maintenance failures: {failed:?}"
    );

    let count_at_stop = rt.maintenance_tick_count();

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        rt.maintenance_tick_count(),
        count_at_stop,
        "Maintenance continued after stop"
    );
}

#[tokio::test]
async fn maintenance_uses_running_session_ttl_days_for_orphan_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let root = RuaRoot::new(dir.path());
    let agent_id = "test-agent";
    let workspace = root
        .agent_workspace(agent_id)
        .expect("valid Agent Workspace");
    workspace.ensure().unwrap();

    let workspace_dir = workspace.as_path().to_path_buf();
    let sessions_dir = workspace.sessions_dir();
    std::fs::create_dir_all(&sessions_dir).unwrap();
    std::fs::write(
        workspace_dir.join("agent.json"),
        serde_json::to_vec_pretty(&json!({
            "id": agent_id,
            "name": "Test",
            "running": {
                "session_ttl_days": 1
            }
        }))
        .unwrap(),
    )
    .unwrap();
    ensure_chat_entry(&workspace_dir, "live-session", "console", "default", None)
        .await
        .unwrap();

    let orphan_dir = sessions_dir.join("old-orphan");
    std::fs::create_dir_all(orphan_dir.join("messages")).unwrap();
    set_modified_age(&orphan_dir, Duration::from_secs(2 * 24 * 3600));

    let ctx = test_helpers::test_context(dir.path().to_path_buf());
    let rt = WorkspaceRuntime::new(agent_id, "Test", workspace, PlatformProfile::Desktop, ctx);
    rt.spawn_maintenance_task(Duration::from_millis(50));

    tokio::time::sleep(Duration::from_millis(180)).await;

    assert!(
        !orphan_dir.exists(),
        "maintenance should use running.session_ttl_days instead of the hard-coded default"
    );

    // These fixtures install no work authority, so a shutdown honestly reports
    // that work termination is unconfirmed. What this test owns is the
    // maintenance loop, so that is what it asserts on.
    let failed = rt.stop().await;
    assert!(
        !failed.iter().any(|component| component == "maintenance"),
        "Stop reported maintenance failures: {failed:?}"
    );
}
