use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use rua_core::{ChatAddress, ConversationActivityPosition};
use rua_device_network::DeviceSessionLiveActivityHandler;

use rua_server::profiles::ServerProfile;
use rua_server::services::active_stream_broadcast::{
    ActiveStreamBroadcastDeviceSessionHandler, ActiveStreamBroadcastFrame,
    ActiveStreamBroadcastHub, ActiveStreamBroadcastSubscribeRequest, ActiveStreamOwner,
};
use rua_server::services::chat_activity_wake::ChatLiveWake;
use rua_server::services::console_chat::{materialize_targeted_chat, CONSOLE_CHANNEL_ID};
use rua_server::services::conversation_authority_placement::{
    ConversationAuthorityEpoch, ConversationAuthorityPlacement,
};
use rua_server::services::conversation_live_activity::ConversationLiveActivity;
use rua_server::services::device_context::current_sync_device_id;
use rua_server::streaming::{Priority, StreamKey};
use rua_server::AppState;
use tempfile::TempDir;

const CHAT_ID: &str = "00000000-0000-0000-0000-000000009902";
const DRAFT_CHAT_ID: &str = "00000000-0000-0000-0000-000000009903";
const PROJECT_CHAT_ID: &str = "00000000-0000-0000-0000-000000009904";

fn epoch(wall_ms: u64) -> ConversationAuthorityEpoch {
    ConversationAuthorityEpoch::from_parts(wall_ms, 0, [0; 8])
}

async fn seed_session_authority(workspace_dir: &std::path::Path, chat_id: &str, device_id: &str) {
    let now = chrono::Utc::now().to_rfc3339();
    let mut header = rua_session::ChatSessionHeader::new(rua_session::ChatMetadata {
        chat_id: chat_id.to_string(),
        agent_id: "default".to_string(),
        title: "Live".to_string(),
        created_at: now.clone(),
        last_activity_at: now,
        pinned: false,
        archived: false,
        preview: None,
    });
    header.extra.insert(
        "authority_placement".to_string(),
        serde_json::json!({
            "device_id": device_id,
            "authority_epoch": "0000000000000001000000000000000000000000",
            "base_manifest_hash": format!("sha256:{}", "1".repeat(64)),
        }),
    );
    rua_chat::apply_agent_scope_defaults(&mut header, "default");
    rua_session::WorkspaceSessionStore::new(workspace_dir)
        .save_chat_header(&header)
        .await
        .expect("seed Session authority");
}

#[tokio::test(start_paused = true)]
async fn idle_live_viewer_receives_a_complete_sse_keepalive_frame() {
    let tmp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let activity = ConversationLiveActivity::new(
        Arc::clone(&state.streams),
        Arc::clone(&state.registry),
        Arc::clone(&state.sync_device),
        Arc::clone(&state.sync_node),
        Arc::clone(&state.device_network),
        state.rua_root.clone(),
    )
    .with_conversation_authority(Arc::clone(&state.conversation_authority));
    let mut viewer = activity
        .subscribe_viewer(
            &ChatAddress::agent("default", DRAFT_CHAT_ID),
            "default",
            "viewer-keepalive",
            None,
        )
        .await;

    let frame = viewer
        .next()
        .await
        .expect("idle live viewer should remain open")
        .expect("keepalive frame is infallible");

    assert_eq!(frame, ": keepalive\n\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn draft_live_viewer_waits_for_materialization_without_authority_error() {
    let tmp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let activity = ConversationLiveActivity::new(
        Arc::clone(&state.streams),
        Arc::clone(&state.registry),
        Arc::clone(&state.sync_device),
        Arc::clone(&state.sync_node),
        Arc::clone(&state.device_network),
        state.rua_root.clone(),
    )
    .with_conversation_authority(Arc::clone(&state.conversation_authority));
    let mut viewer = activity
        .subscribe_viewer(
            &ChatAddress::agent("default", DRAFT_CHAT_ID),
            "default",
            "viewer-draft",
            None,
        )
        .await;

    assert!(
        tokio::time::timeout(Duration::from_millis(100), viewer.next())
            .await
            .is_err(),
        "an unmaterialized Chat should wait instead of emitting an authority error"
    );

    let local_device_id = current_sync_device_id(&state.sync_device).expect("test Device identity");
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("workspace");
    materialize_targeted_chat(
        &runtime.workspace_dir(),
        "default",
        DRAFT_CHAT_ID,
        "Draft",
        &ConversationAuthorityPlacement {
            device_id: local_device_id,
            authority_epoch: epoch(1),
            base_manifest_hash: format!("sha256:{}", "1".repeat(64)),
        },
    )
    .await
    .expect("materialize draft Chat");

    let key = StreamKey::new(
        CONSOLE_CHANNEL_ID,
        ChatAddress::agent("default", DRAFT_CHAT_ID),
        Priority::Normal,
    );
    let (producer_id, _cancel_rx) = state.streams.register_active_keyed(&key).await;
    let payload = serde_json::json!({
        "type": "text_delta",
        "delta": "started"
    })
    .to_string();
    state.streams.push_event_keyed(&key, payload.clone()).await;

    let received = tokio::time::timeout(Duration::from_secs(1), viewer.next())
        .await
        .expect("draft viewer should attach to the materialized local run")
        .expect("draft viewer should remain open")
        .expect("live payload is infallible");
    let received: serde_json::Value =
        serde_json::from_str(&received).expect("live payload should be valid JSON");
    let delivered = if received["type"] == "stream_replay" {
        received["events"][0].clone()
    } else {
        received
    };
    assert_eq!(
        delivered,
        serde_json::from_str::<serde_json::Value>(&payload).unwrap()
    );
    assert!(state.streams.mark_completed(&key, producer_id).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_draft_live_viewer_waits_on_its_resource_device() {
    let tmp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let local_device_id = current_sync_device_id(&state.sync_device).expect("test Device identity");
    let project_id = rua_core::ProjectId::new("project-live-draft");
    let now = chrono::Utc::now();
    state
        .project_catalog
        .apply_descriptor(rua_project::ProjectDescriptor {
            project_id: project_id.clone(),
            display_name: "Draft Project".to_string(),
            resource_device_id: local_device_id.clone(),
            display_path: "/local/draft".to_string(),
            lifecycle: rua_project::ProjectLifecycle::Active,
            resources: None,
            created_at: now,
            updated_at: now,
            extra: BTreeMap::new(),
        })
        .expect("seed Project descriptor");
    let address = ChatAddress::project(project_id, PROJECT_CHAT_ID);
    let activity = ConversationLiveActivity::new(
        Arc::clone(&state.streams),
        Arc::clone(&state.registry),
        Arc::clone(&state.sync_device),
        Arc::clone(&state.sync_node),
        Arc::clone(&state.device_network),
        state.rua_root.clone(),
    )
    .with_conversation_authority(Arc::clone(&state.conversation_authority))
    .with_project_catalog(Arc::clone(&state.project_catalog))
    .with_chat_activity_wake_hub(Arc::clone(&state.chat_activity_wake_hub))
    .with_active_stream_broadcast_hub(Arc::clone(&state.active_stream_broadcast_hub));
    let mut viewer = activity
        .subscribe_viewer(&address, "default", "viewer-project-draft", None)
        .await;

    assert!(
        tokio::time::timeout(Duration::from_millis(100), viewer.next())
            .await
            .is_err(),
        "an unmaterialized Project Chat should wait instead of emitting not-found"
    );

    drop(viewer);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unavailable_project_live_does_not_fallback_or_persist_retry_intent() {
    let tmp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let project_id = rua_core::ProjectId::new("project-live-unavailable");
    let now = chrono::Utc::now();
    state
        .project_catalog
        .apply_descriptor(rua_project::ProjectDescriptor {
            project_id: project_id.clone(),
            display_name: "Unavailable Project".to_string(),
            resource_device_id: "0102030405060708".to_string(),
            display_path: "/remote/unavailable".to_string(),
            lifecycle: rua_project::ProjectLifecycle::Active,
            resources: None,
            created_at: now,
            updated_at: now,
            extra: BTreeMap::new(),
        })
        .expect("seed Project descriptor");
    let address = ChatAddress::project(project_id, PROJECT_CHAT_ID);
    let scope = rua_core::ChatScopeStore::resolve(&state.rua_root, address.clone());
    let activity = ConversationLiveActivity::new(
        Arc::clone(&state.streams),
        Arc::clone(&state.registry),
        Arc::clone(&state.sync_device),
        Arc::clone(&state.sync_node),
        Arc::clone(&state.device_network),
        state.rua_root.clone(),
    )
    .with_conversation_authority(Arc::clone(&state.conversation_authority))
    .with_project_catalog(Arc::clone(&state.project_catalog));
    let mut viewer = activity
        .subscribe_viewer(&address, "default", "viewer-project-unavailable", None)
        .await;

    let payload = tokio::time::timeout(Duration::from_secs(1), viewer.next())
        .await
        .expect("unavailable Project Live should fail explicitly")
        .expect("Project viewer remains open")
        .expect("Project Live payload is infallible");
    let payload: serde_json::Value =
        serde_json::from_str(&payload).expect("Project Live error JSON");
    assert_eq!(payload["type"], "error");
    assert!(payload["message"]
        .as_str()
        .is_some_and(|message| message.contains("unavailable")));
    assert!(
        rua_server::services::chat_index::get(scope.root(), PROJECT_CHAT_ID)
            .await
            .unwrap()
            .is_none()
    );
    assert!(rua_session::WorkspaceSessionStore::new(scope.root())
        .load_chat_header(PROJECT_CHAT_ID)
        .await
        .unwrap()
        .is_none());
    assert!(state
        .conversation_authority
        .active_turn(&address)
        .await
        .is_none());
    assert!(!scope.root().join("device_commands").exists());

    // Reproduce foreground recovery after the view first attached while the
    // runtime was absent. The same viewer must observe the new generation,
    // without navigation, another peer's dial, or a new document subscription.
    assert!(
        tokio::time::timeout(Duration::from_millis(30), viewer.next())
            .await
            .is_err()
    );
    state.device_network_availability.mark_starting();
    tokio::time::sleep(Duration::from_millis(30)).await;
    state
        .device_network_availability
        .mark_failed("rebind regression sentinel");
    let recovered = tokio::time::timeout(Duration::from_secs(1), viewer.next())
        .await
        .expect("same viewer must reattach on resident readiness changes")
        .expect("viewer remains open")
        .unwrap();
    assert!(
        recovered.contains("rebind regression sentinel"),
        "{recovered}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idle_live_viewer_receives_next_local_run() {
    let tmp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let local_device_id = current_sync_device_id(&state.sync_device).expect("test Device identity");
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("workspace");
    let now = chrono::Utc::now();
    rua_server::services::chat_index::upsert(
        &runtime.workspace_dir(),
        rua_server::services::chat_index::ChatIndexEntry {
            id: CHAT_ID.to_string(),
            name: "Live".to_string(),
            user_id: "default".to_string(),
            channel: "console".to_string(),
            created_at: now,
            updated_at: now,
            meta: serde_json::json!({
                "authority_placement": {
                    "device_id": local_device_id,
                    "authority_epoch": "0000000000000001000000000000000000000000",
                    "base_manifest_hash": format!("sha256:{}", "1".repeat(64)),
                }
            }),
            status: "idle".to_string(),
            pinned: false,
            pin_order: 0,
            tag: rua_session::ChatTag::None,
            archived: false,
        },
    )
    .await
    .expect("seed placement");
    seed_session_authority(&runtime.workspace_dir(), CHAT_ID, &local_device_id).await;
    let activity = ConversationLiveActivity::new(
        Arc::clone(&state.streams),
        Arc::clone(&state.registry),
        Arc::clone(&state.sync_device),
        Arc::clone(&state.sync_node),
        Arc::clone(&state.device_network),
        state.rua_root.clone(),
    )
    .with_conversation_authority(Arc::clone(&state.conversation_authority))
    .with_active_stream_broadcast_hub(Arc::clone(&state.active_stream_broadcast_hub));
    let mut viewer = activity
        .subscribe_viewer(
            &ChatAddress::agent("default", CHAT_ID),
            "default",
            "viewer-a",
            None,
        )
        .await;
    let initial_control = tokio::time::timeout(Duration::from_secs(1), viewer.next())
        .await
        .expect("viewer should receive initial control")
        .expect("live viewer should remain open")
        .expect("live payload is infallible");
    let initial_control: serde_json::Value =
        serde_json::from_str(&initial_control).expect("initial control JSON");
    assert_eq!(initial_control["type"], "conversation_control");
    assert_eq!(initial_control["reason"], "viewer_attached");
    assert_eq!(
        initial_control["control"]["placement"]["authority_device_id"],
        local_device_id
    );
    assert_eq!(
        initial_control["control"]["placement"]["authority_epoch"],
        "0000000000000001000000000000000000000000"
    );
    assert_eq!(initial_control["activity_snapshot"], true);
    assert!(initial_control["activity_generation"].is_string());

    let payload = serde_json::json!({
        "type": "text_delta",
        "delta": "hello"
    })
    .to_string();
    state
        .active_stream_broadcast_hub
        .publish_conversation_event(
            "turn-stream",
            &ChatAddress::agent("default", CHAT_ID),
            epoch(1),
            payload,
        )
        .await;

    let received = tokio::time::timeout(Duration::from_secs(1), viewer.next())
        .await
        .expect("idle viewer should attach to the next local run")
        .expect("live viewer should remain open")
        .expect("live payload is infallible");
    let received: serde_json::Value =
        serde_json::from_str(&received).expect("live payload should be valid JSON");
    let delivered_event = if received["type"] == "stream_replay" {
        assert_eq!(received["complete"], true);
        let events = received["events"]
            .as_array()
            .expect("replay snapshot should contain events");
        assert_eq!(events.len(), 1);
        events[0].clone()
    } else {
        received
    };
    assert_eq!(delivered_event["type"], "text_delta");
    assert_eq!(delivered_event["delta"], "hello");
    assert_eq!(delivered_event["activity_sequence"], 1);
    assert_eq!(
        delivered_event["activity_generation"],
        initial_control["activity_generation"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_broadcast_replay_is_one_ordered_transaction() {
    let tmp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let local_device_id = current_sync_device_id(&state.sync_device).expect("test Device identity");
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("workspace");
    let address = ChatAddress::agent("default", CHAT_ID);
    let now = chrono::Utc::now();
    rua_server::services::chat_index::upsert(
        &runtime.workspace_dir(),
        rua_server::services::chat_index::ChatIndexEntry {
            id: CHAT_ID.to_string(),
            name: "Live".to_string(),
            user_id: "default".to_string(),
            channel: "console".to_string(),
            created_at: now,
            updated_at: now,
            meta: serde_json::json!({
                "authority_placement": {
                    "device_id": local_device_id,
                    "authority_epoch": "0000000000000001000000000000000000000000",
                    "base_manifest_hash": format!("sha256:{}", "1".repeat(64)),
                }
            }),
            status: "running".to_string(),
            pinned: false,
            pin_order: 0,
            tag: rua_session::ChatTag::None,
            archived: false,
        },
    )
    .await
    .expect("seed placement");
    seed_session_authority(&runtime.workspace_dir(), CHAT_ID, &local_device_id).await;

    state
        .active_stream_broadcast_hub
        .publish_conversation_event(
            "control-stream",
            &address,
            epoch(1),
            serde_json::json!({
                "type": "conversation_control",
                "reason": "active_turn_started"
            })
            .to_string(),
        )
        .await;
    state
        .active_stream_broadcast_hub
        .publish_conversation_event(
            "turn-stream",
            &address,
            epoch(1),
            serde_json::json!({"type": "text_delta", "delta": "completed output"}).to_string(),
        )
        .await;
    state
        .active_stream_broadcast_hub
        .finish_conversation("turn-stream", &address, epoch(1))
        .await;
    state
        .active_stream_broadcast_hub
        .publish_conversation_event(
            "control-stream",
            &address,
            epoch(1),
            serde_json::json!({
                "type": "conversation_control",
                "reason": "turn_completed"
            })
            .to_string(),
        )
        .await;

    let activity = ConversationLiveActivity::new(
        Arc::clone(&state.streams),
        Arc::clone(&state.registry),
        Arc::clone(&state.sync_device),
        Arc::clone(&state.sync_node),
        Arc::clone(&state.device_network),
        state.rua_root.clone(),
    )
    .with_conversation_authority(Arc::clone(&state.conversation_authority))
    .with_active_stream_broadcast_hub(Arc::clone(&state.active_stream_broadcast_hub));
    let mut viewer = activity
        .subscribe_viewer(&address, "default", "viewer-local-replay", None)
        .await;

    let initial_control = tokio::time::timeout(Duration::from_secs(1), viewer.next())
        .await
        .expect("viewer should receive initial control")
        .expect("live viewer should remain open")
        .expect("live payload is infallible");
    let initial_control: serde_json::Value =
        serde_json::from_str(&initial_control).expect("initial control JSON");
    assert_eq!(initial_control["type"], "conversation_control");
    assert_eq!(initial_control["reason"], "viewer_attached");

    let mut local_replay_chunks = Vec::new();
    loop {
        let replay = tokio::time::timeout(Duration::from_secs(1), viewer.next())
            .await
            .expect("viewer should receive Replay")
            .expect("live viewer should remain open")
            .expect("live payload is infallible");
        let replay: serde_json::Value =
            serde_json::from_str(&replay).expect("Replay should be valid JSON");
        assert_eq!(replay["type"], "stream_replay");
        let complete = replay["complete"].as_bool().expect("Replay completion");
        local_replay_chunks.push((
            replay["events"]
                .as_array()
                .expect("Replay events")
                .iter()
                .map(|event| event["activity_sequence"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            complete,
        ));
        if complete {
            break;
        }
    }

    let handler = ActiveStreamBroadcastDeviceSessionHandler::new(Arc::clone(
        &state.active_stream_broadcast_hub,
    ));
    let request = ActiveStreamBroadcastSubscribeRequest {
        device_delivery_id: String::new(),
        owner: Some(ActiveStreamOwner::chat(address.clone())),
        after_seq_by_stream: BTreeMap::new(),
        activity_generation: None,
        conversation_activity_position: None,
        collapse_replay: true,
        initial_conversation_control: false,
        authority_epoch: None,
        viewer: None,
    };
    let mut remote_viewer = handler
        .subscribe_live_activity(&serde_json::to_vec(&request).expect("remote request"))
        .await
        .expect("remote viewer should attach");
    let mut remote_replay_chunks = Vec::new();
    loop {
        let bytes = tokio::time::timeout(Duration::from_secs(1), remote_viewer.next())
            .await
            .expect("remote viewer should receive Replay")
            .expect("remote viewer should remain open");
        let frame: ActiveStreamBroadcastFrame =
            serde_json::from_slice(&bytes).expect("remote Replay frame");
        let ActiveStreamBroadcastFrame::Event { payload, .. } = frame else {
            panic!("remote Replay should be collapsed into Event frames");
        };
        let replay: serde_json::Value =
            serde_json::from_str(&payload).expect("remote Replay should be valid JSON");
        assert_eq!(replay["type"], "stream_replay");
        let complete = replay["complete"].as_bool().expect("Replay completion");
        remote_replay_chunks.push((
            replay["events"]
                .as_array()
                .expect("Replay events")
                .iter()
                .map(|event| event["activity_sequence"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            complete,
        ));
        if complete {
            break;
        }
    }

    assert_eq!(
        local_replay_chunks
            .iter()
            .flat_map(|(sequences, _)| sequences.iter().copied())
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(
        local_replay_chunks, remote_replay_chunks,
        "local and Device Session viewers must share Replay chunking"
    );
    assert_eq!(
        local_replay_chunks
            .iter()
            .filter(|(_, complete)| *complete)
            .count(),
        1,
        "one Replay attachment has one completion boundary"
    );

    state
        .active_stream_broadcast_hub
        .publish_conversation_event(
            "turn-stream-next",
            &address,
            epoch(1),
            serde_json::json!({"type": "text_delta", "delta": "live output"}).to_string(),
        )
        .await;
    let live = tokio::time::timeout(Duration::from_secs(1), viewer.next())
        .await
        .expect("viewer should continue with current Live events")
        .expect("live viewer should remain open")
        .expect("live payload is infallible");
    let live: serde_json::Value =
        serde_json::from_str(&live).expect("Live event should be valid JSON");
    assert_eq!(live["type"], "text_delta");
    assert_eq!(live["delta"], "live output");
    assert_eq!(live["activity_sequence"], 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn late_local_viewer_receives_replay_as_snapshot_before_live_events() {
    let tmp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let local_device_id = current_sync_device_id(&state.sync_device).expect("test Device identity");
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("workspace");
    let now = chrono::Utc::now();
    rua_server::services::chat_index::upsert(
        &runtime.workspace_dir(),
        rua_server::services::chat_index::ChatIndexEntry {
            id: CHAT_ID.to_string(),
            name: "Live".to_string(),
            user_id: "default".to_string(),
            channel: "console".to_string(),
            created_at: now,
            updated_at: now,
            meta: serde_json::json!({
                "authority_placement": {
                    "device_id": local_device_id,
                    "authority_epoch": "0000000000000001000000000000000000000000",
                    "base_manifest_hash": format!("sha256:{}", "1".repeat(64)),
                }
            }),
            status: "running".to_string(),
            pinned: false,
            pin_order: 0,
            tag: rua_session::ChatTag::None,
            archived: false,
        },
    )
    .await
    .expect("seed placement");
    seed_session_authority(&runtime.workspace_dir(), CHAT_ID, &local_device_id).await;

    let key = StreamKey::new(
        CONSOLE_CHANNEL_ID,
        ChatAddress::agent("default", CHAT_ID),
        Priority::Normal,
    );
    let (producer_id, _cancel_rx) = state.streams.register_active_keyed(&key).await;
    let user = serde_json::json!({
        "type": "user_message",
        "message": {
            "id": "user-1",
            "role": "user",
            "content": [{"type": "text", "text": "hello"}]
        }
    })
    .to_string();
    let first_delta = serde_json::json!({
        "type": "text_delta",
        "delta": "already generated"
    })
    .to_string();
    state.streams.push_event_keyed(&key, user.clone()).await;
    state
        .streams
        .push_event_keyed(&key, first_delta.clone())
        .await;

    let activity = ConversationLiveActivity::new(
        Arc::clone(&state.streams),
        Arc::clone(&state.registry),
        Arc::clone(&state.sync_device),
        Arc::clone(&state.sync_node),
        Arc::clone(&state.device_network),
        state.rua_root.clone(),
    )
    .with_conversation_authority(Arc::clone(&state.conversation_authority));
    let mut viewer = activity
        .subscribe_viewer(
            &ChatAddress::agent("default", CHAT_ID),
            "default",
            "viewer-a",
            None,
        )
        .await;
    let initial_control = tokio::time::timeout(Duration::from_secs(1), viewer.next())
        .await
        .expect("viewer should receive initial control")
        .expect("live viewer should remain open")
        .expect("live payload is infallible");
    let initial_control: serde_json::Value =
        serde_json::from_str(&initial_control).expect("initial control JSON");
    assert_eq!(initial_control["type"], "conversation_control");
    assert_eq!(
        initial_control["control"]["placement"]["authority_device_id"],
        local_device_id
    );
    assert_eq!(
        initial_control["control"]["placement"]["authority_epoch"],
        "0000000000000001000000000000000000000000"
    );

    let snapshot = tokio::time::timeout(Duration::from_secs(1), viewer.next())
        .await
        .expect("late viewer should receive a replay snapshot")
        .expect("live viewer should remain open")
        .expect("live payload is infallible");
    let snapshot: serde_json::Value =
        serde_json::from_str(&snapshot).expect("snapshot should be valid JSON");
    assert_eq!(snapshot["type"], "stream_replay");
    assert_eq!(snapshot["complete"], true);
    assert_eq!(
        snapshot["events"],
        serde_json::json!([
            serde_json::from_str::<serde_json::Value>(&user).unwrap(),
            serde_json::from_str::<serde_json::Value>(&first_delta).unwrap(),
        ])
    );

    let live_delta = serde_json::json!({
        "type": "text_delta",
        "delta": " and still streaming"
    })
    .to_string();
    state
        .streams
        .push_event_keyed(&key, live_delta.clone())
        .await;
    let received = tokio::time::timeout(Duration::from_secs(1), viewer.next())
        .await
        .expect("late viewer should continue with live events")
        .expect("live viewer should remain open")
        .expect("live payload is infallible");
    assert_eq!(received, live_delta);

    assert!(state.streams.mark_completed(&key, producer_id).await);
}

#[tokio::test]
async fn conversation_broadcast_frames_carry_authority_epoch() {
    let hub = ActiveStreamBroadcastHub::new();
    let address = ChatAddress::agent("default", CHAT_ID);
    let mut rx = hub
        .subscribe_chat(&address, &BTreeMap::new())
        .await
        .rx
        .expect("persistent Chat subscription");

    hub.publish_conversation_event(
        "stream-1",
        &address,
        epoch(4),
        serde_json::json!({"type": "text_delta", "delta": "hello"}).to_string(),
    )
    .await;

    let started = rx.recv().await.expect("started frame");
    let event = rx.recv().await.expect("event frame");
    assert!(matches!(
        started,
        ActiveStreamBroadcastFrame::Started {
            authority_epoch: Some(value),
            ..
        } if value == epoch(4)
    ));
    assert!(matches!(
        event,
        ActiveStreamBroadcastFrame::Event {
            authority_epoch: Some(value),
            ..
        } if value == epoch(4)
    ));
}

#[tokio::test]
async fn conversation_broadcast_assigns_one_activity_sequence_across_streams() {
    let hub = ActiveStreamBroadcastHub::new();
    let address = ChatAddress::agent("default", CHAT_ID);
    let mut rx = hub
        .subscribe_chat(&address, &BTreeMap::new())
        .await
        .rx
        .expect("persistent Chat subscription");

    hub.publish_conversation_event(
        "turn-stream",
        &address,
        epoch(4),
        serde_json::json!({"type": "text_delta", "delta": "answer"}).to_string(),
    )
    .await;
    hub.publish_conversation_event(
        "control-stream",
        &address,
        epoch(4),
        serde_json::json!({
            "type": "conversation_control",
            "reason": "turn_completed"
        })
        .to_string(),
    )
    .await;

    let mut activities = Vec::new();
    while activities.len() < 2 {
        let frame = rx.recv().await.expect("activity frame");
        if let ActiveStreamBroadcastFrame::Event { payload, .. } = frame {
            let value: serde_json::Value =
                serde_json::from_str(&payload).expect("activity payload");
            activities.push((
                value["activity_generation"]
                    .as_str()
                    .expect("activity generation")
                    .to_string(),
                value["activity_sequence"]
                    .as_u64()
                    .expect("activity sequence"),
                value["type"].as_str().expect("activity type").to_string(),
            ));
        }
    }

    assert_eq!(activities[0].0, activities[1].0);
    assert_eq!(
        activities
            .iter()
            .map(|(_, sequence, kind)| (*sequence, kind.as_str()))
            .collect::<Vec<_>>(),
        vec![(1, "text_delta"), (2, "conversation_control")]
    );
}

#[tokio::test]
async fn chat_subscription_replays_different_streams_in_activity_order() {
    let hub = ActiveStreamBroadcastHub::new();
    let address = ChatAddress::agent("default", CHAT_ID);

    hub.publish_conversation_event(
        "turn-stream",
        &address,
        epoch(4),
        serde_json::json!({"type": "text_delta", "delta": "first"}).to_string(),
    )
    .await;
    hub.publish_conversation_event(
        "control-stream",
        &address,
        epoch(4),
        serde_json::json!({"type": "conversation_control", "revision": 1}).to_string(),
    )
    .await;
    hub.publish_conversation_event(
        "turn-stream",
        &address,
        epoch(4),
        serde_json::json!({"type": "turn_terminal", "turn_id": "turn-1"}).to_string(),
    )
    .await;

    let replay = hub.subscribe_chat(&address, &BTreeMap::new()).await.replay;
    let sequences = replay
        .into_iter()
        .filter_map(|frame| match frame {
            ActiveStreamBroadcastFrame::Event { payload, .. } => {
                let value: serde_json::Value = serde_json::from_str(&payload).ok()?;
                value["activity_sequence"].as_u64()
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(sequences, vec![1, 2, 3]);
}

#[tokio::test]
async fn positioned_chat_subscription_replays_only_expected_activity_sequences() {
    let hub = ActiveStreamBroadcastHub::new();
    let address = ChatAddress::agent("default", CHAT_ID);

    for sequence in 1..=4 {
        hub.publish_conversation_event(
            "turn-stream",
            &address,
            epoch(4),
            serde_json::json!({"type": "text_delta", "delta": sequence.to_string()}).to_string(),
        )
        .await;
    }

    let position = ConversationActivityPosition::new(hub.activity_generation(), 3);
    let replay = hub
        .subscribe_chat_from_activity_position(&address, &position)
        .await
        .replay;
    let sequences = replay
        .into_iter()
        .filter_map(|frame| match frame {
            ActiveStreamBroadcastFrame::Event { payload, .. } => {
                serde_json::from_str::<serde_json::Value>(&payload).ok()?["activity_sequence"]
                    .as_u64()
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(sequences, vec![3, 4]);
}

#[tokio::test]
async fn positioned_chat_subscription_resets_a_stale_generation() {
    let hub = ActiveStreamBroadcastHub::new();
    let address = ChatAddress::agent("default", CHAT_ID);
    hub.publish_conversation_event(
        "turn-stream",
        &address,
        epoch(4),
        serde_json::json!({"type": "text_delta", "delta": "current"}).to_string(),
    )
    .await;

    let stale = ConversationActivityPosition::new("stale-generation", 999);
    let replay = hub
        .subscribe_chat_from_activity_position(&address, &stale)
        .await
        .replay;
    let sequences = replay
        .into_iter()
        .filter_map(|frame| match frame {
            ActiveStreamBroadcastFrame::Event { payload, .. } => {
                serde_json::from_str::<serde_json::Value>(&payload).ok()?["activity_sequence"]
                    .as_u64()
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(sequences, vec![1]);
}

#[tokio::test]
async fn device_session_chat_subscription_honors_the_activity_position() {
    let hub = Arc::new(ActiveStreamBroadcastHub::new());
    let address = ChatAddress::agent("default", CHAT_ID);
    for sequence in 1..=4 {
        hub.publish_conversation_event(
            "turn-stream",
            &address,
            epoch(4),
            serde_json::json!({"type": "text_delta", "delta": sequence.to_string()}).to_string(),
        )
        .await;
    }
    let handler = ActiveStreamBroadcastDeviceSessionHandler::new(Arc::clone(&hub));
    let request = ActiveStreamBroadcastSubscribeRequest {
        device_delivery_id: String::new(),
        owner: Some(ActiveStreamOwner::chat(address.clone())),
        after_seq_by_stream: BTreeMap::new(),
        activity_generation: None,
        conversation_activity_position: Some(ConversationActivityPosition::new(
            hub.activity_generation(),
            3,
        )),
        collapse_replay: false,
        initial_conversation_control: false,
        authority_epoch: None,
        viewer: None,
    };
    let mut stream = handler
        .subscribe_live_activity(&serde_json::to_vec(&request).expect("request"))
        .await
        .expect("positioned Device Session subscription");
    let mut sequences = Vec::new();
    while sequences.len() < 2 {
        let bytes = tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .expect("positioned replay frame")
            .expect("positioned replay remains open");
        let frame: ActiveStreamBroadcastFrame =
            serde_json::from_slice(&bytes).expect("decode positioned replay frame");
        if let ActiveStreamBroadcastFrame::Event { payload, .. } = frame {
            sequences.push(
                serde_json::from_str::<serde_json::Value>(&payload).expect("event JSON")
                    ["activity_sequence"]
                    .as_u64()
                    .expect("activity sequence"),
            );
        }
    }

    assert_eq!(sequences, vec![3, 4]);

    let request = ActiveStreamBroadcastSubscribeRequest {
        device_delivery_id: String::new(),
        owner: Some(ActiveStreamOwner::chat(address)),
        after_seq_by_stream: BTreeMap::from([("turn-stream".to_string(), 3)]),
        activity_generation: Some(hub.activity_generation().to_string()),
        conversation_activity_position: Some(ConversationActivityPosition::new(
            hub.activity_generation(),
            2,
        )),
        collapse_replay: false,
        initial_conversation_control: false,
        authority_epoch: None,
        viewer: None,
    };
    let mut stream = handler
        .subscribe_live_activity(&serde_json::to_vec(&request).expect("request"))
        .await
        .expect("transport-cursor Device Session subscription");
    let mut sequences = Vec::new();
    while sequences.is_empty() {
        let bytes = tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .expect("transport-cursor replay frame")
            .expect("transport-cursor replay remains open");
        let frame: ActiveStreamBroadcastFrame =
            serde_json::from_slice(&bytes).expect("decode transport-cursor replay frame");
        if let ActiveStreamBroadcastFrame::Event { payload, .. } = frame {
            sequences.push(
                serde_json::from_str::<serde_json::Value>(&payload).expect("event JSON")
                    ["activity_sequence"]
                    .as_u64()
                    .expect("activity sequence"),
            );
        }
    }

    assert_eq!(sequences, vec![4]);
}

#[tokio::test]
async fn completed_turn_stream_does_not_leave_a_structural_replay_gap() {
    let hub = ActiveStreamBroadcastHub::new();
    let address = ChatAddress::agent("default", CHAT_ID);

    hub.publish_conversation_event(
        "control-stream",
        &address,
        epoch(4),
        serde_json::json!({
            "type": "conversation_control",
            "reason": "active_turn_started"
        })
        .to_string(),
    )
    .await;
    hub.publish_conversation_event(
        "turn-stream",
        &address,
        epoch(4),
        serde_json::json!({"type": "text_delta", "delta": "completed output"}).to_string(),
    )
    .await;
    hub.finish_conversation("turn-stream", &address, epoch(4))
        .await;
    hub.publish_conversation_event(
        "control-stream",
        &address,
        epoch(4),
        serde_json::json!({
            "type": "conversation_control",
            "reason": "turn_completed"
        })
        .to_string(),
    )
    .await;

    let replay = hub.subscribe_chat(&address, &BTreeMap::new()).await.replay;
    let sequences = replay
        .into_iter()
        .filter_map(|frame| match frame {
            ActiveStreamBroadcastFrame::Event { payload, .. } => {
                let value: serde_json::Value = serde_json::from_str(&payload).ok()?;
                value["activity_sequence"].as_u64()
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(
        sequences,
        vec![1, 2, 3],
        "completed producer streams must remain in the ordered per-Chat Replay interval"
    );
}

#[tokio::test]
async fn bounded_conversation_replay_emits_one_explicit_prefix_gap() {
    let hub = ActiveStreamBroadcastHub::new();
    let address = ChatAddress::agent("default", CHAT_ID);

    for sequence in 1..=5_000 {
        hub.publish_conversation_event(
            "turn-stream",
            &address,
            epoch(4),
            serde_json::json!({
                "type": "text_delta",
                "delta": sequence.to_string()
            })
            .to_string(),
        )
        .await;
    }

    let replay = hub.subscribe_chat(&address, &BTreeMap::new()).await.replay;
    let gaps = replay
        .iter()
        .filter_map(|frame| match frame {
            ActiveStreamBroadcastFrame::Gap {
                requested_after_seq,
                first_available_seq,
                ..
            } => Some((*requested_after_seq, *first_available_seq)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let retained_sequences = replay
        .iter()
        .filter_map(|frame| match frame {
            ActiveStreamBroadcastFrame::Event { payload, .. } => {
                let value: serde_json::Value = serde_json::from_str(payload).ok()?;
                value["activity_sequence"].as_u64()
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let checkpoint_flags = replay
        .iter()
        .filter_map(|frame| match frame {
            ActiveStreamBroadcastFrame::Event { payload, .. } => {
                let value: serde_json::Value = serde_json::from_str(payload).ok()?;
                Some(value["activity_checkpoint"].as_bool())
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].0, 0);
    assert!(gaps[0].1 > 1);
    assert_eq!(retained_sequences.first().copied(), Some(1));
    assert!(
        retained_sequences
            .windows(2)
            .all(|pair| pair[1] == pair[0] + 1),
        "bounded retention may remove only one contiguous prefix"
    );
    assert_eq!(retained_sequences.last().copied(), Some(5_000));
    assert!(
        checkpoint_flags.iter().all(|flag| *flag == Some(true)),
        "gap recovery must distinguish the runtime-only checkpoint projection"
    );

    let position = ConversationActivityPosition::new(hub.activity_generation(), 1);
    let positioned = hub
        .subscribe_chat_from_activity_position(&address, &position)
        .await
        .replay;
    let positioned_gaps = positioned
        .iter()
        .filter(|frame| matches!(frame, ActiveStreamBroadcastFrame::Gap { .. }))
        .count();
    assert_eq!(positioned_gaps, 1);
}

#[tokio::test]
async fn chat_subscription_replays_each_stream_from_its_own_cursor() {
    let hub = ActiveStreamBroadcastHub::new();
    let address = ChatAddress::agent("default", CHAT_ID);
    for index in 1..=6 {
        hub.publish_conversation_event(
            "stream-high",
            &address,
            epoch(4),
            serde_json::json!({"type": "text_delta", "delta": format!("high-{index}")}).to_string(),
        )
        .await;
    }
    for index in 1..=2 {
        hub.publish_conversation_event(
            "stream-low",
            &address,
            epoch(4),
            serde_json::json!({"type": "conversation_control", "revision": index}).to_string(),
        )
        .await;
    }

    let cursors = BTreeMap::from([
        ("stream-high".to_string(), 5),
        ("stream-low".to_string(), 1),
    ]);
    let subscription = hub.subscribe_chat(&address, &cursors).await;
    let replayed = subscription
        .replay
        .into_iter()
        .filter_map(|frame| match frame {
            ActiveStreamBroadcastFrame::Event {
                device_delivery_id,
                seq,
                ..
            } => Some((device_delivery_id, seq)),
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>();

    assert_eq!(
        replayed,
        std::collections::BTreeSet::from([
            ("stream-high".to_string(), 6),
            ("stream-low".to_string(), 2),
        ])
    );
}

#[tokio::test]
async fn device_session_cursor_keeps_unseen_completed_stream_activity() {
    let hub = Arc::new(ActiveStreamBroadcastHub::new());
    let address = ChatAddress::agent("default", CHAT_ID);

    hub.publish_conversation_event(
        "control-stream",
        &address,
        epoch(4),
        serde_json::json!({
            "type": "conversation_control",
            "reason": "active_turn_started"
        })
        .to_string(),
    )
    .await;
    hub.publish_conversation_event(
        "turn-stream",
        &address,
        epoch(4),
        serde_json::json!({"type": "text_delta", "delta": "unseen output"}).to_string(),
    )
    .await;
    hub.finish_conversation("turn-stream", &address, epoch(4))
        .await;
    hub.publish_conversation_event(
        "control-stream",
        &address,
        epoch(4),
        serde_json::json!({
            "type": "conversation_control",
            "reason": "turn_completed"
        })
        .to_string(),
    )
    .await;

    let cursors = BTreeMap::from([("control-stream".to_string(), 1)]);
    let handler = ActiveStreamBroadcastDeviceSessionHandler::new(Arc::clone(&hub));
    let request = ActiveStreamBroadcastSubscribeRequest {
        device_delivery_id: String::new(),
        owner: Some(ActiveStreamOwner::chat(address)),
        after_seq_by_stream: cursors,
        activity_generation: Some(hub.activity_generation().to_string()),
        conversation_activity_position: None,
        collapse_replay: false,
        initial_conversation_control: false,
        authority_epoch: None,
        viewer: None,
    };
    let mut stream = handler
        .subscribe_live_activity(&serde_json::to_vec(&request).expect("request"))
        .await
        .expect("Device Session viewer should attach");
    let mut replayed = Vec::new();
    while replayed.len() < 2 {
        let bytes = tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .expect("Device Session should replay all unseen activity")
            .expect("Device Session should remain open");
        let frame: ActiveStreamBroadcastFrame =
            serde_json::from_slice(&bytes).expect("activity frame");
        if let ActiveStreamBroadcastFrame::Event {
            device_delivery_id,
            seq,
            payload,
            ..
        } = frame
        {
            let value: serde_json::Value =
                serde_json::from_str(&payload).expect("activity payload");
            replayed.push((
                device_delivery_id,
                seq,
                value["activity_sequence"]
                    .as_u64()
                    .expect("activity sequence"),
            ));
        }
    }

    assert_eq!(
        replayed,
        vec![
            ("turn-stream".to_string(), 1, 2),
            ("control-stream".to_string(), 2, 3),
        ]
    );
}

#[tokio::test]
async fn agent_and_project_same_chat_id_live_activity_are_isolated() {
    let hub = ActiveStreamBroadcastHub::new();
    let agent_address = ChatAddress::agent("same-owner-id", CHAT_ID);
    let project_address = ChatAddress::project("same-owner-id", CHAT_ID);
    let mut agent_rx = hub
        .subscribe_chat(&agent_address, &BTreeMap::new())
        .await
        .rx
        .expect("Agent Chat subscription");
    let mut project_rx = hub
        .subscribe_chat(&project_address, &BTreeMap::new())
        .await
        .rx
        .expect("Project Chat subscription");

    hub.publish_conversation_event(
        "agent-stream",
        &agent_address,
        epoch(4),
        serde_json::json!({"type": "text_delta", "delta": "agent"}).to_string(),
    )
    .await;
    assert!(matches!(
        agent_rx.recv().await,
        Some(ActiveStreamBroadcastFrame::Started { ref owner, .. })
            if owner == &ActiveStreamOwner::chat(agent_address.clone())
    ));
    assert!(matches!(
        agent_rx.recv().await,
        Some(ActiveStreamBroadcastFrame::Event { ref owner, .. })
            if owner == &ActiveStreamOwner::chat(agent_address.clone())
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), project_rx.recv())
            .await
            .is_err(),
        "Agent activity must not reach the equal-UUID Project Chat"
    );

    hub.publish_conversation_event(
        "project-stream",
        &project_address,
        epoch(4),
        serde_json::json!({"type": "text_delta", "delta": "project"}).to_string(),
    )
    .await;
    assert!(matches!(
        project_rx.recv().await,
        Some(ActiveStreamBroadcastFrame::Started { ref owner, .. })
            if owner == &ActiveStreamOwner::chat(project_address.clone())
    ));
    assert!(matches!(
        project_rx.recv().await,
        Some(ActiveStreamBroadcastFrame::Event { ref owner, .. })
            if owner == &ActiveStreamOwner::chat(project_address.clone())
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), agent_rx.recv())
            .await
            .is_err(),
        "Project activity must not reach the equal-UUID Agent Chat"
    );
}

#[tokio::test]
async fn active_stream_device_protocol_requires_typed_chat_address() {
    let handler =
        ActiveStreamBroadcastDeviceSessionHandler::new(Arc::new(ActiveStreamBroadcastHub::new()));
    let legacy = serde_json::json!({
        "device_delivery_id": "",
        "agent_id": "default",
        "chat_id": CHAT_ID,
        "after_seq_by_stream": {},
        "collapse_replay": false,
        "initial_conversation_control": false,
        "authority_epoch": null,
        "viewer": null
    });
    let mut stream = handler
        .subscribe_live_activity(&serde_json::to_vec(&legacy).expect("legacy request"))
        .await
        .expect("legacy request returns a terminal protocol frame");
    let bytes = stream.next().await.expect("legacy request error frame");
    let frame: ActiveStreamBroadcastFrame =
        serde_json::from_slice(&bytes).expect("decode legacy request error");
    assert!(matches!(
        frame,
        ActiveStreamBroadcastFrame::Error {
            owner: None,
            ref detail,
            ..
        } if detail.contains("stream owner")
    ));

    let request = ActiveStreamBroadcastSubscribeRequest {
        device_delivery_id: String::new(),
        owner: Some(ActiveStreamOwner::chat(ChatAddress::project(
            "project-a",
            CHAT_ID,
        ))),
        after_seq_by_stream: BTreeMap::new(),
        activity_generation: None,
        conversation_activity_position: None,
        collapse_replay: false,
        initial_conversation_control: false,
        authority_epoch: None,
        viewer: None,
    };
    let encoded = serde_json::to_value(request).expect("typed request");
    assert!(encoded.get("owner").is_some());
    assert!(encoded.get("agent_id").is_none());
    assert!(encoded.get("chat_id").is_none());
}

#[tokio::test]
async fn activity_generation_change_resets_remote_replay_cursors() {
    let hub = Arc::new(ActiveStreamBroadcastHub::new());
    let address = ChatAddress::agent("default", CHAT_ID);
    hub.publish_conversation_event(
        "turn-stream",
        &address,
        epoch(4),
        serde_json::json!({"type": "text_delta", "delta": "replayed"}).to_string(),
    )
    .await;
    let handler = ActiveStreamBroadcastDeviceSessionHandler::new(Arc::clone(&hub));
    let request = ActiveStreamBroadcastSubscribeRequest {
        device_delivery_id: String::new(),
        owner: Some(ActiveStreamOwner::chat(address)),
        after_seq_by_stream: BTreeMap::from([("turn-stream".to_string(), 99)]),
        activity_generation: Some("stale-generation".to_string()),
        conversation_activity_position: None,
        collapse_replay: false,
        initial_conversation_control: false,
        authority_epoch: None,
        viewer: None,
    };
    let mut stream = handler
        .subscribe_live_activity(&serde_json::to_vec(&request).expect("request"))
        .await
        .expect("subscribe with stale generation");

    let mut replayed = None;
    while let Some(bytes) = stream.next().await {
        let frame: ActiveStreamBroadcastFrame =
            serde_json::from_slice(&bytes).expect("activity frame");
        if let ActiveStreamBroadcastFrame::Event { payload, .. } = frame {
            replayed = Some(
                serde_json::from_str::<serde_json::Value>(&payload).expect("activity payload"),
            );
            break;
        }
    }

    let replayed = replayed.expect("stale generation must replay current activity");
    assert_eq!(replayed["delta"], "replayed");
    assert_eq!(replayed["activity_generation"], hub.activity_generation());
    assert_eq!(replayed["activity_sequence"], 1);
}

#[test]
fn gossip_wake_is_emitted_only_for_authority_scoped_chat_frames() {
    let chat = ChatLiveWake::from_frame(
        ActiveStreamBroadcastFrame::Started {
            device_delivery_id: "stream-1".to_string(),
            owner: ActiveStreamOwner::chat(ChatAddress::agent("default", CHAT_ID)),
            authority_epoch: Some(epoch(4)),
            seq: 0,
        },
        "device-a".to_string(),
    )
    .expect("authority-scoped Chat frame");
    assert_eq!(chat.authority_epoch, epoch(4));
    assert_eq!(chat.authority_device_id, "device-a");

    let terminal = ChatLiveWake::from_frame(
        ActiveStreamBroadcastFrame::Started {
            device_delivery_id: "terminal-stream".to_string(),
            owner: ActiveStreamOwner::terminal("terminal-session"),
            authority_epoch: None,
            seq: 0,
        },
        "device-a".to_string(),
    );
    assert!(
        terminal.is_none(),
        "Terminal activity must not enter Chat wake Gossip"
    );
}

#[test]
fn live_route_recovery_has_no_feature_owned_exponential_retry() {
    let source = include_str!("../../src/services/conversation_live_activity.rs");
    for forbidden in [
        "recovery_backoff",
        "Duration::from_secs(30)",
        "Duration::from_secs(300)",
    ] {
        assert!(
            !source.contains(forbidden),
            "Conversation Live Activity retained forbidden retry state: {forbidden}"
        );
    }
    assert!(
        source.contains("const REMOTE_LIVE_ATTACH_TIMEOUT: Duration = Duration::from_secs(10);"),
        "remote Live attach must allow one complete Device Session connect and handshake budget"
    );
    assert!(
        !source.contains("connect_timeout: Duration::from_secs(3)"),
        "remote Live attach must not truncate the shared Device Session timeout"
    );
    assert!(
        !source.contains("fetch_remote_control_snapshot")
            && !source.contains("ConversationControlDeviceAction::View"),
        "opening remote Live must receive its initial Control View from the Live logical stream, not a separate Command preflight"
    );
}
