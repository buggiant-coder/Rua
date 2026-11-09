use axum::http::{Method, StatusCode};
use rua_skills::{write_install_meta, InstallMeta};
use serde_json::{json, Value};

use super::support::{build_app, request_json, skill_md};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pool_refresh_builtin_and_config_shapes_are_stable() {
    let test = build_app();
    let pool_skill = "contract-pool-skill";

    let pool = request_json(
        &test.app,
        Method::GET,
        "/api/skills/pool",
        Some("skills_alpha"),
        None,
        StatusCode::OK,
    )
    .await;
    assert!(pool.as_array().is_some());

    let refresh = request_json(
        &test.app,
        Method::POST,
        "/api/skills/pool/refresh",
        None,
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(refresh["success"], true);
    assert!(refresh["count"].as_u64().is_some());

    let sources = request_json(
        &test.app,
        Method::GET,
        "/api/skills/pool/builtin-sources",
        None,
        None,
        StatusCode::OK,
    )
    .await;
    let sources = sources.as_array().expect("builtin sources array");
    assert!(!sources.is_empty(), "packaged builtins should be visible");
    assert!(sources[0]["name"].as_str().is_some());
    assert!(sources[0]["status"].as_str().is_some());

    let notice = request_json(
        &test.app,
        Method::GET,
        "/api/skills/pool/builtin-notice",
        Some("skills_alpha"),
        None,
        StatusCode::OK,
    )
    .await;
    assert!(notice["has_updates"].as_bool().is_some());
    assert!(notice["fingerprint"].as_str().is_some());

    let created = request_json(
        &test.app,
        Method::POST,
        "/api/skills/pool/create",
        None,
        Some(json!({
            "name": pool_skill,
            "description": "pool contract skill",
            "content": skill_md(pool_skill, "pool contract skill"),
        })),
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(created["name"], pool_skill);

    let tags = request_json(
        &test.app,
        Method::PUT,
        &format!("/api/skills/pool/{pool_skill}/tags"),
        Some("skills_alpha"),
        Some(json!({"tags": ["contract", "qwenpaw"]})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(tags["success"], true);

    let config = request_json(
        &test.app,
        Method::PUT,
        &format!("/api/skills/pool/{pool_skill}/config"),
        None,
        Some(json!({"config": {"source": "contract"}})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(config["config"]["source"], "contract");

    let fetched = request_json(
        &test.app,
        Method::GET,
        &format!("/api/skills/pool/{pool_skill}/config"),
        Some("skills_alpha"),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(fetched["config"]["source"], "contract");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn market_provider_and_unknown_search_contract_matches_qwenpaw_intent() {
    let test = build_app();

    let providers = request_json(
        &test.app,
        Method::GET,
        "/api/market/providers",
        Some("skills_alpha"),
        None,
        StatusCode::OK,
    )
    .await;
    let providers = providers.as_array().expect("providers array");
    assert!(
        !providers.is_empty(),
        "market provider registry must be visible"
    );
    for provider in providers {
        assert!(provider["key"].as_str().is_some());
        assert!(provider["label"].as_str().is_some());
        assert!(provider["available"].as_bool().is_some());
    }

    let detail = request_json(
        &test.app,
        Method::POST,
        "/api/market/search",
        Some("skills_alpha"),
        Some(json!({
            "query": "anything",
            "provider_pages": {"contract_unknown_provider": 1},
            "limit": 5,
            "lang": "en",
        })),
        StatusCode::BAD_REQUEST,
    )
    .await;
    assert!(detail["detail"]
        .as_str()
        .unwrap_or_default()
        .contains("contract_unknown_provider"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn installed_market_meta_surfaces_in_workspace_and_pool_lists() {
    let test = build_app();
    let meta = InstallMeta {
        installed_from: "clawhub".to_string(),
        source_url: "https://clawhub.ai/meta-skill".to_string(),
        installed_at: "2026-06-18T00:00:00Z".to_string(),
        updated_at: None,
        installed_version: "1.2.3".to_string(),
        content_hash: "hash".to_string(),
        market_meta: Some(json!({
            "author": "Market Author",
            "icon_url": "https://example.test/icon.png",
            "slug": "meta-skill"
        })),
    };

    let workspace_skill_dir = test
        .tmp
        .path()
        .join("workspaces")
        .join("skills_alpha")
        .join("skills")
        .join("meta-skill");
    std::fs::create_dir_all(&workspace_skill_dir).expect("workspace skill dir");
    std::fs::write(
        workspace_skill_dir.join("SKILL.md"),
        skill_md("meta-skill", "workspace meta"),
    )
    .expect("workspace skill md");
    write_install_meta(&workspace_skill_dir, &meta).expect("workspace meta");

    request_json(
        &test.app,
        Method::POST,
        "/api/agents/skills_alpha/skills/refresh",
        Some("skills_alpha"),
        None,
        StatusCode::OK,
    )
    .await;
    let workspace_manifest_path = test
        .tmp
        .path()
        .join("workspaces")
        .join("skills_alpha")
        .join("skill.json");
    let mut workspace_manifest: Value = serde_json::from_slice(
        &std::fs::read(&workspace_manifest_path).expect("workspace manifest"),
    )
    .expect("workspace manifest json");
    workspace_manifest["skills"]["meta-skill"]
        .as_object_mut()
        .expect("workspace skill entry")
        .remove("install_meta");
    std::fs::write(
        &workspace_manifest_path,
        serde_json::to_vec_pretty(&workspace_manifest).expect("workspace manifest bytes"),
    )
    .expect("rewrite workspace manifest");

    let workspace = request_json(
        &test.app,
        Method::GET,
        "/api/agents/skills_alpha/skills",
        Some("skills_alpha"),
        None,
        StatusCode::OK,
    )
    .await;
    let workspace_meta = workspace
        .as_array()
        .and_then(|items| items.iter().find(|item| item["name"] == "meta-skill"))
        .and_then(|item| item.get("install_meta"))
        .expect("workspace install_meta");
    assert_eq!(
        workspace_meta["market_meta"]["icon_url"],
        "https://example.test/icon.png"
    );
    let detail = request_json(
        &test.app,
        Method::GET,
        "/api/agents/skills_alpha/skills/meta-skill",
        Some("skills_alpha"),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        detail["install_meta"]["source_url"],
        "https://clawhub.ai/meta-skill"
    );

    let pool_skill_dir = test.tmp.path().join("skill_pool").join("meta-skill");
    std::fs::create_dir_all(&pool_skill_dir).expect("pool skill dir");
    std::fs::write(
        pool_skill_dir.join("SKILL.md"),
        skill_md("meta-skill", "pool meta"),
    )
    .expect("pool skill md");
    write_install_meta(&pool_skill_dir, &meta).expect("pool meta");

    request_json(
        &test.app,
        Method::POST,
        "/api/skills/pool/refresh",
        Some("skills_alpha"),
        None,
        StatusCode::OK,
    )
    .await;
    let pool = request_json(
        &test.app,
        Method::GET,
        "/api/skills/pool",
        Some("skills_alpha"),
        None,
        StatusCode::OK,
    )
    .await;
    let pool_meta = pool
        .as_array()
        .and_then(|items| items.iter().find(|item| item["name"] == "meta-skill"))
        .and_then(|item| item.get("install_meta"))
        .expect("pool install_meta");
    assert_eq!(
        pool_meta["market_meta"]["icon_url"],
        "https://example.test/icon.png"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upload_workspace_market_skill_to_pool_preserves_install_meta() {
    let test = build_app();
    let workspace_skill_dir = test
        .tmp
        .path()
        .join("workspaces")
        .join("skills_alpha")
        .join("skills")
        .join("upload-meta-skill");
    std::fs::create_dir_all(&workspace_skill_dir).expect("workspace skill dir");
    std::fs::write(
        workspace_skill_dir.join("SKILL.md"),
        skill_md("upload-meta-skill", "workspace upload meta"),
    )
    .expect("workspace skill md");
    write_install_meta(
        &workspace_skill_dir,
        &InstallMeta {
            installed_from: "clawhub".to_string(),
            source_url: "https://clawhub.ai/upload-meta-skill".to_string(),
            installed_at: "2026-06-18T00:00:00Z".to_string(),
            updated_at: Some("2026-06-18T01:00:00Z".to_string()),
            installed_version: "1.2.3".to_string(),
            content_hash: "hash".to_string(),
            market_meta: Some(json!({
                "author": "Market Author",
                "icon_url": "https://example.test/upload-icon.png",
                "slug": "upload-meta-skill",
                "source": "clawhub"
            })),
        },
    )
    .expect("workspace meta");

    let uploaded = request_json(
        &test.app,
        Method::POST,
        "/api/skills/pool/upload",
        Some("skills_alpha"),
        Some(json!({
            "workspace_id": "skills_alpha",
            "skill_name": "upload-meta-skill"
        })),
        StatusCode::OK,
    )
    .await;
    assert_eq!(uploaded["success"], true);
    assert_eq!(uploaded["name"], "upload-meta-skill");

    let pool = request_json(
        &test.app,
        Method::GET,
        "/api/skills/pool",
        Some("skills_alpha"),
        None,
        StatusCode::OK,
    )
    .await;
    let pool_meta = pool
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|item| item["name"] == "upload-meta-skill")
        })
        .and_then(|item| item.get("install_meta"))
        .expect("pool install_meta after workspace upload");
    assert_eq!(pool_meta["installed_from"], "clawhub");
    assert_eq!(pool_meta["installed_version"], "1.2.3");
    assert_eq!(
        pool_meta["market_meta"]["icon_url"],
        "https://example.test/upload-icon.png"
    );
    assert_eq!(pool_meta["market_meta"]["author"], "Market Author");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pool_check_update_reads_state_pool_dir() {
    let test = build_app();
    let pool_skill_dir = test.tmp.path().join("skill_pool").join("state-meta-skill");
    std::fs::create_dir_all(&pool_skill_dir).expect("pool skill dir");
    std::fs::write(
        pool_skill_dir.join("SKILL.md"),
        skill_md("state-meta-skill", "pool meta"),
    )
    .expect("pool skill md");
    write_install_meta(
        &pool_skill_dir,
        &InstallMeta {
            installed_from: "clawhub".to_string(),
            source_url: "file:///not-supported".to_string(),
            installed_at: "2026-06-18T00:00:00Z".to_string(),
            updated_at: None,
            installed_version: "1.2.3".to_string(),
            content_hash: "hash".to_string(),
            market_meta: None,
        },
    )
    .expect("pool meta");

    let detail = request_json(
        &test.app,
        Method::POST,
        "/api/skills/pool/state-meta-skill/check-update",
        Some("skills_alpha"),
        None,
        StatusCode::BAD_GATEWAY,
    )
    .await;
    assert!(detail["detail"]
        .as_str()
        .unwrap_or_default()
        .contains("source unreachable"));
}
