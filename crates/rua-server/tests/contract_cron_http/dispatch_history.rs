use axum::http::{Method, StatusCode};
use serde_json::json;
use std::sync::Arc;

use rua_server::services::cron_dispatch_history::record_cron_dispatch_result;

use super::support::{build_app, create_agent, cron_payload, delete_agent, read_json, request};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cron_state_returns_durable_channel_dispatch_history() {
    let app = build_app();
    let agent_id = "integ_cron_dispatch_history";
    create_agent(&app.app, agent_id, "Cron dispatch history").await;

    let create = request(
        &app.app,
        Method::POST,
        "/api/cron/jobs",
        Some(cron_payload("contract cron dispatch history")),
        Some(agent_id),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);
    let job_id = read_json(create).await["id"]
        .as_str()
        .expect("job id")
        .to_owned();
    let cron_cells = rua_cron::CronCells::new(
        Arc::clone(&app.state.cell_store),
        Arc::clone(&app.state.cron_scheduler),
        app.state
            .rua_root
            .join("workspaces")
            .join(agent_id)
            .join("jobs.json"),
    );
    let spec = cron_cells
        .cell(&job_id)
        .read()
        .await
        .expect("cron cell read")
        .expect("cron job spec");

    record_cron_dispatch_result(&app.state.rua_root, &spec, true, None, 12);
    record_cron_dispatch_result(
        &app.state.rua_root,
        &spec,
        false,
        Some("channel unavailable".into()),
        34,
    );
    record_cron_dispatch_result(
        &app.state.rua_root,
        &spec,
        false,
        Some("max concurrency reached".into()),
        1,
    );
    record_cron_dispatch_result(
        &app.state.rua_root,
        &spec,
        false,
        Some("execution timed out after 1s".into()),
        1000,
    );

    let state = request(
        &app.app,
        Method::GET,
        &format!("/api/cron/jobs/{job_id}/state"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(state.status(), StatusCode::OK);
    let body = read_json(state).await;
    assert_eq!(body["last_status"], "cancelled");
    assert_eq!(body["last_error"], "execution timed out after 1s");
    assert!(body["last_run_at"]
        .as_str()
        .is_some_and(|value| !value.is_empty()));
    let history = body["history"].as_array().expect("history array");
    assert_eq!(history.len(), 4, "body was {body:?}");
    assert_eq!(history[0]["status"], "success");
    assert_eq!(history[1]["status"], "error");
    assert_eq!(history[2]["status"], "skipped");
    assert_eq!(history[3]["status"], "cancelled");
    assert_eq!(history[1]["channel"], "console");
    assert_eq!(history[1]["user_id"], "integ-cron-user");
    assert_eq!(history[1]["session_id"], "console:integ-cron-session");
    assert_eq!(history[1]["duration_ms"], json!(34));

    delete_agent(&app.app, agent_id).await;
}
