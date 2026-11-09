use axum::http::{Method, StatusCode};
use serde_json::json;

use super::support::{build_app, create_agent, delete_agent, read_json, request};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn root_access_control_pending_approve_roundtrip_uses_header_agent() {
    let app = build_app();
    let agent_id = "integ_acl_header";
    create_agent(&app.app, agent_id, "ACL header").await;

    let seed_path = app
        .state
        .rua_root
        .join("workspaces")
        .join(agent_id)
        .join("access_control.json");
    std::fs::create_dir_all(seed_path.parent().expect("acl parent")).unwrap();
    std::fs::write(
        &seed_path,
        r#"{"telegram":{"whitelist":{},"blacklist":{},"pending":[{"user_id":"u1","channel":"telegram","timestamp":2.0,"first_message":"hello","remark":"from pending","username":"Alice"}]}}"#,
    )
    .unwrap();

    let pending = request(
        &app.app,
        Method::GET,
        "/api/access-control/pending/all",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(pending.status(), StatusCode::OK);
    let body = read_json(pending).await;
    assert_eq!(body[0]["user_id"], "u1");
    assert_eq!(body[0]["channel"], "telegram");

    let approve = request(
        &app.app,
        Method::POST,
        "/api/access-control/pending/approve",
        Some(json!({
            "entries": [{"channel": "telegram", "user_id": "u1"}]
        })),
        Some(agent_id),
    )
    .await;
    assert_eq!(approve.status(), StatusCode::OK);
    assert_eq!(
        read_json(approve).await,
        json!({"status": "ok", "count": 1})
    );

    let all = request(
        &app.app,
        Method::GET,
        "/api/access-control",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(all.status(), StatusCode::OK);
    let all_body = read_json(all).await;
    assert_eq!(all_body["telegram"]["pending"], json!([]));
    assert_eq!(
        all_body["telegram"]["whitelist"]["u1"],
        json!({"remark": "from pending", "username": "Alice"})
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_access_control_path_wins_over_conflicting_header() {
    let app = build_app();
    let header_agent = "integ_acl_header_loser";
    let path_agent = "integ_acl_path_winner";
    create_agent(&app.app, header_agent, "ACL header loser").await;
    create_agent(&app.app, path_agent, "ACL path winner").await;

    let add = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{path_agent}/access-control/blacklist/add"),
        Some(json!({
            "entries": [{
                "channel": "dingtalk",
                "user_id": "blocked-user",
                "remark": "spam",
                "username": "Mallory"
            }]
        })),
        Some(header_agent),
    )
    .await;
    assert_eq!(add.status(), StatusCode::OK);

    let path_all = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{path_agent}/access-control"),
        None,
        None,
    )
    .await;
    assert_eq!(path_all.status(), StatusCode::OK);
    assert_eq!(
        read_json(path_all).await["dingtalk"]["blacklist"]["blocked-user"],
        json!({"remark": "spam", "username": "Mallory"})
    );

    let header_all = request(
        &app.app,
        Method::GET,
        "/api/access-control",
        None,
        Some(header_agent),
    )
    .await;
    assert_eq!(header_all.status(), StatusCode::OK);
    assert_eq!(read_json(header_all).await, json!({}));

    delete_agent(&app.app, header_agent).await;
    delete_agent(&app.app, path_agent).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn access_control_missing_pending_remark_returns_fastapi_detail() {
    let app = build_app();
    let agent_id = "integ_acl_missing";
    create_agent(&app.app, agent_id, "ACL missing").await;

    let resp = request(
        &app.app,
        Method::POST,
        "/api/access-control/pending/remark",
        Some(json!({
            "channel": "telegram",
            "user_id": "nobody",
            "remark": "trusted"
        })),
        Some(agent_id),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        read_json(resp).await,
        json!({"detail": "Pending entry not found"})
    );

    delete_agent(&app.app, agent_id).await;
}
