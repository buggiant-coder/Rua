use std::sync::Arc;

use cell_store::FileSystemCellStore;
use rua_skills::{SkillMetadata, SkillRequirements, SkillSource, WorkspaceSkillCells};
use serde_json::json;
use tempfile::TempDir;

fn seed_skill_json(tmp: &TempDir, agent_id: &str) {
    let dir = tmp.path().join("workspaces").join(agent_id);
    std::fs::create_dir_all(&dir).expect("workspace dir");
    std::fs::write(
        dir.join("skill.json"),
        serde_json::to_string_pretty(&json!({
            "schema_version": "workspace-skill-manifest.v1",
            "version": 1,
            "skills": {
                "review": {
                    "enabled": false,
                    "channels": ["all"],
                    "source": "customized",
                    "config": {},
                    "metadata": {
                        "name": "review",
                        "description": "review docs",
                        "version_text": "",
                        "commit_text": "",
                        "signature": "",
                        "source": "customized",
                        "protected": false,
                        "requirements": {"require_bins": [], "require_envs": []},
                        "updated_at": "",
                        "tags": [],
                        "emoji": ""
                    },
                    "requirements": {"require_bins": [], "require_envs": []},
                    "updated_at": ""
                }
            }
        }))
        .unwrap(),
    )
    .expect("write skill.json");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_skill_cells_round_trip_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    let cells = WorkspaceSkillCells::new(
        Arc::new(FileSystemCellStore::new()),
        tmp.path().join("workspaces"),
    );
    let cell = cells.cell("agent-a");

    let mut manifest = rua_skills::WorkspaceManifest::default();
    manifest.skills.insert(
        "review".to_string(),
        rua_skills::WorkspaceSkillEntry {
            enabled: true,
            channels: vec!["console".to_string()],
            source: SkillSource::Customized,
            config: serde_json::Map::new(),
            metadata: SkillMetadata {
                name: "review".to_string(),
                description: "review docs".to_string(),
                version_text: String::new(),
                commit_text: String::new(),
                signature: String::new(),
                source: SkillSource::Customized,
                protected: false,
                requirements: SkillRequirements::default(),
                updated_at: String::new(),
                tags: vec![],
                emoji: String::new(),
                extra: serde_json::Map::new(),
            },
            requirements: SkillRequirements::default(),
            updated_at: String::new(),
            extra: serde_json::Map::new(),
        },
    );

    cell.write(manifest).await.unwrap();

    let fresh_cells = WorkspaceSkillCells::new(
        Arc::new(FileSystemCellStore::new()),
        tmp.path().join("workspaces"),
    );
    let read = fresh_cells.cell("agent-a").read().await.unwrap().unwrap();
    assert!(read.skills["review"].enabled);
    assert_eq!(read.skills["review"].channels, vec!["console".to_string()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_skill_cells_pick_up_external_edits() {
    let tmp = tempfile::tempdir().unwrap();
    seed_skill_json(&tmp, "agent-a");

    let cells = WorkspaceSkillCells::new(
        Arc::new(FileSystemCellStore::new()),
        tmp.path().join("workspaces"),
    );
    let cell = cells.cell("agent-a");
    assert!(!cell.read().await.unwrap().unwrap().skills["review"].enabled);

    let path = tmp.path().join("workspaces/agent-a/skill.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["skills"]["review"]["enabled"] = json!(true);
    std::fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).unwrap();

    let read = cell.read().await.unwrap().unwrap();
    assert!(read.skills["review"].enabled);
}
