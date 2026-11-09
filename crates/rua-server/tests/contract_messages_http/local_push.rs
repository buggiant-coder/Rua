use axum::http::{Method, StatusCode};
use serde_json::json;

use super::support::{build_app, read_json, request};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn root_send_accepts_qwenpaw_text_payload_and_stores_local_push_message() {
    let test = build_app();

    let send = request(
        &test.app,
        Method::POST,
        "/messages/send",
        Some(json!({
            "channel": "console",
            "target_user": "integ-user",
            "target_session": "integ-session",
            "text": "integration hello"
        })),
        None,
    )
    .await;
    assert_eq!(send.status(), StatusCode::OK);
    let send_body = read_json(send).await;
    assert_eq!(send_body["success"], true);
    assert!(send_body["message_id"]
        .as_str()
        .is_some_and(|id| !id.is_empty()));

    let push = request(
        &test.app,
        Method::GET,
        "/agents/default/console/push-messages?session_id=integ-session",
        None,
        None,
    )
    .await;
    assert_eq!(push.status(), StatusCode::OK);
    let body = read_json(push).await;
    let messages = body["messages"].as_array().expect("messages array");
    assert_eq!(
        messages.len(),
        1,
        "expected one filtered push message: {body:?}"
    );
    assert_eq!(messages[0]["id"], send_body["message_id"]);
    assert_eq!(messages[0]["text"], "integration hello");
    assert_eq!(messages[0]["channel"], "console");
    assert_eq!(messages[0]["target_user"], "integ-user");
    assert_eq!(messages[0]["target_session"], "integ-session");
    assert_eq!(messages[0]["session_id"], "integ-session");
    assert_eq!(messages[0]["agent_id"], "default");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn root_push_keeps_rust_message_field_contract() {
    let test = build_app();

    let push = request(
        &test.app,
        Method::POST,
        "/messages/push",
        Some(json!({
            "channel": "console",
            "message": "rust message payload"
        })),
        Some("default"),
    )
    .await;
    assert_eq!(push.status(), StatusCode::OK);
    let push_body = read_json(push).await;
    assert_eq!(push_body["success"], true);

    let listed = request(
        &test.app,
        Method::GET,
        "/agents/default/console/push-messages",
        None,
        Some("default"),
    )
    .await;
    let body = read_json(listed).await;
    assert_eq!(body["messages"][0]["id"], push_body["message_id"]);
    assert_eq!(body["messages"][0]["text"], "rust message payload");
    assert_eq!(body["messages"][0]["channel"], "console");
    assert_eq!(body["messages"][0]["agent_id"], "default");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn root_send_accepts_message_only_payload_without_target_session() {
    let test = build_app();

    let send = request(
        &test.app,
        Method::POST,
        "/messages/send",
        Some(json!({
            "channel": "console",
            "message": "message-only local push"
        })),
        Some("default"),
    )
    .await;
    assert_eq!(send.status(), StatusCode::OK);
    let send_body = read_json(send).await;
    assert_eq!(send_body["success"], true);

    let push = request(
        &test.app,
        Method::GET,
        "/agents/default/console/push-messages",
        None,
        Some("default"),
    )
    .await;
    assert_eq!(push.status(), StatusCode::OK);
    let body = read_json(push).await;
    let messages = body["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["id"], send_body["message_id"]);
    assert_eq!(messages[0]["text"], "message-only local push");
    assert_eq!(messages[0]["channel"], "console");
    assert_eq!(messages[0]["agent_id"], "default");
    assert!(
        messages[0].get("target_session").is_none(),
        "Rust local-push messages are not required to target an existing session"
    );
    assert!(
        messages[0].get("session_id").is_none(),
        "session_id is only synthesized from target_session when present"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn root_send_preserves_unknown_channel_as_local_push_metadata() {
    let test = build_app();

    let send = request(
        &test.app,
        Method::POST,
        "/messages/send",
        Some(json!({
            "channel": "nonexistent_channel",
            "target_user": "integ-user",
            "target_session": "unknown-channel-session",
            "text": "local push with metadata channel"
        })),
        Some("default"),
    )
    .await;
    assert_eq!(send.status(), StatusCode::OK);
    let send_body = read_json(send).await;
    assert_eq!(send_body["success"], true);

    let listed = request(
        &test.app,
        Method::GET,
        "/agents/default/console/push-messages?session_id=unknown-channel-session",
        None,
        Some("default"),
    )
    .await;
    assert_eq!(listed.status(), StatusCode::OK);
    let body = read_json(listed).await;
    let messages = body["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 1, "expected local push event: {body:?}");
    assert_eq!(messages[0]["id"], send_body["message_id"]);
    assert_eq!(messages[0]["text"], "local push with metadata channel");
    assert_eq!(messages[0]["channel"], "nonexistent_channel");
    assert_eq!(messages[0]["agent_id"], "default");
    assert_eq!(messages[0]["target_user"], "integ-user");
    assert_eq!(messages[0]["target_session"], "unknown-channel-session");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_send_rejects_missing_or_empty_message_content() {
    let test = build_app();

    for (route, payload) in [
        ("/messages/send", json!({"channel": "console"})),
        (
            "/messages/push",
            json!({"channel": "console", "message": "   "}),
        ),
        (
            "/agents/messages_agent/messages/send",
            json!({"channel": "console", "text": ""}),
        ),
    ] {
        let resp = request(
            &test.app,
            Method::POST,
            route,
            Some(payload),
            Some("default"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            read_json(resp).await,
            json!({"detail": "message or text is required"})
        );
    }

    let listed = request(
        &test.app,
        Method::GET,
        "/agents/default/console/push-messages",
        None,
        Some("default"),
    )
    .await;
    assert_eq!(listed.status(), StatusCode::OK);
    assert_eq!(read_json(listed).await["messages"], json!([]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_message_payloads_return_validation_errors_without_writing_push_messages() {
    let test = build_app();

    for payload in [json!("not-an-object"), json!(["message", "array"])] {
        let resp = request(
            &test.app,
            Method::POST,
            "/messages/send",
            Some(payload),
            Some("default"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = read_json(resp).await;
        assert!(
            body["detail"].is_array(),
            "axum/serde validation should expose a FastAPI-style detail array: {body:?}"
        );
    }

    let listed = request(
        &test.app,
        Method::GET,
        "/agents/default/console/push-messages",
        None,
        Some("default"),
    )
    .await;
    assert_eq!(listed.status(), StatusCode::OK);
    assert_eq!(read_json(listed).await["messages"], json!([]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn root_send_honors_agent_header_and_push_listing_is_agent_scoped() {
    let test = build_app();

    let alpha = request(
        &test.app,
        Method::POST,
        "/messages/send",
        Some(json!({
            "channel": "console",
            "target_session": "alpha-session",
            "text": "alpha local push"
        })),
        Some("messages_alpha"),
    )
    .await;
    assert_eq!(alpha.status(), StatusCode::OK);
    let alpha_body = read_json(alpha).await;

    let beta = request(
        &test.app,
        Method::POST,
        "/agents/messages_beta/messages/send",
        Some(json!({
            "channel": "console",
            "target_session": "beta-session",
            "text": "beta local push"
        })),
        Some("messages_alpha"),
    )
    .await;
    assert_eq!(beta.status(), StatusCode::OK);
    let beta_body = read_json(beta).await;

    let alpha_list = request(
        &test.app,
        Method::GET,
        "/agents/messages_alpha/console/push-messages",
        None,
        Some("messages_alpha"),
    )
    .await;
    assert_eq!(alpha_list.status(), StatusCode::OK);
    let alpha_list_body = read_json(alpha_list).await;
    let alpha_messages = alpha_list_body["messages"]
        .as_array()
        .expect("messages array");
    assert_eq!(
        alpha_messages.len(),
        1,
        "root listing must use X-Agent-Id and exclude path-scoped pushes: {alpha_list_body:?}"
    );
    assert_eq!(alpha_messages[0]["id"], alpha_body["message_id"]);
    assert_eq!(alpha_messages[0]["agent_id"], "messages_alpha");
    assert_eq!(alpha_messages[0]["text"], "alpha local push");

    let beta_list = request(
        &test.app,
        Method::GET,
        "/agents/messages_beta/console/push-messages",
        None,
        Some("messages_alpha"),
    )
    .await;
    assert_eq!(beta_list.status(), StatusCode::OK);
    let beta_list_body = read_json(beta_list).await;
    let beta_messages = beta_list_body["messages"]
        .as_array()
        .expect("messages array");
    assert_eq!(
        beta_messages.len(),
        1,
        "path-scoped listing must prefer path agent over conflicting header: {beta_list_body:?}"
    );
    assert_eq!(beta_messages[0]["id"], beta_body["message_id"]);
    assert_eq!(beta_messages[0]["agent_id"], "messages_beta");
    assert_eq!(beta_messages[0]["text"], "beta local push");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn payload_agent_id_cannot_override_header_or_path_scope() {
    let test = build_app();

    let root = request(
        &test.app,
        Method::POST,
        "/messages/send",
        Some(json!({
            "agent_id": "payload_agent",
            "channel": "console",
            "target_session": "header-session",
            "text": "header-scoped local push"
        })),
        Some("header_agent"),
    )
    .await;
    assert_eq!(root.status(), StatusCode::OK);
    let root_body = read_json(root).await;

    let path = request(
        &test.app,
        Method::POST,
        "/agents/path_agent/messages/send",
        Some(json!({
            "agent_id": "payload_agent",
            "channel": "console",
            "target_session": "path-session",
            "text": "path-scoped local push"
        })),
        Some("header_agent"),
    )
    .await;
    assert_eq!(path.status(), StatusCode::OK);
    let path_body = read_json(path).await;

    let header_list = request(
        &test.app,
        Method::GET,
        "/agents/header_agent/console/push-messages",
        None,
        Some("header_agent"),
    )
    .await;
    assert_eq!(header_list.status(), StatusCode::OK);
    let header_body = read_json(header_list).await;
    let header_messages = header_body["messages"].as_array().expect("messages array");
    assert_eq!(
        header_messages.len(),
        1,
        "root send should use X-Agent-Id, not the body agent_id: {header_body:?}"
    );
    assert_eq!(header_messages[0]["id"], root_body["message_id"]);
    assert_eq!(header_messages[0]["agent_id"], "header_agent");
    assert_eq!(header_messages[0]["text"], "header-scoped local push");

    let path_list = request(
        &test.app,
        Method::GET,
        "/agents/path_agent/console/push-messages",
        None,
        Some("header_agent"),
    )
    .await;
    assert_eq!(path_list.status(), StatusCode::OK);
    let path_body_list = read_json(path_list).await;
    let path_messages = path_body_list["messages"]
        .as_array()
        .expect("messages array");
    assert_eq!(
        path_messages.len(),
        1,
        "agent-scoped send should use the path agent, not header or body agent_id: {path_body_list:?}"
    );
    assert_eq!(path_messages[0]["id"], path_body["message_id"]);
    assert_eq!(path_messages[0]["agent_id"], "path_agent");
    assert_eq!(path_messages[0]["text"], "path-scoped local push");

    let payload_list = request(
        &test.app,
        Method::GET,
        "/agents/payload_agent/console/push-messages",
        None,
        None,
    )
    .await;
    assert_eq!(payload_list.status(), StatusCode::OK);
    assert_eq!(read_json(payload_list).await["messages"], json!([]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_send_tags_path_agent_without_dispatching_channel() {
    let test = build_app();

    let send = request(
        &test.app,
        Method::POST,
        "/agents/messages_agent/messages/send",
        Some(json!({
            "channel": "nonexistent_channel",
            "text": "agent-scoped local push",
            "target_session": "agent-session"
        })),
        None,
    )
    .await;
    assert_eq!(send.status(), StatusCode::OK);
    let send_body = read_json(send).await;
    assert_eq!(send_body["success"], true);

    let listed = request(
        &test.app,
        Method::GET,
        "/agents/messages_agent/console/push-messages?session_id=agent-session",
        None,
        None,
    )
    .await;
    assert_eq!(listed.status(), StatusCode::OK);
    let body = read_json(listed).await;
    let messages = body["messages"].as_array().expect("messages array");
    assert_eq!(
        messages.len(),
        1,
        "expected one filtered push message: {body:?}"
    );
    assert_eq!(messages[0]["id"], send_body["message_id"]);
    assert_eq!(messages[0]["text"], "agent-scoped local push");
    assert_eq!(messages[0]["agent_id"], "messages_agent");
    assert_eq!(messages[0]["channel"], "nonexistent_channel");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proactive_send_does_not_create_chat_records() {
    let test = build_app();

    let before = request(&test.app, Method::GET, "/chats", None, Some("default")).await;
    assert_eq!(before.status(), StatusCode::OK);
    let before_ids: std::collections::BTreeSet<String> = read_json(before)
        .await
        .as_array()
        .expect("chat list array")
        .iter()
        .filter_map(|chat| chat["id"].as_str().map(ToString::to_string))
        .collect();

    let send = request(
        &test.app,
        Method::POST,
        "/messages/send",
        Some(json!({
            "channel": "console",
            "target_user": "integ-user-chat-boundary",
            "target_session": "integ-session-chat-boundary",
            "text": "boundary check"
        })),
        Some("default"),
    )
    .await;
    assert_eq!(send.status(), StatusCode::OK);
    assert_eq!(read_json(send).await["success"], true);

    let push = request(
        &test.app,
        Method::GET,
        "/agents/default/console/push-messages?session_id=integ-session-chat-boundary",
        None,
        Some("default"),
    )
    .await;
    assert_eq!(push.status(), StatusCode::OK);
    assert_eq!(
        read_json(push).await["messages"]
            .as_array()
            .expect("push messages")
            .len(),
        1
    );

    let after = request(&test.app, Method::GET, "/chats", None, Some("default")).await;
    assert_eq!(after.status(), StatusCode::OK);
    let after_ids: std::collections::BTreeSet<String> = read_json(after)
        .await
        .as_array()
        .expect("chat list array")
        .iter()
        .filter_map(|chat| chat["id"].as_str().map(ToString::to_string))
        .collect();
    assert_eq!(
        after_ids, before_ids,
        "proactive messages are local push events and must not create chat rows"
    );
}
