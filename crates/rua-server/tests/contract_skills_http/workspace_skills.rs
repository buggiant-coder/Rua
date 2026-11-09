use axum::http::{Method, StatusCode};
use cell_store::FileSystemCellStore;
use rua_skills::WorkspaceSkillCells;
use serde_json::json;
use std::sync::Arc;

use super::support::{build_app, request_json, skill_md, skill_names};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skill_file_reads_follow_path_agent_even_with_conflicting_header() {
    let test = build_app();
    for agent in ["skills_alpha", "skills_beta"] {
        request_json(
            &test.app,
            Method::POST,
            &format!("/api/agents/{agent}/skills"),
            None,
            Some(json!({"name":"same-skill", "content":skill_md("same-skill", "scoped files")})),
            StatusCode::CREATED,
        )
        .await;
        let references = test
            .tmp
            .path()
            .join("workspaces")
            .join(agent)
            .join("skills/same-skill/references");
        std::fs::create_dir_all(&references).unwrap();
        std::fs::write(references.join("guide.md"), agent).unwrap();
    }
    let file = request_json(
        &test.app,
        Method::GET,
        "/api/agents/skills_beta/skills/same-skill/files/references/guide.md",
        Some("skills_alpha"),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(file["content"], "skills_beta");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_skill_lifecycle_is_isolated() {
    let test = build_app();
    let root_skill = "contract-skill-root";
    let scoped_skill = "contract-skill-scoped";

    let created = request_json(
        &test.app,
        Method::POST,
        "/api/agents/skills_alpha/skills",
        Some("skills_alpha"),
        Some(json!({
            "name": root_skill,
            "description": "root scoped contract skill",
            "content": skill_md(root_skill, "root scoped contract skill"),
            "enable": false,
            "config": {"threshold": 3},
        })),
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(created["name"], root_skill);
    assert_eq!(created["enabled"], false);

    let alpha_list = request_json(
        &test.app,
        Method::GET,
        "/api/agents/skills_alpha/skills",
        Some("skills_alpha"),
        None,
        StatusCode::OK,
    )
    .await;
    assert!(skill_names(&alpha_list).contains(&root_skill.to_string()));

    let beta_list = request_json(
        &test.app,
        Method::GET,
        "/api/agents/skills_beta/skills",
        Some("skills_beta"),
        None,
        StatusCode::OK,
    )
    .await;
    assert!(!skill_names(&beta_list).contains(&root_skill.to_string()));

    let scoped_created = request_json(
        &test.app,
        Method::POST,
        "/api/agents/skills_beta/skills",
        None,
        Some(json!({
            "name": scoped_skill,
            "description": "path scoped contract skill",
            "content": skill_md(scoped_skill, "path scoped contract skill"),
            "enable": true,
        })),
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(scoped_created["name"], scoped_skill);
    assert_eq!(scoped_created["enabled"], true);

    let scoped_detail = request_json(
        &test.app,
        Method::GET,
        &format!("/api/agents/skills_beta/skills/{scoped_skill}"),
        None,
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(scoped_detail["name"], scoped_skill);
    assert!(scoped_detail["content"]
        .as_str()
        .unwrap_or("")
        .contains(scoped_skill));

    let disable = request_json(
        &test.app,
        Method::POST,
        &format!("/api/agents/skills_beta/skills/{scoped_skill}/disable"),
        None,
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(disable["success"], true);
    assert_eq!(disable["disabled"], true);
    assert_eq!(disable["updated_workspaces"][0], "skills_beta");

    let enable = request_json(
        &test.app,
        Method::POST,
        &format!("/api/agents/skills_beta/skills/{scoped_skill}/enable"),
        None,
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(enable["success"], true);
    assert_eq!(enable["enabled"], true);
    assert_eq!(enable["updated_workspaces"][0], "skills_beta");

    let delete = request_json(
        &test.app,
        Method::DELETE,
        &format!("/api/agents/skills_beta/skills/{scoped_skill}"),
        None,
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(delete["deleted"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn batch_toggles_accept_array_and_object_bodies() {
    let test = build_app();
    let skill_a = "contract-skill-batch-a";
    let skill_b = "contract-skill-batch-b";

    for skill in [skill_a, skill_b] {
        request_json(
            &test.app,
            Method::POST,
            "/api/agents/skills_alpha/skills",
            Some("skills_alpha"),
            Some(json!({
                "name": skill,
                "description": "batch contract skill",
                "content": skill_md(skill, "batch contract skill"),
                "enable": false,
            })),
            StatusCode::CREATED,
        )
        .await;
    }

    let enable = request_json(
        &test.app,
        Method::POST,
        "/api/agents/skills_alpha/skills/batch-enable",
        Some("skills_alpha"),
        Some(json!([skill_a, skill_b])),
        StatusCode::OK,
    )
    .await;
    assert_eq!(enable["results"][skill_a]["success"], true);
    assert_eq!(enable["results"][skill_a]["enabled"], true);
    assert_eq!(enable["results"][skill_b]["success"], true);

    let disable = request_json(
        &test.app,
        Method::POST,
        "/api/agents/skills_alpha/skills/batch-disable",
        None,
        Some(json!({"names": [skill_a, skill_b]})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(disable["results"][skill_a]["success"], true);
    assert_eq!(disable["results"][skill_a]["disabled"], true);
    assert_eq!(disable["results"][skill_b]["success"], true);

    let delete = request_json(
        &test.app,
        Method::POST,
        "/api/agents/skills_alpha/skills/batch-delete",
        None,
        Some(json!({"names": [skill_a, skill_b]})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(delete["success"], true);

    let list = request_json(
        &test.app,
        Method::GET,
        "/api/agents/skills_alpha/skills",
        Some("skills_alpha"),
        None,
        StatusCode::OK,
    )
    .await;
    let names = skill_names(&list);
    assert!(!names.contains(&skill_a.to_string()));
    assert!(!names.contains(&skill_b.to_string()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skill_config_and_channels_roundtrip() {
    let test = build_app();
    let skill = "contract-skill-config";

    request_json(
        &test.app,
        Method::POST,
        "/api/agents/skills_alpha/skills",
        None,
        Some(json!({
            "name": skill,
            "description": "config contract skill",
            "content": skill_md(skill, "config contract skill"),
            "enable": false,
        })),
        StatusCode::CREATED,
    )
    .await;

    let channels = request_json(
        &test.app,
        Method::PUT,
        &format!("/api/agents/skills_alpha/skills/{skill}/channels"),
        None,
        Some(json!({"channels": ["console", "dingtalk"]})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(channels["success"], true);

    let updated = request_json(
        &test.app,
        Method::PUT,
        &format!("/api/agents/skills_alpha/skills/{skill}/config"),
        None,
        Some(json!({"config": {"mode": "strict", "limit": 2}})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(updated["config"]["mode"], "strict");
    assert_eq!(updated["config"]["limit"], 2);

    let fetched = request_json(
        &test.app,
        Method::GET,
        &format!("/api/agents/skills_alpha/skills/{skill}/config"),
        None,
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(fetched["config"]["mode"], "strict");

    let deleted = request_json(
        &test.app,
        Method::DELETE,
        &format!("/api/agents/skills_alpha/skills/{skill}/config"),
        None,
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(deleted["success"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enabling_skill_survives_fresh_cell_store_restart_read() {
    let test = build_app();
    let skill = "contract-skill-enable-persist";

    request_json(
        &test.app,
        Method::POST,
        "/api/agents/skills_alpha/skills",
        None,
        Some(json!({
            "name": skill,
            "description": "enable persistence contract skill",
            "content": skill_md(skill, "enable persistence contract skill"),
            "enable": false,
        })),
        StatusCode::CREATED,
    )
    .await;

    request_json(
        &test.app,
        Method::POST,
        &format!("/api/agents/skills_alpha/skills/{skill}/enable"),
        None,
        None,
        StatusCode::OK,
    )
    .await;

    let fresh_cells = WorkspaceSkillCells::new(
        Arc::new(FileSystemCellStore::new()),
        test.tmp.path().join("workspaces"),
    );
    let manifest = fresh_cells
        .cell("skills_alpha")
        .read()
        .await
        .expect("fresh cells read")
        .expect("skill manifest persisted");
    assert!(
        manifest.skills[skill].enabled,
        "enabled flag must be persisted to skill.json through WorkspaceSkillCells"
    );
}
