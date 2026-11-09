use std::sync::Arc;

use rua_core::ChatAddress;
use rua_server::services::active_stream_broadcast::{
    ActiveStreamBroadcastFrame, ActiveStreamBroadcastHub, ActiveStreamOwner,
};
use rua_server::services::chat_activity_wake::{ChatActivityWakeHub, ChatLiveWake};
use rua_server::services::conversation_authority::{
    ConversationActiveTurnStart, ConversationAuthority, ConversationDriverKind,
};
use rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch;
use rua_server::services::conversation_navigation_activity::{
    ConversationNavigationEvent, NavigationActiveTurn, NavigationActivityChange,
    NavigationActivityFact,
};
use serde_json::{json, Value};

fn address() -> ChatAddress {
    ChatAddress::agent("default", "one")
}
fn epoch() -> ConversationAuthorityEpoch {
    ConversationAuthorityEpoch::from_parts(10, 0, [1; 8])
}
fn active(turn: &str) -> Option<NavigationActiveTurn> {
    Some(NavigationActiveTurn {
        turn_id: turn.into(),
        started_at: "2026-10-23T10:00:00Z".into(),
    })
}
fn wake(sequence: u64, change: NavigationActivityChange) -> ChatLiveWake {
    ChatLiveWake {
        kind: "chat_live_wake".into(),
        version: 2,
        address: address(),
        stream_id: "one".into(),
        phase: "activity".into(),
        authority_device_id: "local".into(),
        authority_epoch: epoch(),
        base_manifest_hash: None,
        activity: Some(NavigationActivityFact {
            generation: "generation-one".into(),
            sequence,
            change,
        }),
    }
}
fn control(sequence: u64, turn: Option<NavigationActiveTurn>) -> ChatLiveWake {
    wake(
        sequence,
        NavigationActivityChange::Control {
            revision: sequence,
            active_turn: turn,
        },
    )
}
fn terminal(sequence: u64, turn: &str, status: &str, completed: &str) -> ChatLiveWake {
    wake(
        sequence,
        NavigationActivityChange::Terminal {
            turn_id: turn.into(),
            status: status.into(),
            completed_at: Some(completed.into()),
        },
    )
}
fn snapshot(hub: &ChatActivityWakeHub) -> Value {
    serde_json::to_value(hub.navigation.subscribe().0).unwrap()
}

#[tokio::test]
async fn real_authority_turns_update_the_snapshot_without_a_subscriber_or_network() {
    let hub = ChatActivityWakeHub::new();
    let stream_hub = Arc::new(ActiveStreamBroadcastHub::new());
    stream_hub.set_navigation_activity_hub(Arc::clone(&hub));
    let authority = ConversationAuthority::new();
    authority.set_active_stream_broadcast_hub(Arc::clone(&stream_hub));
    let turn = authority
        .start_active_turn(ConversationActiveTurnStart {
            address: address(),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "local".into(),
            authority_epoch: epoch(),
            driver_detail: None,
        })
        .await;
    assert_eq!(snapshot(&hub)["activities"][0]["running"], true);
    stream_hub
        .publish_conversation_event(
            "turn-one",
            &address(),
            epoch(),
            json!({
                "type": "turn_terminal", "turn_id": turn.turn_id, "status": "completed",
                "completed_at": chrono::Utc::now().to_rfc3339(), "authority_device_id": "local"
            })
            .to_string(),
        )
        .await;
    authority.finish_active_turn(&address()).await;
    let view = snapshot(&hub);
    assert_eq!(view["activities"][0]["running"], false);
    assert_eq!(view["activities"][0]["completion"]["turn_id"], turn.turn_id);
    assert!(view["activities"][0]["completion"]["token"]
        .as_str()
        .unwrap()
        .contains(&turn.turn_id));
}

#[test]
fn snapshots_include_success_and_ignore_metadata_transport_starts_and_failures() {
    let hub = ChatActivityWakeHub::new();
    let mut transport = control(1, None);
    transport.activity = None;
    transport.phase = "started".into();
    hub.publish(transport);
    assert!(snapshot(&hub)["activities"].as_array().unwrap().is_empty());
    hub.publish(control(2, active("first")));
    let (_, mut receiver) = hub.navigation.subscribe();
    hub.publish(control(3, active("first")));
    assert!(
        receiver.try_recv().is_err(),
        "configuration-only control is not list activity"
    );
    hub.publish(terminal(4, "first", "failed", "2026-10-23T10:00:02Z"));
    assert_eq!(snapshot(&hub)["activities"][0]["running"], false);
    assert!(snapshot(&hub)["activities"][0].get("completion").is_none());
    hub.publish(control(5, active("second")));
    hub.publish(terminal(6, "second", "completed", "2026-10-23T10:00:03Z"));
    let complete = snapshot(&hub);
    hub.publish(terminal(6, "second", "completed", "2026-10-23T10:00:03Z"));
    assert_eq!(snapshot(&hub), complete, "repeated terminal is idempotent");
}

#[test]
fn delayed_terminal_preserves_next_turn_and_completion_even_after_idle_control() {
    let hub = ChatActivityWakeHub::new();
    hub.publish(control(1, active("first")));
    hub.publish(control(4, active("second")));
    hub.publish(terminal(2, "first", "completed", "2026-10-23T10:00:01Z"));
    let view = snapshot(&hub);
    assert_eq!(view["activities"][0]["running"], true);
    assert_eq!(view["activities"][0]["completion"]["turn_id"], "first");
    hub.publish(control(3, None));
    assert_eq!(
        snapshot(&hub),
        view,
        "old idle control cannot stop the next turn"
    );
    hub.publish(control(6, None));
    hub.publish(terminal(5, "second", "completed", "2026-10-23T10:00:02Z"));
    assert_eq!(
        snapshot(&hub)["activities"][0]["completion"]["turn_id"],
        "second"
    );
}

#[test]
fn stale_epochs_and_retired_generations_cannot_restore_running() {
    let hub = ChatActivityWakeHub::new();
    hub.publish(control(5, active("first")));
    let mut replacement = control(1, None);
    replacement.activity.as_mut().unwrap().generation = "generation-two".into();
    hub.publish(replacement);
    hub.publish(control(6, active("first")));
    assert_eq!(snapshot(&hub)["activities"][0]["running"], false);
    let mut old_epoch = control(9, active("old"));
    old_epoch.authority_epoch = ConversationAuthorityEpoch::from_parts(9, 0, [1; 8]);
    hub.publish(old_epoch);
    assert_eq!(snapshot(&hub)["activities"][0]["running"], false);
    let mut returned_authority = control(10, active("return-to-first-device"));
    returned_authority.authority_epoch = ConversationAuthorityEpoch::from_parts(11, 0, [1; 8]);
    hub.publish(returned_authority);
    assert_eq!(snapshot(&hub)["activities"][0]["running"], true);
}

#[test]
fn regenerated_turn_and_rfc3339_offsets_keep_the_newest_completion() {
    let hub = ChatActivityWakeHub::new();
    hub.publish(terminal(1, "same", "completed", "2026-10-23T10:00:00Z"));
    let first = snapshot(&hub)["activities"][0]["completion"]["token"].clone();
    hub.publish(terminal(
        2,
        "same",
        "completed",
        "2026-10-23T12:00:00.100+02:00",
    ));
    let latest = snapshot(&hub);
    assert_ne!(latest["activities"][0]["completion"]["token"], first);
    hub.publish(terminal(1, "same", "completed", "2026-10-23T10:00:00Z"));
    assert_eq!(snapshot(&hub), latest);
}

#[test]
fn distinct_chat_scopes_and_atomic_snapshot_subscription() {
    let hub = ChatActivityWakeHub::new();
    hub.publish(control(1, active("agent")));
    let (initial, mut updates) = hub.navigation.subscribe();
    let mut project = control(1, active("project"));
    project.address = ChatAddress::project("default", "one");
    hub.publish(project);
    assert!(
        matches!(initial, ConversationNavigationEvent::Snapshot { activities } if activities.len() == 1)
    );
    assert!(
        matches!(updates.try_recv().unwrap(), ConversationNavigationEvent::Activity { activity } if activity.address == ChatAddress::project("default", "one"))
    );
    assert_eq!(snapshot(&hub)["activities"].as_array().unwrap().len(), 2);
}

#[test]
fn old_wake_shape_round_trips_and_non_turn_frames_have_no_activity_fact() {
    let old: ChatLiveWake = serde_json::from_value(json!({
        "type": "chat_live_wake", "version": 2, "address": address(), "stream_id": "control",
        "phase": "started", "authority_device_id": "remote", "authority_epoch": epoch()
    }))
    .unwrap();
    assert!(old.activity.is_none());
    let parsed = ChatLiveWake::from_frame(
        ActiveStreamBroadcastFrame::Started {
            device_delivery_id: "control".into(),
            owner: ActiveStreamOwner::chat(address()),
            authority_epoch: Some(epoch()),
            seq: 0,
        },
        "remote".into(),
    )
    .unwrap();
    assert!(parsed.activity.is_none());
}
