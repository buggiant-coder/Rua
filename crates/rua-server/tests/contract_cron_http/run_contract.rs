use axum::http::{Method, StatusCode};
use serde_json::json;

use super::support::{build_app, create_agent, delete_agent, read_json, request};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_missing_job_returns_fastapi_detail_404() {
    let app = build_app();

    let resp = request(
        &app.app,
        Method::POST,
        "/api/cron/jobs/missing-cron-job/run",
        None,
        Some("default"),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert!(read_json(resp).await["detail"]
        .as_str()
        .expect("detail")
        .contains("missing-cron-job"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_text_job_returns_started_without_waiting_for_completion() {
    let app = build_app();
    let agent_id = "integ_cron_run_contract";
    create_agent(&app.app, agent_id, "Cron run contract").await;

    let create = request(
        &app.app,
        Method::POST,
        "/api/cron/jobs",
        Some(json!({
            "name": "contract cron manual run",
            "enabled": true,
            "schedule": {
                "type": "cron",
                "cron": "0 0 * * *",
                "timezone": "UTC"
            },
            "task_type": "text",
            "text": "manual cron text",
            "dispatch": {
                "type": "channel",
                "channel": "console",
                "target": {
                    "user_id": "cron-run-contract-user",
                    "session_id": "console:cron-run-contract-session"
                },
                "mode": "stream"
            }
        })),
        Some(agent_id),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);
    let job_id = read_json(create).await["id"]
        .as_str()
        .expect("job id")
        .to_owned();

    let run = request(
        &app.app,
        Method::POST,
        &format!("/api/cron/jobs/{job_id}/run"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(run.status(), StatusCode::OK);
    assert_eq!(read_json(run).await, json!({ "started": true }));

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
