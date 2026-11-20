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
async fn agent_maintenance_preserves_shared_chat_storage_when_index_rows_are_missing() {
    let dir = tempfile::tempdir().unwrap();
    let root = RuaRoot::new(dir.path());
    let agent_id = "test-agent";
    let workspace = root
        .agent_workspace(agent_id)
        .expect("valid Agent Workspace");
    workspace.ensure().unwrap();

    let workspace_dir = workspace.as_path().to_path_buf();
    let chats = root.chat_workspace();
    let store = rua_session::WorkspaceSessionStore::new(chats.as_path());
    let old_age = Duration::from_secs(2 * 24 * 3600);
    std::fs::write(
        workspace_dir.join("agent.json"),
        serde_json::to_vec(&json!({"id": agent_id, "running": {"session_ttl_days": 1}})).unwrap(),
    )
    .unwrap();
    for (chat_id, selected_agent) in [
        ("registered", agent_id),
        ("repairable", agent_id),
        ("other", "another-agent"),
    ] {
        let header = rua_session::ChatSessionHeader::new(rua_session::ChatMetadata {
            chat_id: chat_id.into(),
            agent_id: selected_agent.into(),
            title: chat_id.into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            last_activity_at: "2026-01-01T00:00:00Z".into(),
            pinned: false,
            archived: false,
            preview: None,
        });
        store.save_chat_header(&header).await.unwrap();
        set_modified_age(&store.chat_dir(chat_id), old_age);
    }
    ensure_chat_entry(chats.as_path(), "registered", "console", "default", None)
        .await
        .unwrap();
    let unassigned = store.chat_dir("unassigned");
    std::fs::create_dir_all(&unassigned).unwrap();
    set_modified_age(&unassigned, old_age);
    let expired_agent_file = workspace_dir.join("expired.tmp");
    std::fs::write(&expired_agent_file, b"temporary").unwrap();
    set_modified_age(&expired_agent_file, old_age);

    let ctx = test_helpers::test_context(dir.path().to_path_buf());
    let rt = WorkspaceRuntime::new(agent_id, "Test", workspace, PlatformProfile::Desktop, ctx);
    rt.spawn_maintenance_task(Duration::from_millis(50));

    tokio::time::timeout(Duration::from_secs(2), async {
        while rt.maintenance_tick_count() == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("agent maintenance tick");
    assert!(!expired_agent_file.exists());
    assert!(unassigned.is_dir());
    for chat_id in ["registered", "repairable", "other"] {
        assert!(store.load_chat_header(chat_id).await.unwrap().is_some());
    }
    assert!(rua_session::chat_index::get(chats.as_path(), "registered")
        .await
        .unwrap()
        .is_some());
    assert!(rua_session::chat_index::get(chats.as_path(), "repairable")
        .await
        .unwrap()
        .is_none());

    // These fixtures install no work authority, so a shutdown honestly reports
    // that work termination is unconfirmed. What this test owns is the
    // maintenance loop, so that is what it asserts on.
    let failed = rt.stop().await;
    assert!(
        !failed.iter().any(|component| component == "maintenance"),
        "Stop reported maintenance failures: {failed:?}"
    );
}
