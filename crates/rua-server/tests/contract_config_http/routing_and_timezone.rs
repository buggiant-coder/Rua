use axum::http::{Method, StatusCode};
use serde_json::json;

use super::support::{build_app, create_agent, delete_agent, read_json, request};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_timezone_roundtrips_through_root_and_agent_scoped_routes() {
    let app = build_app();
    let agent_id = "integ_cfg_user_tz_scoped";
    create_agent(&app.app, agent_id, "User timezone scoped agent").await;

    let root_path = "/api/config/user-timezone";
    let scoped_path = format!("/api/agents/{agent_id}/config/user-timezone");

    let initial = request(&app.app, Method::GET, root_path, None, None).await;
    assert_eq!(initial.status(), StatusCode::OK);
    let initial_body = read_json(initial).await;
    assert!(
        initial_body["timezone"]
            .as_str()
            .is_some_and(|timezone| !timezone.is_empty()),
        "missing user timezone should default to the host timezone, got {initial_body:?}"
    );

    let scoped_put = request(
        &app.app,
        Method::PUT,
        &scoped_path,
        Some(json!({"timezone": "Asia/Shanghai"})),
        None,
    )
    .await;
    assert_eq!(scoped_put.status(), StatusCode::OK);
    assert_eq!(
        read_json(scoped_put).await,
        json!({"timezone": "Asia/Shanghai"})
    );

    let scoped_get = request(&app.app, Method::GET, &scoped_path, None, None).await;
    assert_eq!(
        read_json(scoped_get).await,
        json!({"timezone": "Asia/Shanghai"})
    );

    let root_get = request(&app.app, Method::GET, root_path, None, None).await;
    assert_eq!(
        read_json(root_get).await,
        json!({"timezone": "Asia/Shanghai"})
    );

    let root_put = request(
        &app.app,
        Method::PUT,
        root_path,
        Some(json!({"timezone": "UTC"})),
        None,
    )
    .await;
    assert_eq!(root_put.status(), StatusCode::OK);
    assert_eq!(read_json(root_put).await, json!({"timezone": "UTC"}));

    let scoped_after_root_put = request(&app.app, Method::GET, &scoped_path, None, None).await;
    assert_eq!(
        read_json(scoped_after_root_put).await,
        json!({"timezone": "UTC"})
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_timezone_rejects_empty_value_with_detail_error() {
    let app = build_app();

    let resp = request(
        &app.app,
        Method::PUT,
        "/api/config/user-timezone",
        Some(json!({"timezone": ""})),
        None,
    )
    .await;

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = read_json(resp).await;
    assert_eq!(body["detail"], "timezone is required");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn llm_routing_roundtrips_through_agent_scoped_config_route() {
    let app = build_app();
    let agent_id = "integ_cfg_llm_routing";
    create_agent(&app.app, agent_id, "LLM routing scoped agent").await;

    let path = format!("/api/agents/{agent_id}/config/agents/llm-routing");

    let initial = request(&app.app, Method::GET, &path, None, None).await;
    assert_eq!(initial.status(), StatusCode::OK);
    assert_eq!(
        read_json(initial).await,
        json!({
            "enabled": false,
            "mode": "local_first",
            "local": {"provider_id": "", "model": ""},
            "cloud": null
        })
    );

    let updated = json!({
        "enabled": true,
        "mode": "cloud_first",
        "local": {"provider_id": "ollama", "model": "qwen2.5:7b"},
        "cloud": {"provider_id": "dashscope", "model": "qwen-plus"},
        "policy_note": "kept"
    });
    let put = request(&app.app, Method::PUT, &path, Some(updated.clone()), None).await;
    assert_eq!(put.status(), StatusCode::OK);
    assert_eq!(read_json(put).await, updated);

    let after = request(&app.app, Method::GET, &path, None, None).await;
    assert_eq!(after.status(), StatusCode::OK);
    assert_eq!(read_json(after).await, updated);

    let stored: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            app.data_dir
                .join("workspaces")
                .join(agent_id)
                .join("agent.json"),
        )
        .expect("agent.json"),
    )
    .expect("agent json");
    assert_eq!(stored["llm_routing"], updated);

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn llm_routing_is_isolated_per_agent_and_rejects_invalid_mode_without_mutation() {
    let app = build_app();
    let agent_a = "integ_cfg_llm_route_a";
    let agent_b = "integ_cfg_llm_route_b";
    create_agent(&app.app, agent_a, "LLM routing A").await;
    create_agent(&app.app, agent_b, "LLM routing B").await;

    let path_a = format!("/api/agents/{agent_a}/config/agents/llm-routing");
    let path_b = format!("/api/agents/{agent_b}/config/agents/llm-routing");
    let update_a = json!({
        "enabled": true,
        "mode": "local_first",
        "local": {"provider_id": "ollama", "model": "qwen2.5:7b"},
        "cloud": null
    });

    let put_a = request(&app.app, Method::PUT, &path_a, Some(update_a.clone()), None).await;
    assert_eq!(put_a.status(), StatusCode::OK);
    assert_eq!(read_json(put_a).await, update_a);

    let get_b = request(&app.app, Method::GET, &path_b, None, None).await;
    assert_eq!(get_b.status(), StatusCode::OK);
    assert_eq!(
        read_json(get_b).await,
        json!({
            "enabled": false,
            "mode": "local_first",
            "local": {"provider_id": "", "model": ""},
            "cloud": null
        }),
        "agent A llm_routing must not leak to agent B"
    );

    let invalid = request(
        &app.app,
        Method::PUT,
        &path_a,
        Some(json!({
            "enabled": false,
            "mode": "hybrid",
            "local": {"provider_id": "bad", "model": "bad"},
            "cloud": null
        })),
        None,
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    assert!(read_json(invalid).await["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains("Invalid llm routing mode")));

    let after_invalid = request(&app.app, Method::GET, &path_a, None, None).await;
    assert_eq!(after_invalid.status(), StatusCode::OK);
    assert_eq!(read_json(after_invalid).await, update_a);

    delete_agent(&app.app, agent_a).await;
    delete_agent(&app.app, agent_b).await;
}
