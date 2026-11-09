use axum::http::{Method, StatusCode};
use serde_json::json;

use super::support::{build_app, create_agent, delete_agent, read_json, request};

fn heartbeat_payload(enabled: bool, every: &str, target: &str) -> serde_json::Value {
    json!({
        "enabled": enabled,
        "every": every,
        "target": target,
        "active_hours": {
            "start": "09:00",
            "end": "18:00"
        }
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn heartbeat_roundtrips_on_header_scoped_config_route() {
    let app = build_app();
    let agent_id = "integ_cfg_heartbeat_header";
    create_agent(&app.app, agent_id, "Heartbeat header agent").await;

    let before = request(
        &app.app,
        Method::GET,
        "/api/config/heartbeat",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(before.status(), StatusCode::OK);
    let before_body = read_json(before).await;
    assert_eq!(before_body["enabled"], false);
    assert_eq!(before_body["every"], "");
    assert_eq!(before_body["target"], "");

    let updated = heartbeat_payload(true, "30m", "console");
    let put = request(
        &app.app,
        Method::PUT,
        "/api/config/heartbeat",
        Some(updated.clone()),
        Some(agent_id),
    )
    .await;
    assert_eq!(put.status(), StatusCode::OK);
    assert_eq!(read_json(put).await, updated);

    let after = request(
        &app.app,
        Method::GET,
        "/api/config/heartbeat",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(read_json(after).await, updated);

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn heartbeat_agent_scoped_route_is_isolated_from_header_agent() {
    let app = build_app();
    let header_agent = "integ_cfg_heartbeat_header_isolated";
    let scoped_agent = "integ_cfg_heartbeat_scoped";
    create_agent(&app.app, header_agent, "Heartbeat header isolated").await;
    create_agent(&app.app, scoped_agent, "Heartbeat scoped agent").await;

    let header_payload = heartbeat_payload(true, "15m", "email");
    let header_put = request(
        &app.app,
        Method::PUT,
        "/api/config/heartbeat",
        Some(header_payload.clone()),
        Some(header_agent),
    )
    .await;
    assert_eq!(header_put.status(), StatusCode::OK);

    let scoped_path = format!("/api/agents/{scoped_agent}/config/heartbeat");
    let scoped_before = request(&app.app, Method::GET, &scoped_path, None, None).await;
    assert_eq!(scoped_before.status(), StatusCode::OK);
    let scoped_before_body = read_json(scoped_before).await;
    assert_eq!(scoped_before_body["enabled"], false);
    assert_eq!(scoped_before_body["every"], "");

    let scoped_payload = heartbeat_payload(false, "1h", "console");
    let scoped_put = request(
        &app.app,
        Method::PUT,
        &scoped_path,
        Some(scoped_payload.clone()),
        None,
    )
    .await;
    assert_eq!(scoped_put.status(), StatusCode::OK);
    assert_eq!(read_json(scoped_put).await, scoped_payload);

    let scoped_after = request(&app.app, Method::GET, &scoped_path, None, None).await;
    assert_eq!(read_json(scoped_after).await, scoped_payload);

    let header_after = request(
        &app.app,
        Method::GET,
        "/api/config/heartbeat",
        None,
        Some(header_agent),
    )
    .await;
    assert_eq!(read_json(header_after).await, header_payload);

    delete_agent(&app.app, header_agent).await;
    delete_agent(&app.app, scoped_agent).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn heartbeat_manual_run_returns_started_on_agent_scoped_route() {
    let app = build_app();
    let agent_id = "integ_cfg_heartbeat_run_scoped";
    create_agent(&app.app, agent_id, "Heartbeat manual run scoped").await;

    let resp = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/config/heartbeat/run"),
        None,
        None,
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(read_json(resp).await, json!({ "started": true }));

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn heartbeat_manual_run_uses_header_selected_agent_on_root_route() {
    let app = build_app();
    let agent_id = "integ_cfg_heartbeat_run_header";
    create_agent(&app.app, agent_id, "Heartbeat manual run header").await;

    let resp = request(
        &app.app,
        Method::POST,
        "/api/config/heartbeat/run",
        None,
        Some(agent_id),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(read_json(resp).await, json!({ "started": true }));

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn heartbeat_config_persists_after_agent_disable_enable() {
    let app = build_app();
    let agent_id = "integ_cfg_heartbeat_toggle_persist";
    create_agent(&app.app, agent_id, "Heartbeat toggle persistence").await;

    let path = format!("/api/agents/{agent_id}/config/heartbeat");
    let updated = heartbeat_payload(true, "45m", "console");
    let put = request(&app.app, Method::PUT, &path, Some(updated.clone()), None).await;
    assert_eq!(put.status(), StatusCode::OK);

    let disable = request(
        &app.app,
        Method::PATCH,
        &format!("/api/agents/{agent_id}/toggle"),
        Some(json!({ "enabled": false })),
        None,
    )
    .await;
    assert_eq!(disable.status(), StatusCode::OK);

    let enable = request(
        &app.app,
        Method::PATCH,
        &format!("/api/agents/{agent_id}/toggle"),
        Some(json!({ "enabled": true })),
        None,
    )
    .await;
    assert_eq!(enable.status(), StatusCode::OK);

    let after = request(&app.app, Method::GET, &path, None, None).await;
    assert_eq!(after.status(), StatusCode::OK);
    assert_eq!(
        read_json(after).await,
        updated,
        "agent enable/disable must not reset scoped heartbeat config"
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_recreate_agent_gets_fresh_heartbeat_config() {
    let app = build_app();
    let agent_id = "integ_cfg_heartbeat_recreate";
    create_agent(&app.app, agent_id, "Heartbeat recreate original").await;

    let path = format!("/api/agents/{agent_id}/config/heartbeat");
    let updated = heartbeat_payload(true, "20m", "email");
    let put = request(&app.app, Method::PUT, &path, Some(updated), None).await;
    assert_eq!(put.status(), StatusCode::OK);

    delete_agent(&app.app, agent_id).await;
    create_agent(&app.app, agent_id, "Heartbeat recreate fresh").await;

    let after = request(&app.app, Method::GET, &path, None, None).await;
    assert_eq!(after.status(), StatusCode::OK);
    let after_body = read_json(after).await;
    assert_eq!(after_body["enabled"], false);
    assert_eq!(after_body["every"], "");
    assert_eq!(after_body["target"], "");

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn heartbeat_null_section_reads_as_default_and_put_replaces_only_heartbeat() {
    let app = build_app();
    let agent_id = "integ_cfg_heartbeat_null_section";
    create_agent(&app.app, agent_id, "Heartbeat null section agent").await;

    let agent_path = app
        .data_dir
        .join("workspaces")
        .join(agent_id)
        .join("agent.json");
    let mut stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&agent_path).expect("agent.json"))
            .expect("agent json");
    stored["heartbeat"] = serde_json::Value::Null;
    stored["qwenpaw_alignment_marker"] = json!("keep-me");
    rua_config::atomic_write_json(&agent_path, &stored).expect("seed null heartbeat");

    let root_get = request(
        &app.app,
        Method::GET,
        "/api/config/heartbeat",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(root_get.status(), StatusCode::OK);
    assert_eq!(
        read_json(root_get).await,
        json!({
            "enabled": false,
            "every": "",
            "target": "",
            "active_hours": null
        })
    );

    let path = format!("/api/agents/{agent_id}/config/heartbeat");
    let scoped_get = request(&app.app, Method::GET, &path, None, None).await;
    assert_eq!(scoped_get.status(), StatusCode::OK);
    assert_eq!(
        read_json(scoped_get).await,
        json!({
            "enabled": false,
            "every": "",
            "target": "",
            "active_hours": null
        })
    );

    let updated = heartbeat_payload(true, "1h", "console");
    let put = request(&app.app, Method::PUT, &path, Some(updated.clone()), None).await;
    assert_eq!(put.status(), StatusCode::OK);
    assert_eq!(read_json(put).await, updated);

    let after: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&agent_path).expect("agent.json after put"))
            .expect("agent json after put");
    assert_eq!(after["heartbeat"], updated);
    assert_eq!(after["qwenpaw_alignment_marker"], "keep-me");

    delete_agent(&app.app, agent_id).await;
}
