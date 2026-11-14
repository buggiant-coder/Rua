use axum::http::{Method, StatusCode};
use serde_json::{json, Value};

use super::support::{build_app, read_json, request};

const PATH: &str = "/api/agents/tool_contract_agent/tools/selection-policy";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn policy_preview_is_read_only_and_apply_checks_revision() {
    let test = build_app();
    let path = test.agent_json_path("tool_contract_agent");
    let mut agent: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    agent["tools"] = json!({
        "selection_policy": {"mode":"restricted","allowed_tools":[{"kind":"builtin","name":"read"}]},
        "builtin_tools": {
            "read": {"name":"read", "enabled":true},
            "edit": {"name":"edit", "enabled":false},
            "bash": {"name":"bash", "enabled":true, "config":{"api_key":"private-key"}}
        }
    });
    rua_config::atomic_write_json(&path, &agent).unwrap();
    let before = std::fs::read(&path).unwrap();

    let response = request(&test.app, Method::GET, PATH, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        read_json(response).await["selection_policy"],
        agent["tools"]["selection_policy"]
    );
    let candidate =
        json!({"mode":"default_enabled", "disabled_tools":[], "disabled_capabilities":[]});
    let response = request(
        &test.app,
        Method::POST,
        &format!("{PATH}/preview"),
        Some(json!({"selection_policy":candidate})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let preview = read_json(response).await;
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(!preview.to_string().contains("private-key"));
    assert_eq!(preview["current"], agent["tools"]["selection_policy"]);
    assert_eq!(preview["candidate"], candidate);
    assert_eq!(preview.as_object().unwrap().len(), 3);

    let apply = json!({"expected":preview["expected"], "candidate":preview["candidate"]});
    let response = request(&test.app, Method::PUT, PATH, Some(apply.clone())).await;
    assert_eq!(response.status(), StatusCode::OK);
    let saved = read_json(response).await;
    assert_eq!(saved["selection_policy"], candidate);
    let response = request(&test.app, Method::PUT, PATH, Some(apply)).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(read_json(response).await["detail"].is_string());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_selection_policy_is_invalid_and_read_only() {
    let test = build_app();
    let path = test.agent_json_path("tool_contract_agent");
    let mut agent: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    agent["tools"]
        .as_object_mut()
        .unwrap()
        .remove("selection_policy");
    rua_config::atomic_write_json(&path, &agent).unwrap();
    let before = std::fs::read(&path).unwrap();
    for (method, path, body) in [
        (Method::GET, PATH.to_owned(), None),
        (
            Method::POST,
            format!("{PATH}/preview"),
            Some(json!({
                "selection_policy":{"mode":"default_enabled","disabled_tools":[],"disabled_capabilities":[]}
            })),
        ),
    ] {
        let response = request(&test.app, method, &path, body).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(read_json(response).await["detail"]
            .as_str()
            .unwrap()
            .contains("required"));
    }
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restricted_policy_and_invalid_input_use_the_same_agent_surface() {
    let test = build_app();
    let policy = json!({"mode":"restricted","allowed_tools":[{"kind":"builtin","name":"read"}]});
    let response = request(
        &test.app,
        Method::POST,
        &format!("{PATH}/preview"),
        Some(json!({"selection_policy":policy})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let preview = read_json(response).await;
    let response = request(
        &test.app,
        Method::PUT,
        PATH,
        Some(json!({"expected":preview["expected"],"candidate":preview["candidate"]})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = request(
        &test.app,
        Method::GET,
        "/api/agents/tool_contract_agent/tools",
        None,
    )
    .await;
    let tools = read_json(response).await;
    let enabled: Vec<_> = tools
        .as_array()
        .unwrap()
        .iter()
        .filter(|tool| tool["enabled"] == true)
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(enabled, ["read"]);
    let response = request(
        &test.app,
        Method::POST,
        &format!("{PATH}/preview"),
        Some(json!({"selection_policy":{"mode":"default_enabled"}})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(read_json(response).await["detail"].is_string());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_agent_policy_routes_never_create_configuration() {
    let test = build_app();
    let path = "/api/agents/unknown-policy-agent/tools/selection-policy";
    let policy = json!({"mode":"default_enabled","disabled_tools":[],"disabled_capabilities":[]});
    for (method, path, body) in [
        (Method::GET, path.to_string(), None),
        (
            Method::POST,
            format!("{path}/preview"),
            Some(json!({"selection_policy":policy})),
        ),
        (
            Method::PUT,
            path.to_string(),
            Some(json!({"expected":null,"candidate":policy})),
        ),
    ] {
        let response = request(&test.app, method, &path, body).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(read_json(response).await["detail"].is_string());
    }
    assert!(!test.agent_json_path("unknown-policy-agent").exists());
}
