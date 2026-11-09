use axum::http::StatusCode;
use serial_test::serial;

use super::support::{build_app, get, read_json, write_log};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn backend_logs_missing_file_returns_empty_contract_body() {
    let ctx = build_app();

    let resp = get(&ctx.app, "/api/console/debug/backend-logs").await;
    assert_eq!(resp.status(), StatusCode::OK);

    let body = read_json(resp).await;
    assert_eq!(body["exists"], false);
    assert_eq!(body["lines"], 200);
    assert_eq!(body["updated_at"], serde_json::Value::Null);
    assert_eq!(body["size"], 0);
    assert_eq!(body["content"], "");
    assert!(
        body["path"]
            .as_str()
            .is_some_and(|path| path.ends_with("logs/rua.log")),
        "unexpected path: {body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn backend_logs_returns_tail_content_and_file_metadata() {
    let ctx = build_app();
    let content = (1..=30)
        .map(|i| format!("line-{i:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    write_log(ctx.data_dir(), &content);

    let resp = get(&ctx.app, "/api/console/debug/backend-logs?lines=20").await;
    assert_eq!(resp.status(), StatusCode::OK);

    let body = read_json(resp).await;
    assert_eq!(body["exists"], true);
    assert_eq!(body["lines"], 20);
    assert!(body["updated_at"].as_f64().is_some());
    assert!(body["size"].as_u64().is_some_and(|size| size > 0));

    let returned = body["content"].as_str().expect("content string");
    assert!(returned.starts_with("line-11"), "content was {returned:?}");
    assert!(returned.ends_with("line-30"), "content was {returned:?}");
    assert!(!returned.contains("line-10"), "content was {returned:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn backend_logs_clamps_requested_lines_to_supported_range() {
    let ctx = build_app();

    let too_small = get(&ctx.app, "/api/console/debug/backend-logs?lines=1").await;
    assert_eq!(too_small.status(), StatusCode::OK);
    let too_small_body = read_json(too_small).await;
    assert_eq!(too_small_body["lines"], 20);

    let too_large = get(&ctx.app, "/api/console/debug/backend-logs?lines=99999").await;
    assert_eq!(too_large.status(), StatusCode::OK);
    let too_large_body = read_json(too_large).await;
    assert_eq!(too_large_body["lines"], 5000);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn backend_logs_agent_scoped_route_matches_root_contract() {
    let ctx = build_app();
    write_log(ctx.data_dir(), "scoped-line");

    let resp = get(
        &ctx.app,
        "/api/agents/default/console/debug/backend-logs?lines=20",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let body = read_json(resp).await;
    assert_eq!(body["exists"], true);
    assert_eq!(body["lines"], 20);
    assert_eq!(body["content"], "scoped-line");
}
