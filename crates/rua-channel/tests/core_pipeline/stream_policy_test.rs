use std::time::Duration;

use rua_channel::core::streaming::{
    PlatformStreamAction, PlatformStreamMode, PlatformStreamPolicy, PlatformStreamState,
};

#[test]
fn placeholder_edit_policy_creates_throttles_and_finalizes() {
    let policy = PlatformStreamPolicy::placeholder_edit(Duration::from_millis(500), 100);
    let mut state = PlatformStreamState::default();

    assert_eq!(
        state.on_delta(&policy, Duration::from_millis(0), "hello"),
        PlatformStreamAction::Create {
            mode: PlatformStreamMode::PlaceholderEdit,
            content: "hello".into(),
        }
    );
    assert_eq!(
        state.on_delta(&policy, Duration::from_millis(200), "hello world"),
        PlatformStreamAction::Skip,
        "edits before min_update_interval should be throttled"
    );
    assert_eq!(
        state.on_delta(&policy, Duration::from_millis(700), "hello world"),
        PlatformStreamAction::Update {
            mode: PlatformStreamMode::PlaceholderEdit,
            content: "hello world".into(),
            final_update: false,
        }
    );
    assert_eq!(
        state.on_end(&policy, "hello world!"),
        PlatformStreamAction::Update {
            mode: PlatformStreamMode::PlaceholderEdit,
            content: "hello world!".into(),
            final_update: true,
        }
    );
}

#[test]
fn card_and_reply_stream_modes_share_policy_shape() {
    for mode_policy in [
        PlatformStreamPolicy::card_update(Duration::ZERO, 100),
        PlatformStreamPolicy::reply_stream(Duration::ZERO, 100),
    ] {
        let mut state = PlatformStreamState::default();
        let action = state.on_delta(&mode_policy, Duration::from_secs(1), "delta");
        match action {
            PlatformStreamAction::Create { mode, content } => {
                assert_eq!(mode, mode_policy.mode);
                assert_eq!(content, "delta");
            }
            other => panic!("expected create action, got {other:?}"),
        }
    }
}

#[test]
fn final_only_policy_suppresses_deltas_and_sends_final() {
    let policy = PlatformStreamPolicy::final_only();
    let mut state = PlatformStreamState::default();

    assert_eq!(
        state.on_delta(&policy, Duration::from_secs(1), "partial"),
        PlatformStreamAction::Skip
    );
    assert_eq!(
        state.on_end(&policy, "final answer"),
        PlatformStreamAction::SendFinal {
            content: "final answer".into(),
        }
    );
}

#[test]
fn overlong_text_falls_back_to_final_only() {
    let policy = PlatformStreamPolicy::placeholder_edit(Duration::ZERO, 5);
    let mut state = PlatformStreamState::default();

    assert_eq!(
        state.on_delta(&policy, Duration::from_secs(1), "too long"),
        PlatformStreamAction::Skip,
        "overlong text must not be sent as platform edit"
    );
    assert_eq!(
        state.on_end(&policy, "too long final"),
        PlatformStreamAction::SendFinal {
            content: "too long final".into(),
        }
    );
}

#[test]
fn end_without_any_delta_sends_final_when_available() {
    let policy = PlatformStreamPolicy::placeholder_edit(Duration::ZERO, 100);
    let mut state = PlatformStreamState::default();

    assert_eq!(
        state.on_end(&policy, "late final"),
        PlatformStreamAction::SendFinal {
            content: "late final".into(),
        }
    );
    assert_eq!(state.on_end(&policy, ""), PlatformStreamAction::Skip);
}
