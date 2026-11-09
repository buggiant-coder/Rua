use std::convert::Infallible;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use axum::http::{Method, StatusCode};
use http_body_util::BodyExt;
use rua_channel::core::console::{ConsoleChatFacade, SseOutputStream};
use rua_core::ChatAddress;
use serde_json::Value;

use super::support::{build_app, build_app_with_console_facade, request, request_json};

struct FixedLiveFacade;

struct CapturingLiveFacade {
    position: Mutex<Option<rua_core::ConversationActivityPosition>>,
}

#[async_trait]
impl ConsoleChatFacade for FixedLiveFacade {
    async fn connect_chat_activity(
        &self,
        _agent_id: &str,
        _chat_id: &str,
        _viewer_id: &str,
        _position: Option<rua_core::ConversationActivityPosition>,
    ) -> SseOutputStream {
        fixed_live_stream()
    }

    async fn connect_scoped_chat_activity(
        &self,
        _address: ChatAddress,
        _execution_agent_id: &str,
        _viewer_id: &str,
        _position: Option<rua_core::ConversationActivityPosition>,
    ) -> SseOutputStream {
        fixed_live_stream()
    }

    async fn stop_chat(&self, _agent_id: &str, _chat_id: &str) -> bool {
        false
    }
}

#[async_trait]
impl ConsoleChatFacade for CapturingLiveFacade {
    async fn connect_chat_activity(
        &self,
        _agent_id: &str,
        _chat_id: &str,
        _viewer_id: &str,
        position: Option<rua_core::ConversationActivityPosition>,
    ) -> SseOutputStream {
        *self.position.lock().expect("position lock") = position;
        fixed_live_stream()
    }

    async fn stop_chat(&self, _agent_id: &str, _chat_id: &str) -> bool {
        false
    }
}

fn fixed_live_stream() -> SseOutputStream {
    Box::pin(futures::stream::once(async {
        Ok::<_, Infallible>(r#"{"type":"stream_event"}"#.to_string())
    }))
}

async fn next_json_line(body: &mut axum::body::Body) -> Value {
    let frame = tokio::time::timeout(std::time::Duration::from_secs(1), body.frame())
        .await
        .expect("activity stream should yield a frame")
        .expect("activity stream ended before wake")
        .expect("activity stream frame");
    let data = frame.data_ref().expect("activity stream data frame");
    let text = std::str::from_utf8(data).expect("activity stream utf8");
    let line = text.lines().next().expect("activity stream json line");
    serde_json::from_str(line).expect("activity stream json payload")
}

async fn next_started_wake(body: &mut axum::body::Body) -> Value {
    loop {
        let value = next_json_line(body).await;
        if value["phase"] == "started" {
            return value;
        }
    }
}

#[tokio::test]
async fn chat_live_requires_a_viewer_identity_without_changing_submit_requests() {
    let test = build_app();
    let missing_viewer = request_json(
        &test.app,
        Method::POST,
        "/agents/default/chats/00000000-0000-4000-8000-000000000431/console/live",
        serde_json::json!({ "chat_id": "00000000-0000-4000-8000-000000000431" }),
    )
    .await;
    assert_eq!(missing_viewer.status(), StatusCode::BAD_REQUEST);

    let live = request_json(
        &test.app,
        Method::POST,
        "/agents/default/chats/00000000-0000-4000-8000-000000000431/console/live",
        serde_json::json!({
            "chat_id": "00000000-0000-4000-8000-000000000431",
            "viewer_id": "browser-one"
        }),
    )
    .await;
    assert_eq!(live.status(), StatusCode::OK);
    assert_eq!(live.headers()["content-type"], "text/event-stream");
    assert_eq!(live.headers()["cache-control"], "no-cache");
    assert_eq!(live.headers()["x-accel-buffering"], "no");
}

#[tokio::test]
async fn chat_live_encodes_payloads_as_standard_sse_data_frames() {
    let test = build_app_with_console_facade(Arc::new(FixedLiveFacade));
    let live = request_json(
        &test.app,
        Method::POST,
        "/agents/default/chats/00000000-0000-4000-8000-000000000431/console/live",
        serde_json::json!({
            "chat_id": "00000000-0000-4000-8000-000000000431",
            "viewer_id": "browser-one"
        }),
    )
    .await;

    assert_eq!(live.status(), StatusCode::OK);
    assert_eq!(live.headers()["content-type"], "text/event-stream");
    let body = live.into_body().collect().await.expect("Live SSE body");
    assert_eq!(
        body.to_bytes(),
        r#"data: {"type":"stream_event"}

"#
    );
}

#[tokio::test]
async fn chat_live_forwards_the_conversation_activity_position() {
    let facade = Arc::new(CapturingLiveFacade {
        position: Mutex::new(None),
    });
    let test = build_app_with_console_facade(facade.clone());
    let live = request_json(
        &test.app,
        Method::POST,
        "/agents/default/chats/00000000-0000-4000-8000-000000000431/console/live",
        serde_json::json!({
            "chat_id": "00000000-0000-4000-8000-000000000431",
            "viewer_id": "browser-one",
            "activity_generation": "generation-a",
            "next_activity_sequence": 42
        }),
    )
    .await;

    assert_eq!(live.status(), StatusCode::OK);
    assert_eq!(
        facade.position.lock().expect("position lock").as_ref(),
        Some(&rua_core::ConversationActivityPosition::new(
            "generation-a",
            42,
        ))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_activity_stream_emits_wakes_after_each_started_run() {
    let test = build_app();
    let response = request(
        &test.app,
        Method::GET,
        "/agents/default/chats/chat-one/console/activity",
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let mut body = response.into_body();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    test.state
        .active_stream_broadcast_hub
        .publish_event(
            "stream-one",
            &ChatAddress::agent("default", "chat-one"),
            "ignored token".to_string(),
        )
        .await;
    let first = next_started_wake(&mut body).await;
    assert_eq!(first["type"], "chat_live_wake");
    assert_eq!(first["agent_id"], "default");
    assert_eq!(first["chat_id"], "chat-one");
    assert_eq!(first["stream_id"], "stream-one");

    test.state
        .active_stream_broadcast_hub
        .finish("stream-one", &ChatAddress::agent("default", "chat-one"))
        .await;
    test.state
        .active_stream_broadcast_hub
        .publish_event(
            "stream-two",
            &ChatAddress::agent("default", "chat-two"),
            "ignored token".to_string(),
        )
        .await;
    let second = next_started_wake(&mut body).await;
    assert_eq!(second["type"], "chat_live_wake");
    assert_eq!(second["agent_id"], "default");
    assert_eq!(second["chat_id"], "chat-two");
    assert_eq!(second["stream_id"], "stream-two");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_activity_stream_forwards_gossip_wake_without_refreshing_peers() {
    let test = build_app();
    let response = request(
        &test.app,
        Method::GET,
        "/agents/default/chats/remote-chat/console/activity",
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    test.state.chat_activity_wake_hub.publish(
        rua_server::services::chat_activity_wake::ChatLiveWake {
            kind: "chat_live_wake".to_string(),
            version: 2,
            address: ChatAddress::agent("default", "remote-chat"),
            stream_id: "remote-stream".to_string(),
            phase: "started".to_string(),
            authority_device_id: "remote-device".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(7, 0, [0; 8]),
            base_manifest_hash: None,
            activity: None,
        },
    );

    let wake = next_started_wake(&mut body).await;
    assert_eq!(wake["chat_id"], "remote-chat");
    assert_eq!(wake["authority_device_id"], "remote-device");
    assert_eq!(
        wake["authority_epoch"],
        "0000000000000007000000000000000000000000"
    );

    let runtime = test.state.device_network.lock().await.as_ref().cloned();
    if let Some(runtime) = runtime {
        assert_eq!(
            runtime
                .session_manager()
                .diagnostics()
                .physical_dial_attempts,
            0
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_chat_activity_stream_is_scoped_to_the_complete_chat_address() {
    let test = build_app();
    let repository = test.state.rua_root.join("project-chat-activity");
    std::fs::create_dir_all(&repository).expect("project repository");
    let project = test
        .state
        .project_catalog
        .create_bound(rua_project::ProjectCreateRequest {
            resource_device_id: "resource-device".to_string(),
            resource_root: repository.to_string_lossy().to_string(),
            display_name: Some("Activity Project".to_string()),
        })
        .expect("create Project")
        .project;
    let project_id = project.project_id.as_str();
    let chat_id = "00000000-0000-4000-8000-000000000201";
    let other_chat_id = "00000000-0000-4000-8000-000000000202";
    let response = request(
        &test.app,
        Method::GET,
        &format!("/api/projects/{project_id}/chats/{chat_id}/console/activity"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    test.state
        .active_stream_broadcast_hub
        .publish_event(
            "agent-same-chat",
            &ChatAddress::agent(project_id, chat_id),
            "ignored token".to_string(),
        )
        .await;
    test.state
        .active_stream_broadcast_hub
        .publish_event(
            "project-other-chat",
            &ChatAddress::project(project_id, other_chat_id),
            "ignored token".to_string(),
        )
        .await;
    test.state
        .active_stream_broadcast_hub
        .publish_event(
            "project-target-chat",
            &ChatAddress::project(project_id, chat_id),
            "ignored token".to_string(),
        )
        .await;

    let wake = next_started_wake(&mut body).await;
    assert_eq!(wake["stream_id"], "project-target-chat");
    assert_eq!(wake["project_id"], project_id);
    assert_eq!(wake["chat_id"], chat_id);
    assert_eq!(
        wake["address"],
        serde_json::json!({
            "scope": {
                "kind": "project",
                "project_id": project_id,
            },
            "chat_id": chat_id,
        })
    );

    for (address, stream_id) in [
        (
            ChatAddress::agent(project_id, chat_id),
            "remote-agent-same-chat",
        ),
        (
            ChatAddress::project(project_id, other_chat_id),
            "remote-project-other-chat",
        ),
        (
            ChatAddress::project(project_id, chat_id),
            "remote-project-target-chat",
        ),
    ] {
        test.state.chat_activity_wake_hub.publish(
            rua_server::services::chat_activity_wake::ChatLiveWake {
                kind: "chat_live_wake".to_string(),
                version: 2,
                address,
                stream_id: stream_id.to_string(),
                phase: "started".to_string(),
                authority_device_id: "remote-resource-device".to_string(),
                authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(
                    8,
                    0,
                    [0; 8],
                ),
                base_manifest_hash: None,
            activity: None,
            },
        );
    }
    let remote_wake = next_started_wake(&mut body).await;
    assert_eq!(remote_wake["stream_id"], "remote-project-target-chat");
    assert_eq!(remote_wake["authority_device_id"], "remote-resource-device");
}
