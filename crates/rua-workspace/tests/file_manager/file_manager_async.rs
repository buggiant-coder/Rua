use std::sync::Arc;
use std::time::{Duration, SystemTime};

use cell_store::FileSystemCellStore;
use rua_agent::cells::AgentCells;
use rua_workspace::FileManager;
use serde_json::json;
use tempfile::TempDir;

fn seed_agent_json(data_dir: &std::path::Path, agent_id: &str, value: serde_json::Value) {
    let dir = data_dir.join("workspaces").join(agent_id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("agent.json"),
        serde_json::to_string_pretty(&value).unwrap(),
    )
    .unwrap();
}

fn make_file_manager(tmp: &TempDir, agent_id: Option<&str>) -> FileManager {
    let data_dir = tmp.path().to_path_buf();
    let workspace_dir = data_dir
        .join("workspaces")
        .join(agent_id.unwrap_or("default"));
    let store = Arc::new(FileSystemCellStore::new());
    let cells = Arc::new(AgentCells::new(store, data_dir.join("workspaces")));
    FileManager::new(
        workspace_dir,
        data_dir,
        agent_id.map(|s| s.to_string()),
        cells,
    )
}

#[tokio::test]
async fn load_agent_config_returns_via_cells() {
    let tmp = TempDir::new().unwrap();
    seed_agent_json(
        tmp.path(),
        "default",
        json!({
            "id": "default",
            "name": "Default Agent",
            "description": "test",
            "workspace_dir": "/tmp/ws",
            "language": "en",
            "system_prompt_files": ["AGENT.md"],
        }),
    );

    let mgr = make_file_manager(&tmp, None);
    let cfg = mgr.load_agent_config().await.unwrap();

    assert_eq!(cfg.id, "default");
    assert_eq!(cfg.name, "Default Agent");
    assert_eq!(cfg.description, "test");
    assert_eq!(cfg.system_prompt_files, vec!["AGENT.md".to_string()]);
}

#[tokio::test]
async fn list_system_prompt_files_returns_current() {
    let tmp = TempDir::new().unwrap();
    seed_agent_json(
        tmp.path(),
        "default",
        json!({
            "id": "default",
            "name": "default",
            "system_prompt_files": ["A.md", "B.md"],
        }),
    );

    let mgr = make_file_manager(&tmp, None);
    let files = mgr.list_system_prompt_files().await.unwrap();
    assert_eq!(files, vec!["A.md".to_string(), "B.md".to_string()]);
}

#[tokio::test]
async fn update_system_prompt_files_persists_through_cells() {
    let tmp = TempDir::new().unwrap();
    seed_agent_json(
        tmp.path(),
        "default",
        json!({
            "id": "default",
            "name": "default",
            "system_prompt_files": [],
        }),
    );

    let mgr = make_file_manager(&tmp, None);

    let updated = mgr
        .update_system_prompt_files(vec!["X.md".into(), "Y.md".into()])
        .await
        .unwrap();
    assert_eq!(updated, vec!["X.md".to_string(), "Y.md".to_string()]);

    let files = mgr.list_system_prompt_files().await.unwrap();
    assert_eq!(files, vec!["X.md".to_string(), "Y.md".to_string()]);

    let raw: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(tmp.path().join("workspaces/default/agent.json")).unwrap(),
    )
    .unwrap();
    let on_disk = raw
        .get("system_prompt_files")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(on_disk, vec!["X.md".to_string(), "Y.md".to_string()]);
}

#[tokio::test]
async fn update_system_prompt_files_external_edit_takes_effect_on_next_read() {
    let tmp = TempDir::new().unwrap();
    seed_agent_json(
        tmp.path(),
        "default",
        json!({
            "id": "default",
            "name": "default",
            "system_prompt_files": ["old.md"],
        }),
    );

    let mgr = make_file_manager(&tmp, None);

    let files = mgr.list_system_prompt_files().await.unwrap();
    assert_eq!(files, vec!["old.md".to_string()]);

    let agent_json_path = tmp.path().join("workspaces/default/agent.json");
    let new_value = json!({
        "id": "default",
        "name": "default",
        "system_prompt_files": ["externally_edited.md"],
    });
    std::fs::write(
        &agent_json_path,
        serde_json::to_string_pretty(&new_value).unwrap(),
    )
    .unwrap();

    let later = SystemTime::now() + Duration::from_secs(2);
    let f = std::fs::OpenOptions::new()
        .write(true)
        .open(&agent_json_path)
        .unwrap();
    f.set_modified(later).unwrap();

    let files = mgr.list_system_prompt_files().await.unwrap();
    assert_eq!(files, vec!["externally_edited.md".to_string()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn file_manager_concurrent_updates_serialize_via_cells() {
    let tmp = TempDir::new().unwrap();
    seed_agent_json(
        tmp.path(),
        "default",
        json!({
            "id": "default",
            "name": "default",
            "system_prompt_files": [],
        }),
    );

    let mgr = make_file_manager(&tmp, None);

    let mut handles = Vec::new();
    for i in 0..10 {
        let m = mgr.clone();
        handles.push(tokio::spawn(async move {
            m.update_system_prompt_files(vec![format!("file_{i}.md")])
                .await
                .unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }

    let final_files = mgr.list_system_prompt_files().await.unwrap();
    assert_eq!(final_files.len(), 1);
    assert!(final_files[0].starts_with("file_"));
    assert!(final_files[0].ends_with(".md"));
}

#[tokio::test]
async fn list_files_returns_only_md_files() {
    let tmp = TempDir::new().unwrap();
    let ws_dir = tmp.path().join("workspaces").join("default");
    let prompts_dir = ws_dir.join("prompts");
    std::fs::create_dir_all(&prompts_dir).unwrap();
    std::fs::write(prompts_dir.join("hello.md"), "# Hello").unwrap();
    std::fs::write(prompts_dir.join("world.md"), "# World").unwrap();
    std::fs::write(prompts_dir.join("ignore.txt"), "not md").unwrap();

    seed_agent_json(
        tmp.path(),
        "default",
        json!({"id": "default", "name": "default"}),
    );
    let mgr = make_file_manager(&tmp, None);

    let files = mgr.list_files().await.unwrap();
    let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, vec!["hello.md", "world.md"]);
}

#[derive(Default)]
struct RecordingPublisher(std::sync::Mutex<Vec<rua_prompts::agent_files::AgentPromptFileChange>>);

#[async_trait::async_trait]
impl rua_prompts::agent_files::AgentPromptFilePublisher for RecordingPublisher {
    async fn publish(&self, changes: Vec<rua_prompts::agent_files::AgentPromptFileChange>) {
        self.0.lock().unwrap().extend(changes);
    }
}

#[tokio::test]
async fn prompt_mutations_publish_canonical_names_and_skip_failed_writes() {
    let tmp = TempDir::new().unwrap();
    let publisher = Arc::new(RecordingPublisher::default());
    let manager = make_file_manager(&tmp, Some("default")).with_publisher(publisher.clone());
    manager
        .write_subdir_file("plans", "today", "text".into())
        .await
        .unwrap();
    manager
        .rename_subdir_file("pLaNs", "today", "tomorrow")
        .await
        .unwrap();
    manager
        .delete_subdir_file("plans", "tomorrow")
        .await
        .unwrap();
    assert!(manager.delete_file("missing").await.is_err());
    let changes = publisher.0.lock().unwrap();
    assert_eq!(
        changes.iter().map(|c| c.path.as_str()).collect::<Vec<_>>(),
        vec![
            "PLANS/today.md",
            "PLANS/today.md",
            "PLANS/tomorrow.md",
            "PLANS/tomorrow.md"
        ]
    );
    assert!(changes[0].content.is_some());
    assert!(changes[1].content.is_none());
    assert!(changes[2].content.is_some());
    assert!(changes[3].content.is_none());
}

#[tokio::test]
async fn independent_managers_serialize_file_commits_and_publication() {
    let tmp = TempDir::new().unwrap();
    let publisher = Arc::new(RecordingPublisher::default());
    let a = make_file_manager(&tmp, Some("default")).with_publisher(publisher.clone());
    let b = make_file_manager(&tmp, Some("default")).with_publisher(publisher.clone());
    let (a_result, b_result) = tokio::join!(
        a.write_file("note", "a".into()),
        b.write_file("note", "b".into())
    );
    a_result.unwrap();
    b_result.unwrap();
    let changes = publisher.0.lock().unwrap();
    assert_eq!(changes.len(), 2);
    assert!(changes[1].version.modified_ms > changes[0].version.modified_ms);
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("workspaces/default/prompts/note.md")).unwrap(),
        changes[1].content.as_deref().unwrap()
    );
}
