use axum::http::{Method, StatusCode};
use serde_json::{json, Value};
use std::path::Path;

use super::support::{build_app, read_json, request};

const PROFILE_TOP_LEVEL_KEYS: &[&str] = &[
    "channels",
    "mcp",
    "heartbeat",
    "running",
    "llm_routing",
    "system_prompt_files",
    "tools",
    "plan",
];

fn agent_ids(payload: &Value) -> Vec<String> {
    payload["agents"]
        .as_array()
        .expect("agents array")
        .iter()
        .map(|row| row["id"].as_str().expect("agent id").to_string())
        .collect()
}

async fn create_agent(app: &axum::Router, id: &str, name: &str) -> Value {
    let resp = request(
        app,
        Method::POST,
        "/api/agents",
        Some(json!({
            "id": id,
            "name": name,
            "description": ""
        })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    read_json(resp).await
}

async fn delete_agent(app: &axum::Router, id: &str) {
    let resp = request(app, Method::DELETE, &format!("/api/agents/{id}"), None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(read_json(resp).await["success"], true);
}

fn read_config_agent_order(data_dir: &Path) -> Vec<String> {
    let value: Value =
        serde_json::from_slice(&std::fs::read(data_dir.join("config.json")).expect("config"))
            .expect("config json");
    value["agents"]["agent_order"]
        .as_array()
        .expect("agent_order array")
        .iter()
        .map(|id| id.as_str().expect("agent id").to_string())
        .collect()
}

fn read_agent_json(data_dir: &Path, agent_id: &str) -> Value {
    serde_json::from_slice(
        &std::fs::read(
            data_dir
                .join("workspaces")
                .join(agent_id)
                .join("agent.json"),
        )
        .expect("agent json"),
    )
    .expect("agent json parse")
}

fn write_config_agent_order(data_dir: &Path, order: &[&str]) {
    let path = data_dir.join("config.json");
    let mut value: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("config")).expect("config json");
    value["agents"]["agent_order"] = json!(order);
    rua_config::atomic_write_json(&path, &value).expect("write config");
}

fn without_ids(ids: &[String], excluded: &[&str]) -> Vec<String> {
    ids.iter()
        .filter(|id| !excluded.iter().any(|excluded_id| excluded_id == id))
        .cloned()
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_create_get_delete_matches_qwenpaw_contract() {
    let app = build_app();
    let agent_id = "integ_agents_01";

    let list_resp = request(&app.app, Method::GET, "/api/agents", None).await;
    assert_eq!(list_resp.status(), StatusCode::OK);
    let list_payload = read_json(list_resp).await;
    assert!(list_payload["agents"].is_array());
    assert!(
        agent_ids(&list_payload).iter().any(|id| id == "default"),
        "default agent must be present in list"
    );

    let created = create_agent(&app.app, agent_id, "Integration smoke agent").await;
    assert_eq!(created["id"], agent_id);
    assert_eq!(created["enabled"], true);
    assert!(
        created["workspace_dir"]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty()),
        "created agent must return a non-empty workspace_dir"
    );

    let get_resp = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}"),
        None,
    )
    .await;
    assert_eq!(get_resp.status(), StatusCode::OK);
    let profile = read_json(get_resp).await;
    assert_eq!(profile["id"], agent_id);
    assert_eq!(profile["name"], "Integration smoke agent");
    let missing: Vec<_> = PROFILE_TOP_LEVEL_KEYS
        .iter()
        .copied()
        .filter(|key| profile.get(key).is_none())
        .collect();
    assert!(missing.is_empty(), "missing profile keys: {missing:?}");

    delete_agent(&app.app, agent_id).await;

    let missing_resp = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}"),
        None,
    )
    .await;
    assert_eq!(missing_resp.status(), StatusCode::NOT_FOUND);
    assert!(read_json(missing_resp).await.get("detail").is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_agents_reports_registered_profile_errors_without_fabrication() {
    let app = build_app();
    let id = "broken-profile";
    create_agent(&app.app, id, "Broken profile").await;
    let path = app.data_dir.join("workspaces").join(id).join("agent.json");
    std::fs::remove_file(&path).unwrap();
    let response = request(&app.app, Method::GET, "/api/agents", None).await;
    assert!(response.status().is_server_error());
    assert!(!path.exists());
    std::fs::write(&path, "invalid JSON").unwrap();
    let response = request(&app.app, Method::GET, "/api/agents", None).await;
    assert!(response.status().is_server_error());
    let healthy = request(&app.app, Method::GET, "/api/agents/default", None).await;
    assert_eq!(healthy.status(), StatusCode::OK);
    assert_eq!(std::fs::read_to_string(path).unwrap(), "invalid JSON");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_agent_generates_short_id_when_id_is_missing_or_blank() {
    let app = build_app();

    let missing_id_resp = request(
        &app.app,
        Method::POST,
        "/api/agents",
        Some(json!({
            "name": "Generated Id Agent",
            "description": ""
        })),
    )
    .await;
    assert_eq!(missing_id_resp.status(), StatusCode::CREATED);
    let missing_id_agent = read_json(missing_id_resp).await;
    let generated_id = missing_id_agent["id"].as_str().expect("generated id");
    assert_eq!(generated_id.len(), 12);
    assert!(
        generated_id.chars().all(|ch| ch.is_ascii_hexdigit()),
        "Rust generated ids should use the rua-config short hex format, got {generated_id}"
    );

    let blank_id_resp = request(
        &app.app,
        Method::POST,
        "/api/agents",
        Some(json!({
            "id": "   ",
            "name": "Blank Id Agent",
            "description": ""
        })),
    )
    .await;
    assert_eq!(blank_id_resp.status(), StatusCode::CREATED);
    let blank_id_agent = read_json(blank_id_resp).await;
    let blank_generated_id = blank_id_agent["id"].as_str().expect("generated id");
    assert_eq!(blank_generated_id.len(), 12);
    assert!(
        blank_generated_id.chars().all(|ch| ch.is_ascii_hexdigit()),
        "blank explicit ids should also be generated short hex ids, got {blank_generated_id}"
    );
    assert_ne!(
        generated_id, blank_generated_id,
        "two generated ids in the same config should not collide"
    );

    delete_agent(&app.app, generated_id).await;
    delete_agent(&app.app, blank_generated_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_agent_sanitizes_custom_id_and_rejects_invalid_reserved_or_duplicate_ids() {
    let app = build_app();

    let trimmed = request(
        &app.app,
        Method::POST,
        "/api/agents",
        Some(json!({
            "id": "  Browse-Agent  ",
            "name": "Trimmed custom id",
            "description": ""
        })),
    )
    .await;
    assert_eq!(trimmed.status(), StatusCode::CREATED);
    let trimmed_agent = read_json(trimmed).await;
    assert_eq!(trimmed_agent["id"], "Browse-Agent");

    for (id, expected_detail) in [
        ("a", "at least 2 characters"),
        ("中文名", "invalid characters"),
        ("-starts-dash", "invalid characters"),
        ("default", "reserved"),
        ("Browse-Agent", "already exists"),
    ] {
        let resp = request(
            &app.app,
            Method::POST,
            "/api/agents",
            Some(json!({
                "id": id,
                "name": format!("Invalid {id}"),
                "description": ""
            })),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY, "{id}");
        let body = read_json(resp).await;
        assert!(
            body["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains(expected_detail)),
            "expected {expected_detail:?} detail for id={id:?}, got {body:?}"
        );
    }

    delete_agent(&app.app, "Browse-Agent").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_and_readback_matches_qwenpaw_contract() {
    let app = build_app();
    let agent_id = "integ_agents_upd_01";
    create_agent(&app.app, agent_id, "Integration smoke agent").await;

    let get_before = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}"),
        None,
    )
    .await;
    assert_eq!(get_before.status(), StatusCode::OK);
    let mut profile = read_json(get_before).await;
    profile["name"] = json!("Integration smoke agent updated");
    profile["description"] = json!("updated by integration test");
    profile["system_prompt_files"] = json!(["AGENTS.md", "PROFILE.md", "SOUL.md"]);

    let put_resp = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}"),
        Some(profile),
    )
    .await;
    assert_eq!(put_resp.status(), StatusCode::OK);
    let put_payload = read_json(put_resp).await;
    assert_eq!(put_payload["id"], agent_id);
    assert_eq!(put_payload["name"], "Integration smoke agent updated");
    assert_eq!(put_payload["description"], "updated by integration test");
    assert_eq!(
        put_payload["system_prompt_files"],
        json!(["AGENTS.md", "PROFILE.md", "SOUL.md"])
    );

    let get_after = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}"),
        None,
    )
    .await;
    assert_eq!(get_after.status(), StatusCode::OK);
    let after = read_json(get_after).await;
    assert_eq!(after["name"], "Integration smoke agent updated");
    assert_eq!(after["description"], "updated by integration test");
    assert_eq!(
        after["system_prompt_files"],
        json!(["AGENTS.md", "PROFILE.md", "SOUL.md"])
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_preserves_enabled_state_in_detail_response() {
    let app = build_app();
    let agent_id = "integ_agents_disabled_update_01";
    create_agent(&app.app, agent_id, "Disabled update agent").await;

    let disable = request(
        &app.app,
        Method::PATCH,
        &format!("/api/agents/{agent_id}/toggle"),
        Some(json!({ "enabled": false })),
    )
    .await;
    assert_eq!(disable.status(), StatusCode::OK);
    assert_eq!(read_json(disable).await["enabled"], false);

    let get_before = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}"),
        None,
    )
    .await;
    assert_eq!(get_before.status(), StatusCode::OK);
    let mut profile = read_json(get_before).await;
    assert_eq!(
        profile["enabled"], false,
        "detail reads must reflect the config-profile enabled flag"
    );
    profile["name"] = json!("Disabled update agent renamed");
    profile["description"] = json!("renamed while disabled");
    profile["enabled"] = json!(true);

    let put_resp = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}"),
        Some(profile),
    )
    .await;
    assert_eq!(put_resp.status(), StatusCode::OK);
    let updated = read_json(put_resp).await;
    assert_eq!(updated["name"], "Disabled update agent renamed");
    assert_eq!(
        updated["enabled"], false,
        "profile update must not silently re-enable the agent; use /toggle for enabled state"
    );
    assert_eq!(enabled_from_list(&app.app, agent_id).await, Some(false));

    let re_enable = request(
        &app.app,
        Method::PATCH,
        &format!("/api/agents/{agent_id}/toggle"),
        Some(json!({ "enabled": true })),
    )
    .await;
    assert_eq!(re_enable.status(), StatusCode::OK);
    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_agent_can_be_updated_but_not_deleted_or_disabled() {
    let app = build_app();

    let before = request(&app.app, Method::GET, "/api/agents/default", None).await;
    assert_eq!(before.status(), StatusCode::OK);
    let mut baseline = read_json(before).await;
    let original_description = baseline["description"].clone();
    baseline["description"] = json!("contract-default-update");

    let update = request(&app.app, Method::PUT, "/api/agents/default", Some(baseline)).await;
    assert_eq!(update.status(), StatusCode::OK);
    assert_eq!(
        read_json(update).await["description"],
        "contract-default-update"
    );

    let after = request(&app.app, Method::GET, "/api/agents/default", None).await;
    assert_eq!(after.status(), StatusCode::OK);
    let mut restored = read_json(after).await;
    assert_eq!(restored["description"], "contract-default-update");

    let disable = request(
        &app.app,
        Method::PATCH,
        "/api/agents/default/toggle",
        Some(json!({ "enabled": false })),
    )
    .await;
    assert_eq!(disable.status(), StatusCode::BAD_REQUEST);
    assert!(read_json(disable).await["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains("default agent")));

    let delete = request(&app.app, Method::DELETE, "/api/agents/default", None).await;
    assert_eq!(delete.status(), StatusCode::BAD_REQUEST);
    assert!(read_json(delete).await["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains("default agent")));

    restored["description"] = original_description;
    let restore = request(&app.app, Method::PUT, "/api/agents/default", Some(restored)).await;
    assert_eq!(restore.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn order_put_roundtrips_and_controls_list_order() {
    let app = build_app();
    create_agent(&app.app, "integ_order_a", "Order A").await;
    create_agent(&app.app, "integ_order_b", "Order B").await;

    let before_resp = request(&app.app, Method::GET, "/api/agents", None).await;
    assert_eq!(before_resp.status(), StatusCode::OK);
    let baseline_ids = agent_ids(&read_json(before_resp).await);
    assert!(baseline_ids.len() >= 3);
    let builtin_ids = without_ids(&baseline_ids, &["integ_order_a", "integ_order_b"]);

    let mut reordered_ids = vec!["integ_order_b".to_string()];
    reordered_ids.extend(builtin_ids);
    reordered_ids.push("integ_order_a".to_string());
    let put_resp = request(
        &app.app,
        Method::PUT,
        "/api/agents/order",
        Some(json!({ "agent_ids": reordered_ids })),
    )
    .await;
    assert_eq!(put_resp.status(), StatusCode::OK);
    let put_payload = read_json(put_resp).await;
    assert_eq!(put_payload["success"], true);
    assert_eq!(put_payload["agent_ids"], json!(reordered_ids));

    let after_resp = request(&app.app, Method::GET, "/api/agents", None).await;
    assert_eq!(after_resp.status(), StatusCode::OK);
    assert_eq!(agent_ids(&read_json(after_resp).await), reordered_ids);

    delete_agent(&app.app, "integ_order_a").await;
    delete_agent(&app.app, "integ_order_b").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn order_put_rejects_incomplete_duplicate_or_unknown_payload() {
    let app = build_app();
    create_agent(&app.app, "integ_order_reject_a", "Order reject A").await;
    create_agent(&app.app, "integ_order_reject_b", "Order reject B").await;

    let current_resp = request(&app.app, Method::GET, "/api/agents", None).await;
    assert_eq!(current_resp.status(), StatusCode::OK);
    let current_ids = agent_ids(&read_json(current_resp).await);
    let incomplete = current_ids
        .iter()
        .filter(|id| id.as_str() != "integ_order_reject_b")
        .cloned()
        .collect::<Vec<_>>();
    let mut duplicate = current_ids.clone();
    let last_index = duplicate.len() - 1;
    duplicate[last_index] = "integ_order_reject_a".to_string();
    let mut unknown = current_ids.clone();
    unknown[last_index] = "unknown-agent".to_string();

    for invalid_order in [json!(incomplete), json!(duplicate), json!(unknown)] {
        let resp = request(
            &app.app,
            Method::PUT,
            "/api/agents/order",
            Some(json!({ "agent_ids": invalid_order })),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(read_json(resp).await["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("exactly once")));
    }

    delete_agent(&app.app, "integ_order_reject_a").await;
    delete_agent(&app.app, "integ_order_reject_b").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_agents_appends_missing_ids_from_old_config_order() {
    let app = build_app();
    let initial_resp = request(&app.app, Method::GET, "/api/agents", None).await;
    assert_eq!(initial_resp.status(), StatusCode::OK);
    let initial_ids = agent_ids(&read_json(initial_resp).await);

    create_agent(&app.app, "integ_order_missing_b", "Order missing B").await;
    create_agent(&app.app, "integ_order_missing_a", "Order missing A").await;

    write_config_agent_order(&app.data_dir, &["default"]);

    let resp = request(&app.app, Method::GET, "/api/agents", None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let actual = agent_ids(&read_json(resp).await);
    assert_eq!(actual.first().map(String::as_str), Some("default"));
    for id in initial_ids
        .iter()
        .map(String::as_str)
        .chain(["integ_order_missing_a", "integ_order_missing_b"])
    {
        assert!(
            actual.iter().any(|actual| actual == id),
            "missing appended Agent {id}"
        );
    }
    assert!(
        actual
            .iter()
            .position(|id| id == "integ_order_missing_a")
            .unwrap()
            < actual
                .iter()
                .position(|id| id == "integ_order_missing_b")
                .unwrap()
    );

    delete_agent(&app.app, "integ_order_missing_a").await;
    delete_agent(&app.app, "integ_order_missing_b").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_and_delete_keep_persisted_agent_order_in_sync() {
    let app = build_app();
    let initial_order = read_config_agent_order(&app.data_dir);
    create_agent(&app.app, "integ_order_sync_a", "Order sync A").await;
    create_agent(&app.app, "integ_order_sync_b", "Order sync B").await;

    let mut expected_initial_order = initial_order.clone();
    expected_initial_order.push("integ_order_sync_a".to_string());
    expected_initial_order.push("integ_order_sync_b".to_string());
    assert_eq!(
        read_config_agent_order(&app.data_dir),
        expected_initial_order
    );

    let mut reordered_ids = vec!["integ_order_sync_b".to_string()];
    reordered_ids.extend(initial_order.clone());
    reordered_ids.push("integ_order_sync_a".to_string());
    let put_resp = request(
        &app.app,
        Method::PUT,
        "/api/agents/order",
        Some(json!({ "agent_ids": reordered_ids })),
    )
    .await;
    assert_eq!(put_resp.status(), StatusCode::OK);

    create_agent(&app.app, "integ_order_sync_c", "Order sync C").await;
    let mut expected_after_create = reordered_ids.clone();
    expected_after_create.push("integ_order_sync_c".to_string());
    assert_eq!(
        read_config_agent_order(&app.data_dir),
        expected_after_create
    );

    delete_agent(&app.app, "integ_order_sync_b").await;
    let mut expected_after_delete = initial_order;
    expected_after_delete.push("integ_order_sync_a".to_string());
    expected_after_delete.push("integ_order_sync_c".to_string());
    assert_eq!(
        read_config_agent_order(&app.data_dir),
        expected_after_delete
    );

    delete_agent(&app.app, "integ_order_sync_a").await;
    delete_agent(&app.app, "integ_order_sync_c").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_agent_initializes_rust_runtime_workspace_layout() {
    let app = build_app();
    let agent_id = "integ_workspace_init_01";

    create_agent(&app.app, agent_id, "Workspace init agent").await;

    let workspace = app.data_dir.join("workspaces").join(agent_id);
    assert!(workspace.join("agent.json").is_file());
    assert!(workspace.join("sessions").is_dir());
    assert!(workspace.join("skills").is_dir());
    assert!(workspace.join("prompts").is_dir());
    let workspace_entries: Vec<String> = std::fs::read_dir(&workspace)
        .expect("workspace entries")
        .map(|entry| {
            entry
                .expect("workspace entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert!(
        !workspace_entries.iter().any(|name| name == "MEMORY"),
        "Removed Memory Markdown directory must not be initialized"
    );
    assert!(
        !workspace_entries.iter().any(|name| name == "memory"),
        "Removed lowercase Memory Markdown directory must not be initialized"
    );
    assert!(
        !workspace_entries.iter().any(|name| name == "MEMORY.md"),
        "Removed Memory Markdown file must not be initialized"
    );
    assert!(!workspace.join("active_skills").exists());
    assert!(!workspace.join("customized_skills").exists());

    let agent_json: Value =
        serde_json::from_slice(&std::fs::read(workspace.join("agent.json")).expect("agent json"))
            .expect("agent json parse");
    assert_eq!(agent_json["id"], agent_id);
    assert_eq!(agent_json["name"], "Workspace init agent");
    assert_eq!(agent_json["template_id"], "default");
    assert!(
        !agent_json["system_prompt_files"]
            .as_array()
            .is_some_and(|files| files.iter().any(|value| value == "AGENT_MEMORY.md")),
        "AGENT_MEMORY.md must not be a user-toggleable prompt file"
    );
    assert!(
        !workspace.join("prompts/AGENT_MEMORY.md").exists(),
        "AGENT_MEMORY.md must not be generated in workspace prompts"
    );

    delete_agent(&app.app, agent_id).await;
    assert!(!workspace.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_agent_applies_selected_template_prompt_files_to_workspace() {
    let app = build_app();
    let agent_id = "integ_template_apply_01";
    let template_file = app.data_dir.join("agent_templates.json");
    let prompt_library = app.data_dir.join("prompt_library");
    std::fs::create_dir_all(&prompt_library).expect("prompt library");
    std::fs::write(
        prompt_library.join("CUSTOM_PROMPT.md"),
        "# Custom prompt\n\nUse the selected Rust template.\n",
    )
    .expect("write custom prompt");
    rua_config::atomic_write_json(
        &template_file,
        &json!({
            "version": 1,
            "templates": {
                "custom-research": {
                    "name": "Custom Research",
                    "description": "Rust-owned template",
                    "prompt_files": ["CUSTOM_PROMPT.md"],
                    "builtin": false,
                    "created_at": "2026-06-20T00:00:00Z",
                    "updated_at": "2026-06-20T00:00:00Z"
                }
            }
        }),
    )
    .expect("write templates");

    let resp = request(
        &app.app,
        Method::POST,
        "/api/agents",
        Some(json!({
            "id": agent_id,
            "name": "Template apply agent",
            "description": "",
            "template_id": "custom-research"
        })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let created = read_json(resp).await;
    assert_eq!(created["template_id"], "custom-research");
    assert_eq!(created["system_prompt_files"], json!(["CUSTOM_PROMPT.md"]));

    let workspace = app.data_dir.join("workspaces").join(agent_id);
    assert_eq!(
        std::fs::read_to_string(workspace.join("prompts").join("CUSTOM_PROMPT.md"))
            .expect("workspace prompt"),
        "# Custom prompt\n\nUse the selected Rust template.\n"
    );

    let agent_json = read_agent_json(&app.data_dir, agent_id);
    assert_eq!(agent_json["template_id"], "custom-research");
    assert_eq!(
        agent_json["system_prompt_files"],
        json!(["CUSTOM_PROMPT.md"])
    );

    let templates: Value =
        serde_json::from_slice(&std::fs::read(template_file).expect("templates"))
            .expect("templates json");
    assert!(
        templates["templates"].get("default").is_none(),
        "agent creation must not backfill a user-owned template file"
    );
    assert!(templates["templates"].get("custom-research").is_some());

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn created_agent_persists_policy_and_missing_policy_is_rejected_without_rewrite() {
    let app = build_app();
    let id = "explicit-tool-inventory";
    create_agent(&app.app, id, "Explicit tools").await;
    let path = app.data_dir.join("workspaces").join(id).join("agent.json");
    let mut profile: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let builtin = profile["tools"]["builtin_tools"].as_object().unwrap();
    assert!(builtin.is_empty());
    assert_eq!(
        profile["tools"]["selection_policy"],
        serde_json::to_value(rua_config::types::tools::ToolSelectionPolicy::default_enabled())
            .unwrap()
    );
    profile.as_object_mut().unwrap().remove("tools");
    let bytes = serde_json::to_vec_pretty(&profile).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let error = app
        .state
        .tool_config_store
        .list_tools(Some(id))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("required"));
    let runtime = app.state.registry.get_or_create(id).await.unwrap();
    let error = runtime.capability_configuration(None).await.unwrap_err();
    assert!(error.to_string().contains("required"));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    delete_agent(&app.app, id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn template_errors_do_not_create_agents_or_restore_deleted_prompts() {
    let app = build_app();
    let store = rua_config::AgentTemplateStore::new(app.data_dir.clone());
    let mut template = store.get("default").unwrap().unwrap();
    template.prompt_files = vec!["MISSING_CUSTOM.md".into()];
    template.builtin = false;
    store.create("missing-file", template).unwrap();
    for (id, template_id) in [
        ("unknown-template-agent", "does-not-exist"),
        ("missing-file-agent", "missing-file"),
    ] {
        let response = request(
            &app.app,
            Method::POST,
            "/api/agents",
            Some(json!({"id":id,"name":id,"template_id":template_id})),
        )
        .await;
        assert!(
            response.status().is_client_error(),
            "{}",
            read_json(response).await
        );
        assert!(!app.data_dir.join("workspaces").join(id).exists());
    }
    let default = store.get("default").unwrap().unwrap();
    let deleted = app
        .data_dir
        .join("prompt_library")
        .join(&default.prompt_files[0]);
    std::fs::remove_file(&deleted).unwrap();
    let response = request(
        &app.app,
        Method::POST,
        "/api/agents",
        Some(json!({"id":"deleted-prompt-agent","name":"deleted","template_id":"default"})),
    )
    .await;
    assert!(response.status().is_client_error());
    assert!(!deleted.exists());
    assert!(!app
        .data_dir
        .join("workspaces/deleted-prompt-agent")
        .exists());
    let blank = request(
        &app.app,
        Method::POST,
        "/api/agents",
        Some(json!({"id":"blank-template-agent","name":"blank","template_id":"blank"})),
    )
    .await;
    assert_eq!(blank.status(), StatusCode::CREATED);
    delete_agent(&app.app, "blank-template-agent").await;
    std::fs::write(app.data_dir.join("agent_templates.json"), b"invalid JSON").unwrap();
    let response = request(
        &app.app,
        Method::POST,
        "/api/agents",
        Some(json!({"id":"corrupt-template-agent","name":"corrupt"})),
    )
    .await;
    assert!(response.status().is_server_error());
    assert!(!app
        .data_dir
        .join("workspaces/corrupt-template-agent")
        .exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_agent_preserves_provider_model_as_extra_without_setting_default_model() {
    let app = build_app();
    let agent_id = "integ_agents_model_extra_01";

    let create_resp = request(
        &app.app,
        Method::POST,
        "/api/agents",
        Some(json!({
            "id": agent_id,
            "name": "Model extra agent",
            "description": "",
            "provider_id": "dashscope",
            "model_id": "qwen-max"
        })),
    )
    .await;
    assert_eq!(create_resp.status(), StatusCode::CREATED);
    let created = read_json(create_resp).await;
    assert_eq!(created["id"], agent_id);
    assert_eq!(created["provider_id"], "dashscope");
    assert_eq!(created["model_id"], "qwen-max");
    assert!(
        created["default_model"].is_null(),
        "Rust create does not infer default_model from provider_id/model_id"
    );

    let get_resp = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}"),
        None,
    )
    .await;
    assert_eq!(get_resp.status(), StatusCode::OK);
    let detail = read_json(get_resp).await;
    assert_eq!(detail["provider_id"], "dashscope");
    assert_eq!(detail["model_id"], "qwen-max");
    assert!(detail["default_model"].is_null());

    let agent_json = read_agent_json(&app.data_dir, agent_id);
    assert_eq!(agent_json["provider_id"], "dashscope");
    assert_eq!(agent_json["model_id"], "qwen-max");
    assert!(agent_json["default_model"].is_null());

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_default_model_can_be_set_and_cleared_through_detail_route() {
    let app = build_app();
    let agent_id = "integ_agent_default_model_01";
    create_agent(&app.app, agent_id, "Active model agent").await;

    let get_before = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}"),
        None,
    )
    .await;
    assert_eq!(get_before.status(), StatusCode::OK);
    let mut profile = read_json(get_before).await;
    assert!(profile["default_model"].is_null());

    profile["default_model"] = json!({
        "provider_id": "openai",
        "model": "gpt-4",
    });
    let set_resp = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}"),
        Some(profile),
    )
    .await;
    assert_eq!(set_resp.status(), StatusCode::OK);
    let set_body = read_json(set_resp).await;
    assert_eq!(
        set_body["default_model"],
        json!({ "provider_id": "openai", "model": "gpt-4" })
    );
    assert_eq!(
        read_agent_json(&app.data_dir, agent_id)["default_model"],
        json!({ "provider_id": "openai", "model": "gpt-4" })
    );

    let get_after_set = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}"),
        None,
    )
    .await;
    assert_eq!(get_after_set.status(), StatusCode::OK);
    let mut profile = read_json(get_after_set).await;
    assert_eq!(profile["default_model"]["provider_id"], "openai");
    assert_eq!(profile["default_model"]["model"], "gpt-4");

    profile["default_model"] = Value::Null;
    let clear_resp = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}"),
        Some(profile),
    )
    .await;
    assert_eq!(clear_resp.status(), StatusCode::OK);
    assert!(
        read_json(clear_resp).await["default_model"].is_null(),
        "explicit JSON null must clear an existing default_model"
    );

    let get_after_clear = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}"),
        None,
    )
    .await;
    assert_eq!(get_after_clear.status(), StatusCode::OK);
    assert!(read_json(get_after_clear).await["default_model"].is_null());
    assert!(read_agent_json(&app.data_dir, agent_id)["default_model"].is_null());

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_running_retry_config_persists_and_rejects_invalid_backoff() {
    let app = build_app();
    let agent_id = "integ_agent_running_retry_01";
    create_agent(&app.app, agent_id, "Running retry agent").await;

    let get_before = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}"),
        None,
    )
    .await;
    assert_eq!(get_before.status(), StatusCode::OK);
    let mut profile = read_json(get_before).await;
    profile["running"] = json!({
        "llm_retry_enabled": false,
        "llm_max_retries": 5,
        "llm_backoff_base": 0.5,
        "llm_backoff_cap": 8.0,
    });

    let set_resp = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}"),
        Some(profile),
    )
    .await;
    assert_eq!(set_resp.status(), StatusCode::OK);
    let set_body = read_json(set_resp).await;
    assert_eq!(set_body["running"]["llm_retry_enabled"], false);
    assert_eq!(set_body["running"]["llm_max_retries"], 5);
    assert_eq!(set_body["running"]["llm_backoff_base"], 0.5);
    assert_eq!(set_body["running"]["llm_backoff_cap"], 8.0);
    let persisted = read_agent_json(&app.data_dir, agent_id);
    assert_eq!(persisted["running"]["llm_retry_enabled"], false);
    assert_eq!(persisted["running"]["llm_max_retries"], 5);
    assert_eq!(persisted["running"]["llm_backoff_base"], 0.5);
    assert_eq!(persisted["running"]["llm_backoff_cap"], 8.0);

    let get_after_set = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}"),
        None,
    )
    .await;
    assert_eq!(get_after_set.status(), StatusCode::OK);
    let mut invalid_profile = read_json(get_after_set).await;
    invalid_profile["running"]["llm_backoff_base"] = json!(2.0);
    invalid_profile["running"]["llm_backoff_cap"] = json!(1.0);

    let invalid_resp = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}"),
        Some(invalid_profile),
    )
    .await;
    assert_eq!(invalid_resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(read_json(invalid_resp).await["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains("llm_backoff_cap")));

    let after_invalid = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}"),
        None,
    )
    .await;
    assert_eq!(after_invalid.status(), StatusCode::OK);
    let after_invalid_body = read_json(after_invalid).await;
    assert_eq!(after_invalid_body["running"]["llm_backoff_base"], 0.5);
    assert_eq!(after_invalid_body["running"]["llm_backoff_cap"], 8.0);

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn patch_toggle_enabled_roundtrips_through_list_summary() {
    let app = build_app();
    let agent_id = "integ_agent_toggle_01";
    create_agent(&app.app, agent_id, "Toggle agent").await;

    let off = request(
        &app.app,
        Method::PATCH,
        &format!("/api/agents/{agent_id}/toggle"),
        Some(json!({ "enabled": false })),
    )
    .await;
    assert_eq!(off.status(), StatusCode::OK);
    assert_eq!(read_json(off).await["enabled"], false);
    assert_eq!(enabled_from_list(&app.app, agent_id).await, Some(false));

    let on = request(
        &app.app,
        Method::PATCH,
        &format!("/api/agents/{agent_id}/toggle"),
        Some(json!({ "enabled": true })),
    )
    .await;
    assert_eq!(on.status(), StatusCode::OK);
    assert_eq!(read_json(on).await["enabled"], true);
    assert_eq!(enabled_from_list(&app.app, agent_id).await, Some(true));

    delete_agent(&app.app, agent_id).await;
}

async fn enabled_from_list(app: &axum::Router, agent_id: &str) -> Option<bool> {
    let list = request(app, Method::GET, "/api/agents", None).await;
    assert_eq!(list.status(), StatusCode::OK);
    read_json(list).await["agents"]
        .as_array()
        .expect("agents array")
        .iter()
        .find(|row| row["id"] == agent_id)
        .map(|row| row["enabled"].as_bool().unwrap_or(true))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_route_returns_rust_agent_status_contract() {
    let app = build_app();
    let agent_id = "integ_agent_status_01";
    create_agent(&app.app, agent_id, "Agent status contract").await;

    let resp = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/status"),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["id"], agent_id);
    assert_eq!(body["status"], "idle");
    assert_eq!(body["active_sessions"], 0);

    let missing = request(
        &app.app,
        Method::GET,
        "/api/agents/missing_status_agent/status",
        None,
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert!(read_json(missing).await["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains("missing_status_agent")));

    delete_agent(&app.app, agent_id).await;
}
