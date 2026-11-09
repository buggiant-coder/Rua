use std::sync::Arc;

use axum::{
    http::{Method, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use rua_core::ChatAddress;
use rua_server::services::conversation_authority::{
    ConversationActiveTurnStart, ConversationDriverKind,
};
use rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch;
use serde_json::{json, Value};

use super::support::{build_app, request};

async fn next_line(body: &mut axum::body::Body) -> Value {
    let frame = tokio::time::timeout(std::time::Duration::from_secs(2), body.frame())
        .await
        .expect("activity timeout")
        .expect("activity ended")
        .expect("activity frame");
    serde_json::from_slice(frame.data_ref().unwrap()).unwrap()
}

#[tokio::test]
async fn global_activity_stream_snapshots_running_chats_then_emits_terminal_and_closes_on_shutdown()
{
    let test = build_app();
    let address = ChatAddress::project("project-a", "chat-a");
    let epoch = ConversationAuthorityEpoch::from_parts(10, 0, [1; 8]);
    let turn = test
        .state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: address.clone(),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "local".into(),
            authority_epoch: epoch,
            driver_detail: None,
        })
        .await;
    let app = Router::new()
        .nest("/api", rua_server::endpoints::platform::router())
        .with_state(test.state.clone());
    let response = request(&app, Method::GET, "/api/platform/conversation-activity").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "application/x-ndjson");
    let mut body = response.into_body();
    let snapshot = next_line(&mut body).await;
    assert_eq!(snapshot["type"], "snapshot");
    assert_eq!(snapshot["activities"][0]["address"], json!(address));
    assert_eq!(snapshot["activities"][0]["running"], true);
    let hub = Arc::clone(&test.state.active_stream_broadcast_hub);
    hub.publish_conversation_event(
        "turn",
        &address,
        epoch,
        json!({
            "type": "turn_terminal", "turn_id": turn.turn_id, "status": "completed",
            "completed_at": chrono::Utc::now().to_rfc3339(), "authority_device_id": "local"
        })
        .to_string(),
    )
    .await;
    let update = next_line(&mut body).await;
    assert_eq!(update["type"], "activity");
    assert_eq!(update["address"], json!(address));
    assert_eq!(update["running"], false);
    assert_eq!(update["completion"]["turn_id"], turn.turn_id);
    test.state.signal_shutdown();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(1), body.frame())
            .await
            .unwrap()
            .is_none()
    );
}
