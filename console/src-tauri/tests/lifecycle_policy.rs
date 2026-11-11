use rua_desktop_lib::runtime::{
    stop_disposition, wait_for_termination, RuntimeOwnership, StopDisposition,
};
use rua_desktop_lib::tray::{
    rua_root_switch_enabled, secondary_launch_action, window_close_action, ExitRequestAction,
    SecondaryLaunchAction, TrayState, WindowCloseAction,
};

#[test]
fn a_second_launch_focuses_the_existing_window() {
    assert_eq!(
        secondary_launch_action(&[]),
        SecondaryLaunchAction::ShowMainWindow
    );
}

#[test]
fn a_secondary_quit_request_uses_the_same_explicit_quit_path() {
    assert_eq!(
        secondary_launch_action(&["rua-desktop".into(), "--quit".into()]),
        SecondaryLaunchAction::Quit
    );
}

#[test]
fn an_os_exit_request_is_intercepted_until_explicit_quit_finishes() {
    let state = TrayState::default();

    assert_eq!(
        state.exit_request_action(),
        ExitRequestAction::PreventAndQuit
    );
}

#[test]
fn closing_the_window_hides_to_tray_instead_of_quitting() {
    assert_eq!(window_close_action(), WindowCloseAction::HideToTray);
}

#[test]
fn environment_managed_rua_root_disables_the_switch_menu() {
    assert!(rua_root_switch_enabled(false));
    assert!(!rua_root_switch_enabled(true));
}

#[test]
fn concurrent_quit_requests_wait_for_the_shared_stop_operation() {
    assert_eq!(
        stop_disposition(RuntimeOwnership::Owned, true, true),
        StopDisposition::WaitForSharedStop
    );
}

#[test]
fn attached_unowned_runtime_is_never_stopped_by_the_shell() {
    assert_eq!(
        stop_disposition(RuntimeOwnership::Attached, true, false),
        StopDisposition::NoProcess
    );
}

#[test]
fn owned_runtime_receives_the_stop_request() {
    assert_eq!(
        stop_disposition(RuntimeOwnership::Owned, true, false),
        StopDisposition::RequestStop
    );
}

#[tokio::test]
async fn a_graceful_shutdown_timeout_requires_escalation() {
    let (_sender, receiver) = tokio::sync::watch::channel(false);

    let error = wait_for_termination(receiver, std::time::Duration::from_millis(5))
        .await
        .expect_err("termination wait must time out");

    assert!(error.contains("timed out"));
}

#[tokio::test]
async fn shared_stop_waiters_complete_from_one_termination_event() {
    let (sender, receiver) = tokio::sync::watch::channel(false);
    let first = wait_for_termination(receiver.clone(), std::time::Duration::from_secs(1));
    let second = wait_for_termination(receiver, std::time::Duration::from_secs(1));
    sender.send_replace(true);

    let (first, second) = tokio::join!(first, second);

    assert!(first.is_ok());
    assert!(second.is_ok());
}

#[tokio::test]
async fn exit_rejects_a_startup_already_queued_behind_update() {
    let lifecycle = rua_desktop_lib::update::DesktopLifecycle::default();
    let update = lifecycle.begin_operation().await.unwrap();
    let startup = lifecycle.begin_operation();
    tokio::pin!(startup);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(5), &mut startup)
            .await
            .is_err()
    );
    lifecycle.request_exit();
    drop(update);
    assert!(startup.await.unwrap_err().contains("shutting down"));
    assert!(lifecycle.begin_operation().await.is_err());
}

#[test]
fn exit_event_is_allowed_only_after_owned_shutdown_finishes() {
    let state = TrayState::default();
    assert_eq!(
        state.exit_request_action(),
        ExitRequestAction::PreventAndQuit
    );
    state.complete_shutdown();
    assert_eq!(state.exit_request_action(), ExitRequestAction::Allow);
}

#[test]
fn repeated_quit_requests_share_one_confirmation_and_cancel_allows_retry() {
    let state = TrayState::default();
    assert!(state.begin_quit_confirmation());
    assert!(!state.begin_quit_confirmation());
    state.finish_quit_confirmation();
    assert!(state.begin_quit_confirmation());
}

#[test]
fn confirmation_does_not_mark_shutdown_complete() {
    let state = TrayState::default();
    state.begin_quit_confirmation();
    assert_eq!(
        state.exit_request_action(),
        ExitRequestAction::PreventAndQuit
    );
    state.finish_quit_confirmation();
    assert_eq!(
        state.exit_request_action(),
        ExitRequestAction::PreventAndQuit
    );
}
