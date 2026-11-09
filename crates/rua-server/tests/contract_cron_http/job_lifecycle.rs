use axum::http::{Method, StatusCode};
use chrono::Utc;
use serde_json::json;

use super::support::{build_app, create_agent, cron_payload, delete_agent, read_json, request};
use rua_server::services::chat_index::{self, ChatIndexEntry};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn root_cron_jobs_lifecycle_uses_rust_public_contract_shape() {
    let app = build_app();
    let agent_id = "integ_cron_header_contract";
    create_agent(&app.app, agent_id, "Cron header contract").await;

    let empty = request(
        &app.app,
        Method::GET,
        "/api/cron/jobs",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(empty.status(), StatusCode::OK);
    assert_eq!(read_json(empty).await, json!([]));

    let create = request(
        &app.app,
        Method::POST,
        "/api/cron/jobs",
        Some(cron_payload("contract cron v1")),
        Some(agent_id),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);
    let created = read_json(create).await;
    let job_id = created["id"].as_str().expect("job id").to_owned();
    assert_eq!(created["name"], "contract cron v1");
    assert_eq!(created["enabled"], true);
    assert_eq!(created["schedule"]["type"], "cron");
    assert_eq!(created["schedule"]["cron"], "0 0 * * *");
    assert_eq!(created["schedule"]["timezone"], "UTC");
    assert_eq!(created["dispatch"]["channel"], "console");

    let jobs_path = app
        .state
        .rua_root
        .join("workspaces")
        .join(agent_id)
        .join("jobs.json");
    assert!(
        jobs_path.exists(),
        "root cron create must write into the request agent workspace"
    );

    let list = request(
        &app.app,
        Method::GET,
        "/api/cron/jobs",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(list.status(), StatusCode::OK);
    let list_body = read_json(list).await;
    assert_eq!(list_body.as_array().expect("list").len(), 1);
    assert_eq!(list_body[0]["id"], job_id);

    let get = request(
        &app.app,
        Method::GET,
        &format!("/api/cron/jobs/{job_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(get.status(), StatusCode::OK);
    let get_body = read_json(get).await;
    assert_eq!(get_body["id"], job_id);
    assert!(
        get_body.get("spec").is_none(),
        "Rust returns the job DTO directly"
    );
    assert!(get_body.get("state").is_none(), "state is a separate route");

    let update = request(
        &app.app,
        Method::PUT,
        &format!("/api/cron/jobs/{job_id}"),
        Some(json!({
            "name": "contract cron v2",
            "schedule": {
                "type": "cron",
                "cron": "*/15 * * * *",
                "timezone": "Asia/Shanghai"
            },
            "enabled": false
        })),
        Some(agent_id),
    )
    .await;
    assert_eq!(update.status(), StatusCode::OK);
    let updated = read_json(update).await;
    assert_eq!(updated["name"], "contract cron v2");
    assert_eq!(updated["enabled"], false);
    assert_eq!(updated["schedule"]["cron"], "*/15 * * * *");

    let state = request(
        &app.app,
        Method::GET,
        &format!("/api/cron/jobs/{job_id}/state"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(state.status(), StatusCode::OK);
    assert_eq!(
        read_json(state).await,
        json!({
            "next_run_at": null,
            "last_run_at": null,
            "last_status": null,
            "last_error": null
        })
    );

    let history = request(
        &app.app,
        Method::GET,
        &format!("/api/cron/jobs/{job_id}/history"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    assert_eq!(read_json(history).await, json!([]));

    let delete = request(
        &app.app,
        Method::DELETE,
        &format!("/api/cron/jobs/{job_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(delete.status(), StatusCode::OK);
    assert_eq!(
        read_json(delete).await,
        json!({"success": true, "message": null})
    );

    let delete_again = request(
        &app.app,
        Method::DELETE,
        &format!("/api/cron/jobs/{job_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(delete_again.status(), StatusCode::NOT_FOUND);
    assert!(read_json(delete_again).await["detail"]
        .as_str()
        .expect("detail")
        .contains(&job_id));

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cron_pause_and_resume_are_persisted_as_job_state_not_response_flags() {
    let app = build_app();
    let agent_id = "integ_cron_pause_contract";
    create_agent(&app.app, agent_id, "Cron pause contract").await;

    let create = request(
        &app.app,
        Method::POST,
        "/api/cron/jobs",
        Some(cron_payload("contract cron pause")),
        Some(agent_id),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);
    let job_id = read_json(create).await["id"]
        .as_str()
        .expect("job id")
        .to_owned();

    let pause = request(
        &app.app,
        Method::POST,
        &format!("/api/cron/jobs/{job_id}/pause"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(pause.status(), StatusCode::OK);
    let pause_body = read_json(pause).await;
    assert_eq!(pause_body["id"], job_id);
    assert!(pause_body.get("paused").is_none());

    let reenable = request(
        &app.app,
        Method::PUT,
        &format!("/api/cron/jobs/{job_id}"),
        Some(json!({ "enabled": true })),
        Some(agent_id),
    )
    .await;
    assert_eq!(reenable.status(), StatusCode::OK);
    let raw = std::fs::read_to_string(
        app.state
            .rua_root
            .join("workspaces")
            .join(agent_id)
            .join("jobs.json"),
    )
    .expect("read jobs.json");
    let persisted: serde_json::Value = serde_json::from_str(&raw).expect("jobs json");
    assert_eq!(
        persisted["jobs"][&job_id]["paused"], false,
        "enabling a job must clear persisted pause state so the scheduler can run it"
    );

    let resume = request(
        &app.app,
        Method::POST,
        &format!("/api/cron/jobs/{job_id}/resume"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(resume.status(), StatusCode::OK);
    let resume_body = read_json(resume).await;
    assert_eq!(resume_body["id"], job_id);
    assert!(resume_body.get("resumed").is_none());

    let delete = request(
        &app.app,
        Method::DELETE,
        &format!("/api/cron/jobs/{job_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(delete.status(), StatusCode::OK);
    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cron_create_accepts_qwenpaw_once_repeat_runtime_shape() {
    let app = build_app();
    let agent_id = "integ_cron_once_contract";
    create_agent(&app.app, agent_id, "Cron once contract").await;

    let create = request(
        &app.app,
        Method::POST,
        "/api/cron/jobs",
        Some(json!({
            "name": "qwenpaw once repeat",
            "enabled": true,
            "schedule": {
                "type": "once",
                "run_at": "2099-01-01T09:30:00Z",
                "timezone": "Asia/Shanghai",
                "repeat_every_days": 2,
                "repeat_end_type": "count",
                "repeat_count": 3
            },
            "task_type": "text",
            "text": "once text",
            "runtime": {
                "max_concurrency": 2,
                "timeout_seconds": 30,
                "misfire_grace_seconds": 600,
                "share_session": false,
                "tool_safety": true
            },
            "dispatch": {
                "type": "channel",
                "channel": "console",
                "target": {
                    "user_id": "cron-once-user",
                    "session_id": "console:cron-once-session"
                },
                "mode": "stream",
                "meta": {}
            }
        })),
        Some(agent_id),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);
    let created = read_json(create).await;
    let job_id = created["id"].as_str().expect("job id");
    assert_eq!(created["schedule"]["type"], "once");
    assert_eq!(created["schedule"]["run_at"], "2099-01-01T09:30:00Z");
    assert_eq!(created["schedule"]["repeat_every_days"], 2);
    assert_eq!(created["schedule"]["repeat_end_type"], "count");
    assert_eq!(created["schedule"]["repeat_count"], 3);
    assert_eq!(created["save_result_to_inbox"], true);
    assert_eq!(created["runtime"]["max_concurrency"], 2);
    assert_eq!(created["runtime"]["timeout_seconds"], 30);
    assert_eq!(created["runtime"]["misfire_grace_seconds"], 600);
    assert_eq!(created["runtime"]["share_session"], false);
    assert_eq!(created["runtime"]["tool_safety"], true);

    let raw = std::fs::read_to_string(
        app.state
            .rua_root
            .join("workspaces")
            .join(agent_id)
            .join("jobs.json"),
    )
    .expect("read jobs.json");
    let persisted: serde_json::Value = serde_json::from_str(&raw).expect("jobs json");
    assert_eq!(persisted["jobs"][job_id]["schedule"]["type"], "once");
    assert_eq!(
        persisted["jobs"][job_id]["schedule"]["repeat_end_type"],
        "count"
    );
    assert_eq!(persisted["jobs"][job_id]["runtime"]["share_session"], false);
    assert_eq!(persisted["jobs"][job_id]["runtime"]["tool_safety"], true);

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cron_dispatch_targets_are_derived_from_agent_chat_index() {
    let app = build_app();
    let agent_id = "integ_cron_targets_contract";
    create_agent(&app.app, agent_id, "Cron targets contract").await;
    let workspace_dir = app.state.rua_root.join("workspaces").join(agent_id);

    for (id, channel, user_id) in [
        ("console:cron-session-a", "console", "cron-user-a"),
        ("ding:cron-session-b", "dingtalk", "cron-user-b"),
    ] {
        chat_index::upsert(
            &workspace_dir,
            ChatIndexEntry {
                id: id.to_string(),
                name: id.to_string(),
                user_id: user_id.to_string(),
                channel: channel.to_string(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
                meta: serde_json::Value::Null,
                status: "idle".to_string(),
                pinned: false,
                pin_order: 0,
                tag: rua_session::ChatTag::None,
                archived: false,
            },
        )
        .await
        .expect("seed chat index");
    }

    let all = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/cron/dispatch-targets"),
        None,
        Some("default"),
    )
    .await;
    assert_eq!(all.status(), StatusCode::OK);
    let all_body = read_json(all).await;
    assert_eq!(all_body["channels"], json!(["console", "dingtalk"]));
    assert_eq!(
        all_body["items"].as_array().expect("items").len(),
        2,
        "one Chat identity should produce one dispatch target"
    );

    let filtered = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/cron/dispatch-targets?channel=dingtalk&keyword=session-b"),
        None,
        Some("default"),
    )
    .await;
    assert_eq!(filtered.status(), StatusCode::OK);
    let filtered_body = read_json(filtered).await;
    assert_eq!(filtered_body["channels"], json!(["console", "dingtalk"]));
    assert_eq!(filtered_body["items"].as_array().expect("items").len(), 1);
    assert_eq!(filtered_body["items"][0]["channel"], "dingtalk");
    assert_eq!(filtered_body["items"][0]["user_id"], "cron-user-b");
    assert_eq!(
        filtered_body["items"][0]["session_id"],
        "ding:cron-session-b"
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cron_routes_are_isolated_by_header_and_path_agent_scope() {
    let app = build_app();
    let alpha_agent = "integ_cron_alpha_contract";
    let beta_agent = "integ_cron_beta_contract";
    create_agent(&app.app, alpha_agent, "Cron alpha contract").await;
    create_agent(&app.app, beta_agent, "Cron beta contract").await;

    let alpha_create = request(
        &app.app,
        Method::POST,
        "/api/cron/jobs",
        Some(cron_payload("contract cron alpha")),
        Some(alpha_agent),
    )
    .await;
    assert_eq!(alpha_create.status(), StatusCode::CREATED);
    let alpha_job_id = read_json(alpha_create).await["id"]
        .as_str()
        .expect("job id")
        .to_owned();

    let beta_base = format!("/api/agents/{beta_agent}/cron/jobs");
    let beta_create = request(
        &app.app,
        Method::POST,
        &beta_base,
        Some(cron_payload("contract cron beta")),
        Some(alpha_agent),
    )
    .await;
    assert_eq!(beta_create.status(), StatusCode::CREATED);
    let beta_job_id = read_json(beta_create).await["id"]
        .as_str()
        .expect("job id")
        .to_owned();

    let alpha_list = request(
        &app.app,
        Method::GET,
        "/api/cron/jobs",
        None,
        Some(alpha_agent),
    )
    .await;
    assert_eq!(alpha_list.status(), StatusCode::OK);
    let alpha_ids: Vec<_> = read_json(alpha_list)
        .await
        .as_array()
        .expect("jobs")
        .iter()
        .filter_map(|job| job["id"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(alpha_ids, vec![alpha_job_id.clone()]);

    let beta_list = request(&app.app, Method::GET, &beta_base, None, Some(alpha_agent)).await;
    assert_eq!(beta_list.status(), StatusCode::OK);
    let beta_ids: Vec<_> = read_json(beta_list)
        .await
        .as_array()
        .expect("jobs")
        .iter()
        .filter_map(|job| job["id"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(
        beta_ids,
        vec![beta_job_id.clone()],
        "path-scoped cron list must prefer the path agent over X-Agent-Id"
    );

    let alpha_gets_beta = request(
        &app.app,
        Method::GET,
        &format!("/api/cron/jobs/{beta_job_id}"),
        None,
        Some(alpha_agent),
    )
    .await;
    assert_eq!(alpha_gets_beta.status(), StatusCode::NOT_FOUND);

    let beta_gets_alpha = request(
        &app.app,
        Method::GET,
        &format!("{beta_base}/{alpha_job_id}"),
        None,
        Some(alpha_agent),
    )
    .await;
    assert_eq!(beta_gets_alpha.status(), StatusCode::NOT_FOUND);

    let alpha_delete = request(
        &app.app,
        Method::DELETE,
        &format!("/api/cron/jobs/{alpha_job_id}"),
        None,
        Some(alpha_agent),
    )
    .await;
    assert_eq!(alpha_delete.status(), StatusCode::OK);

    let beta_delete = request(
        &app.app,
        Method::DELETE,
        &format!("{beta_base}/{beta_job_id}"),
        None,
        Some(alpha_agent),
    )
    .await;
    assert_eq!(beta_delete.status(), StatusCode::OK);
    delete_agent(&app.app, alpha_agent).await;
    delete_agent(&app.app, beta_agent).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cron_mutation_routes_do_not_cross_agent_scope() {
    let app = build_app();
    let owner_agent = "integ_cron_owner_scope";
    let other_agent = "integ_cron_other_scope";
    create_agent(&app.app, owner_agent, "Cron owner scope").await;
    create_agent(&app.app, other_agent, "Cron other scope").await;

    let owner_create = request(
        &app.app,
        Method::POST,
        "/api/cron/jobs",
        Some(cron_payload("contract cron owned")),
        Some(owner_agent),
    )
    .await;
    assert_eq!(owner_create.status(), StatusCode::CREATED);
    let owner_job_id = read_json(owner_create).await["id"]
        .as_str()
        .expect("job id")
        .to_owned();

    let other_base = format!("/api/agents/{other_agent}/cron/jobs/{owner_job_id}");
    for (method, uri, body) in [
        (Method::GET, other_base.clone(), None),
        (Method::GET, format!("{other_base}/state"), None),
        (
            Method::PUT,
            other_base.clone(),
            Some(json!({"name": "wrong agent"})),
        ),
        (Method::POST, format!("{other_base}/pause"), None),
        (Method::POST, format!("{other_base}/resume"), None),
        (Method::POST, format!("{other_base}/run"), None),
        (Method::DELETE, other_base.clone(), None),
    ] {
        let resp = request(&app.app, method, &uri, body, Some(owner_agent)).await;
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "{uri} must not resolve another agent's cron job"
        );
        let detail = read_json(resp).await["detail"]
            .as_str()
            .expect("detail")
            .to_owned();
        assert!(
            detail.contains(&owner_job_id),
            "404 detail should name the inaccessible job id: {detail}"
        );
    }

    let owner_get = request(
        &app.app,
        Method::GET,
        &format!("/api/cron/jobs/{owner_job_id}"),
        None,
        Some(owner_agent),
    )
    .await;
    assert_eq!(owner_get.status(), StatusCode::OK);
    let owner_body = read_json(owner_get).await;
    assert_eq!(owner_body["id"], owner_job_id);
    assert_eq!(owner_body["name"], "contract cron owned");

    let owner_delete = request(
        &app.app,
        Method::DELETE,
        &format!("/api/cron/jobs/{owner_job_id}"),
        None,
        Some(owner_agent),
    )
    .await;
    assert_eq!(owner_delete.status(), StatusCode::OK);
    delete_agent(&app.app, owner_agent).await;
    delete_agent(&app.app, other_agent).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cron_update_roundtrips_dispatch_target_and_meta() {
    let app = build_app();
    let agent_id = "integ_cron_dispatch_update_contract";
    create_agent(&app.app, agent_id, "Cron dispatch update contract").await;

    let create = request(
        &app.app,
        Method::POST,
        "/api/cron/jobs",
        Some(cron_payload("contract cron dispatch update")),
        Some(agent_id),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);
    let created = read_json(create).await;
    let job_id = created["id"].as_str().expect("job id").to_owned();
    assert_eq!(created["dispatch"]["target"]["user_id"], "integ-cron-user");
    assert_eq!(
        created["dispatch"]["target"]["session_id"],
        "console:integ-cron-session"
    );

    let update = request(
        &app.app,
        Method::PUT,
        &format!("/api/cron/jobs/{job_id}"),
        Some(json!({
            "dispatch": {
                "type": "channel",
                "channel": "console",
                "target": {
                    "user_id": "updated-cron-user",
                    "session_id": "console:updated-cron-session"
                },
                "mode": "batch",
                "meta": {
                    "source": "contract-test",
                    "priority": 5
                }
            }
        })),
        Some(agent_id),
    )
    .await;
    assert_eq!(update.status(), StatusCode::OK);
    let updated = read_json(update).await;
    assert_eq!(updated["id"], job_id);
    assert_eq!(updated["dispatch"]["channel"], "console");
    assert_eq!(updated["dispatch"]["mode"], "batch");
    assert_eq!(
        updated["dispatch"]["target"]["user_id"],
        "updated-cron-user"
    );
    assert_eq!(
        updated["dispatch"]["target"]["session_id"],
        "console:updated-cron-session"
    );
    assert_eq!(updated["dispatch"]["meta"]["source"], "contract-test");
    assert_eq!(updated["dispatch"]["meta"]["priority"], 5);

    let get = request(
        &app.app,
        Method::GET,
        &format!("/api/cron/jobs/{job_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(get.status(), StatusCode::OK);
    let fetched = read_json(get).await;
    assert_eq!(fetched["dispatch"], updated["dispatch"]);

    let delete = request(
        &app.app,
        Method::DELETE,
        &format!("/api/cron/jobs/{job_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(delete.status(), StatusCode::OK);
    delete_agent(&app.app, agent_id).await;
}
