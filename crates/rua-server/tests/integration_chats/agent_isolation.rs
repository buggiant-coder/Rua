use axum::http::{Method, StatusCode};
use serde_json::Value;

use super::support::{
    accept_first_input, build_app, create_agent, create_chat, delete_agent, read_json, request,
};

fn ids(payload: &Value) -> Vec<String> {
    payload
        .as_array()
        .expect("list response array")
        .iter()
        .map(|row| row["id"].as_str().expect("chat id").to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chats_are_isolated_between_agents_for_header_and_scoped_lists() {
    let app = build_app();
    let agent_a = "integ_iso_agent_a";
    let agent_b = "integ_iso_agent_b";
    let shared_user = "integ-isolation-user";
    create_agent(&app.app, agent_a, "Isolation agent A").await;
    create_agent(&app.app, agent_b, "Isolation agent B").await;

    let chat_a = create_chat(
        &app.app,
        "/api/chats",
        Some(agent_a),
        "Isolation chat A",
        shared_user,
    )
    .await;
    let chat_b = create_chat(
        &app.app,
        "/api/chats",
        Some(agent_b),
        "Isolation chat B",
        shared_user,
    )
    .await;
    let chat_a_id = chat_a["id"].as_str().expect("chat a id").to_string();
    let chat_b_id = chat_b["id"].as_str().expect("chat b id").to_string();
    accept_first_input(&app, agent_a, &chat_a_id, shared_user).await;
    accept_first_input(&app, agent_b, &chat_b_id, shared_user).await;

    let list_a = request(
        &app.app,
        Method::GET,
        &format!("/api/chats?user_id={shared_user}"),
        None,
        Some(agent_a),
    )
    .await;
    assert_eq!(list_a.status(), StatusCode::OK);
    let list_a_ids = ids(&read_json(list_a).await);
    assert!(list_a_ids.contains(&chat_a_id));
    assert!(!list_a_ids.contains(&chat_b_id));

    let list_b = request(
        &app.app,
        Method::GET,
        &format!("/api/chats?user_id={shared_user}"),
        None,
        Some(agent_b),
    )
    .await;
    assert_eq!(list_b.status(), StatusCode::OK);
    let list_b_ids = ids(&read_json(list_b).await);
    assert!(list_b_ids.contains(&chat_b_id));
    assert!(!list_b_ids.contains(&chat_a_id));

    let scoped_a = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_a}/chats?user_id={shared_user}"),
        None,
        None,
    )
    .await;
    assert_eq!(scoped_a.status(), StatusCode::OK);
    let scoped_a_ids = ids(&read_json(scoped_a).await);
    assert!(scoped_a_ids.contains(&chat_a_id));
    assert!(!scoped_a_ids.contains(&chat_b_id));

    let scoped_b = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_b}/chats?user_id={shared_user}"),
        None,
        None,
    )
    .await;
    assert_eq!(scoped_b.status(), StatusCode::OK);
    let scoped_b_ids = ids(&read_json(scoped_b).await);
    assert!(scoped_b_ids.contains(&chat_b_id));
    assert!(!scoped_b_ids.contains(&chat_a_id));

    let delete_a = request(
        &app.app,
        Method::DELETE,
        &format!("/api/chats/{chat_a_id}"),
        None,
        Some(agent_a),
    )
    .await;
    assert_eq!(delete_a.status(), StatusCode::OK);
    let delete_b = request(
        &app.app,
        Method::DELETE,
        &format!("/api/chats/{chat_b_id}"),
        None,
        Some(agent_b),
    )
    .await;
    assert_eq!(delete_b.status(), StatusCode::OK);
    delete_agent(&app.app, agent_a).await;
    delete_agent(&app.app, agent_b).await;
}
