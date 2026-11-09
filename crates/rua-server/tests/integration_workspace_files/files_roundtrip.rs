use axum::http::{Method, StatusCode};
use serde_json::{json, Value};
use serial_test::serial;

use super::support::{build_app, create_agent, delete_agent, read_json, request};

fn filenames(payload: &Value) -> Vec<String> {
    payload
        .as_array()
        .expect("list response array")
        .iter()
        .map(|row| {
            row["filename"]
                .as_str()
                .or_else(|| row["name"].as_str())
                .expect("filename")
                .to_string()
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(workspace_files)]
async fn fresh_agent_file_list_exposes_default_template_prompt_files() {
    let app = build_app();
    let agent_id = "integ_ws_template_seed";
    create_agent(&app.app, agent_id, "Workspace template seed agent").await;

    let list = request(
        &app.app,
        Method::GET,
        "/api/agent/files",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(list.status(), StatusCode::OK);
    let names = filenames(&read_json(list).await);
    for expected in [
        "10_AGENT_SAFETY.md",
        "20_AGENT_WORKFLOW.md",
        "30_EXECUTION_REPORTING.md",
        "40_STRUCTURED_MEMORY.md",
        "50_AGENT_RESPONSE.md",
        "60_MARKDOWN_PLUS.md",
        "90_DEFAULT_PROFILE.md",
    ] {
        assert!(
            names.iter().any(|name| name == expected),
            "fresh Rust agent should expose default template prompt file {expected}; have={names:?}"
        );
    }
    assert!(
        !names.iter().any(|name| name == "AGENT_MODE.md"),
        "agent mode prompt must not be generated for fresh Rust agents; have={names:?}"
    );
    assert!(!names.iter().any(|name| name == "AGENT_MEMORY.md"));

    for python_seed in [
        "AGENTS.md",
        "BOOTSTRAP.md",
        "HEARTBEAT.md",
        "MEMORY.md",
        "PROFILE.md",
    ] {
        assert!(
            !names.iter().any(|name| name == python_seed),
            "Rust default template prompt files should not be confused with QwenPaw seed file {python_seed}; have={names:?}"
        );
    }

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(workspace_files)]
async fn working_file_list_put_get_and_header_scoped_consistency() {
    let app = build_app();
    let agent_id = "integ_ws_files";
    let file_stem = "scope_consistency_note";
    create_agent(&app.app, agent_id, "Workspace files agent").await;

    let list_before = request(
        &app.app,
        Method::GET,
        "/api/agent/files",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(list_before.status(), StatusCode::OK);
    assert!(read_json(list_before).await.is_array());

    let v1 = "# scope consistency v1\n";
    let root_put = request(
        &app.app,
        Method::PUT,
        &format!("/api/agent/files/{file_stem}"),
        Some(json!({"content": v1})),
        Some(agent_id),
    )
    .await;
    assert_eq!(root_put.status(), StatusCode::OK);
    assert_eq!(read_json(root_put).await["content"], v1);

    let scoped_get = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/files/{file_stem}"),
        None,
        None,
    )
    .await;
    assert_eq!(scoped_get.status(), StatusCode::OK);
    assert_eq!(read_json(scoped_get).await["content"], v1.trim());

    let v2 = "# scope consistency v2\n";
    let scoped_put = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}/files/{file_stem}"),
        Some(json!({"content": v2})),
        None,
    )
    .await;
    assert_eq!(scoped_put.status(), StatusCode::OK);
    assert_eq!(read_json(scoped_put).await["content"], v2);

    let root_get = request(
        &app.app,
        Method::GET,
        &format!("/api/agent/files/{file_stem}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(root_get.status(), StatusCode::OK);
    assert_eq!(read_json(root_get).await["content"], v2.trim());

    let list_after = request(
        &app.app,
        Method::GET,
        "/api/agent/files",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(list_after.status(), StatusCode::OK);
    assert!(filenames(&read_json(list_after).await).contains(&format!("{file_stem}.md")));

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(workspace_files)]
async fn file_shaped_memory_routes_are_removed_for_header_and_scoped_routes() {
    let app = build_app();
    let agent_id = "integ_ws_memory_removed";
    create_agent(&app.app, agent_id, "Removed file-shaped Memory agent").await;

    for (method, uri, body, header_agent) in [
        (
            Method::GET,
            "/api/agent/memory".to_string(),
            None,
            Some(agent_id),
        ),
        (
            Method::GET,
            "/api/agent/memory/2026-06-26.md".to_string(),
            None,
            Some(agent_id),
        ),
        (
            Method::PUT,
            "/api/agent/memory/2026-06-26.md".to_string(),
            Some(json!({"content": "removed"})),
            Some(agent_id),
        ),
        (
            Method::GET,
            format!("/api/agents/{agent_id}/memory"),
            None,
            None,
        ),
        (
            Method::GET,
            format!("/api/agents/{agent_id}/memory/2026-06-26.md"),
            None,
            None,
        ),
        (
            Method::PUT,
            format!("/api/agents/{agent_id}/memory/2026-06-26.md"),
            Some(json!({"content": "removed"})),
            None,
        ),
    ] {
        let response = request(&app.app, method, &uri, body, header_agent).await;
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "file-shaped Memory route must be removed: {uri}"
        );
    }

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(workspace_files)]
async fn workspace_files_are_isolated_between_agents() {
    let app = build_app();
    let agent_a = "integ_ws_files_iso_a";
    let agent_b = "integ_ws_files_iso_b";
    let file_stem = "private_workspace_note";
    create_agent(&app.app, agent_a, "Workspace files isolation A").await;
    create_agent(&app.app, agent_b, "Workspace files isolation B").await;

    let put_a = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/{agent_a}/files/{file_stem}"),
        Some(json!({"content": "agent A only\n"})),
        None,
    )
    .await;
    assert_eq!(put_a.status(), StatusCode::OK);

    let get_b = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_b}/files/{file_stem}"),
        None,
        None,
    )
    .await;
    assert_eq!(
        get_b.status(),
        StatusCode::NOT_FOUND,
        "agent B must not read agent A workspace files"
    );

    let get_a_via_header = request(
        &app.app,
        Method::GET,
        &format!("/api/agent/files/{file_stem}"),
        None,
        Some(agent_a),
    )
    .await;
    assert_eq!(get_a_via_header.status(), StatusCode::OK);
    assert_eq!(read_json(get_a_via_header).await["content"], "agent A only");

    delete_agent(&app.app, agent_a).await;
    delete_agent(&app.app, agent_b).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(workspace_files)]
async fn prompt_subdir_routes_roundtrip_case_insensitive_directory_and_cleanup() {
    let app = build_app();
    let agent_id = "integ_ws_subdir_prompt";
    create_agent(&app.app, agent_id, "Workspace subdir prompt agent").await;

    let root_prompt = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}/files/Research"),
        Some(json!({"content": "# Research root prompt\n"})),
        None,
    )
    .await;
    assert_eq!(root_prompt.status(), StatusCode::OK);

    let created = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/subdirs/research"),
        Some(json!({"filename": "alpha"})),
        None,
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created_body = read_json(created).await;
    assert_eq!(created_body["filename"], "alpha.md");
    assert_eq!(created_body["path"], "RESEARCH/alpha.md");

    let written = request(
        &app.app,
        Method::PUT,
        "/api/agent/subdirs/research/alpha",
        Some(json!({"content": "# Alpha\nsubdir prompt\n"})),
        Some(agent_id),
    )
    .await;
    assert_eq!(written.status(), StatusCode::OK);
    assert_eq!(
        read_json(written).await["content"],
        "# Alpha\nsubdir prompt\n"
    );

    let fetched_upper = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/subdirs/RESEARCH/alpha"),
        None,
        None,
    )
    .await;
    assert_eq!(fetched_upper.status(), StatusCode::OK);
    assert_eq!(
        read_json(fetched_upper).await["content"],
        "# Alpha\nsubdir prompt"
    );

    let listed = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/subdirs/research"),
        None,
        None,
    )
    .await;
    assert_eq!(listed.status(), StatusCode::OK);
    assert!(filenames(&read_json(listed).await).contains(&"alpha.md".to_string()));

    let info = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/subdirs-info"),
        None,
        None,
    )
    .await;
    assert_eq!(info.status(), StatusCode::OK);
    assert_eq!(read_json(info).await["Research.md"], 1);

    let renamed = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/subdirs/research/alpha/rename"),
        Some(json!({"new_name": "beta"})),
        None,
    )
    .await;
    assert_eq!(renamed.status(), StatusCode::OK);
    let renamed_body = read_json(renamed).await;
    assert_eq!(renamed_body["filename"], "beta.md");
    assert_eq!(renamed_body["content"], "# Alpha\nsubdir prompt\n");

    let old_name = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/subdirs/research/alpha"),
        None,
        None,
    )
    .await;
    assert_eq!(old_name.status(), StatusCode::NOT_FOUND);

    let deleted = request(
        &app.app,
        Method::DELETE,
        &format!("/api/agents/{agent_id}/subdirs/research/beta"),
        None,
        None,
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::OK);
    assert_eq!(read_json(deleted).await["success"], true);

    let info_after_delete = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/subdirs-info"),
        None,
        None,
    )
    .await;
    assert_eq!(info_after_delete.status(), StatusCode::OK);
    assert!(
        read_json(info_after_delete).await["Research.md"].is_null(),
        "empty subdirs should drop out of prompt subdir info"
    );

    delete_agent(&app.app, agent_id).await;
}
