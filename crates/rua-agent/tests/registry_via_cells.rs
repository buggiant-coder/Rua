use std::path::PathBuf;
use std::sync::Arc;

#[path = "registry_via_cells/profile_store.rs"]
mod profile_store;

use cell_store::FileSystemCellStore;
use rua_agent::profile_store::AgentProfileStore;
use rua_agent::registry::AgentRegistry;
use rua_agent::{AgentCells, AgentProfile, FileAgentRegistry};
use tempfile::TempDir;

fn build_registry(tmp: &TempDir) -> (Arc<AgentCells>, Arc<AgentProfileStore>, FileAgentRegistry) {
    let store = Arc::new(FileSystemCellStore::new());
    let cells = Arc::new(AgentCells::new(
        Arc::clone(&store),
        tmp.path().join("workspaces"),
    ));
    let profile_store = Arc::new(AgentProfileStore::new(tmp.path().to_path_buf()));
    let registry =
        FileAgentRegistry::with_cells(Arc::clone(&cells), Arc::clone(&profile_store), "default");
    (cells, profile_store, registry)
}

fn agent_json_path(tmp: &TempDir, id: &str) -> PathBuf {
    tmp.path().join("workspaces").join(id).join("agent.json")
}

fn write_default_baseline(tmp: &TempDir) {
    let path = agent_json_path(tmp, "default");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&serde_json::json!({
            "id": "default",
            "name": "default",
            "language": "zh",
            "system_prompt_files": ["DEFAULT_AGENT.md"]
        }))
        .unwrap(),
    )
    .unwrap();
}

#[tokio::test]
async fn upsert_persists_to_disk_immediately() {
    let tmp = TempDir::new().unwrap();
    write_default_baseline(&tmp);
    let (_cells, _store, registry) = build_registry(&tmp);

    let profile = AgentProfile::new("alpha", "Alpha");
    registry.upsert(profile).await.expect("upsert");

    let path = agent_json_path(&tmp, "alpha");
    let raw = std::fs::read_to_string(&path).expect("agent.json present");
    let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(json["id"], "alpha");
    assert_eq!(json["name"], "Alpha");

    let got = registry.get_agent("alpha").await.expect("get_agent");
    assert_eq!(got.profile.id, "alpha");
    assert_eq!(got.profile.name, "Alpha");
}

#[tokio::test]
async fn reload_picks_up_external_disk_changes() {
    let tmp = TempDir::new().unwrap();
    write_default_baseline(&tmp);
    let (cells, _store, registry) = build_registry(&tmp);

    let mut profile = AgentProfile::new("zeta", "Zeta-Initial");
    profile.language = "en".to_string();
    registry.upsert(profile).await.expect("upsert");

    let got = registry.get_agent("zeta").await.expect("get_agent");
    assert_eq!(got.profile.name, "Zeta-Initial");

    let path = agent_json_path(&tmp, "zeta");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&serde_json::json!({
            "id": "zeta",
            "name": "Zeta-Externally-Edited",
            "language": "ja",
            "running": {"max_iters": 77}
        }))
        .unwrap(),
    )
    .unwrap();

    bump_mtime(&path);

    let _ = cells;

    registry.reload().await.expect("reload");
    let got = registry.get_agent("zeta").await.expect("get_agent");
    assert_eq!(
        got.profile.name, "Zeta-Externally-Edited",
        "reload must surface external disk edits — pre-feat-50 returned 'Zeta-Initial'"
    );
    assert_eq!(got.profile.language, "ja");
    assert_eq!(got.running.max_iters, 77);
}

fn bump_mtime(path: &std::path::Path) {
    use std::time::{Duration, SystemTime};
    let later = SystemTime::now() + Duration::from_secs(2);
    let f = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open for mtime bump");
    f.set_modified(later).expect("set_modified");
}

#[tokio::test]
async fn remove_disappears_from_list_immediately() {
    let tmp = TempDir::new().unwrap();
    write_default_baseline(&tmp);
    let (_cells, _store, registry) = build_registry(&tmp);

    registry
        .upsert(AgentProfile::new("kept", "Kept"))
        .await
        .expect("upsert kept");
    registry
        .upsert(AgentProfile::new("vanishing", "Vanishing"))
        .await
        .expect("upsert vanishing");

    let listed_before = registry.list_agents().await.expect("list");
    let ids_before: Vec<String> = listed_before.iter().map(|p| p.profile.id.clone()).collect();
    assert!(ids_before.contains(&"kept".to_string()));
    assert!(ids_before.contains(&"vanishing".to_string()));

    let removed = registry.remove("vanishing").await.expect("remove");
    assert!(removed.is_some());
    assert!(!agent_json_path(&tmp, "vanishing").exists());

    let listed_after = registry.list_agents().await.expect("list");
    let ids_after: Vec<String> = listed_after.iter().map(|p| p.profile.id.clone()).collect();
    assert!(ids_after.contains(&"kept".to_string()));
    assert!(
        !ids_after.contains(&"vanishing".to_string()),
        "removed agent must disappear from next list; got {ids_after:?}"
    );
}

#[tokio::test]
async fn target_agent_does_not_inherit_default_prompt_or_model() {
    let tmp = TempDir::new().unwrap();
    let default_path = agent_json_path(&tmp, "default");
    std::fs::create_dir_all(default_path.parent().unwrap()).unwrap();
    std::fs::write(
        &default_path,
        serde_json::to_string_pretty(&serde_json::json!({
            "id": "default",
            "name": "default-baseline",
            "language": "zh",
            "system_prompt_files": ["BASE.md", "TOOLS.md"],
            "default_model": {"provider_id": "openai", "model": "gpt-4"}
        }))
        .unwrap(),
    )
    .unwrap();

    let target_path = agent_json_path(&tmp, "overlay-target");
    std::fs::create_dir_all(target_path.parent().unwrap()).unwrap();
    std::fs::write(
        &target_path,
        serde_json::to_string_pretty(&serde_json::json!({
            "id": "overlay-target",
            "name": "Overlay Target Display",
            "language": "en"
        }))
        .unwrap(),
    )
    .unwrap();

    let (cells, profile_store, _registry) = build_registry(&tmp);
    let merged = profile_store
        .load_agent_config_via_cells(&cells, Some("overlay-target"))
        .await
        .expect("merge load");
    assert_eq!(merged.id, "overlay-target");
    assert_eq!(merged.name, "Overlay Target Display");

    assert_eq!(merged.language, "en");

    assert!(
        merged.system_prompt_files.is_empty(),
        "target Agent owns its Prompt selection"
    );
    assert!(
        merged.default_model.is_none(),
        "another Agent's model is not a default overlay"
    );
}
