use std::net::{IpAddr, Ipv4Addr};

use rua_desktop::runtime::{
    packaged_binary_next_to, ready_endpoint_from_stdout, RuntimeTracker, DESKTOP_READY_PREFIX,
};

#[test]
fn structured_ready_output_is_parsed_and_validated() {
    let line = format!(
        "{}{}",
        DESKTOP_READY_PREFIX,
        serde_json::json!({
            "version": 1,
            "host": IpAddr::V4(Ipv4Addr::LOCALHOST),
            "port": 43123,
            "pid": 99,
            "rua_version": "0.1.0"
        })
    );

    let endpoint = ready_endpoint_from_stdout(&line).expect("ready endpoint");
    assert_eq!(endpoint.port, 43123);
    assert_eq!(endpoint.pid, 99);
    assert_eq!(endpoint.rua_version, "0.1.0");
}

#[test]
fn structured_ready_output_rejects_wrong_version_and_non_loopback_hosts() {
    for payload in [
        serde_json::json!({
            "version": 2,
            "host": "127.0.0.1",
            "port": 43123,
            "pid": 99,
            "rua_version": "0.1.0"
        }),
        serde_json::json!({
            "version": 1,
            "host": "0.0.0.0",
            "port": 43123,
            "pid": 99,
            "rua_version": "0.1.0"
        }),
    ] {
        let line = format!("{DESKTOP_READY_PREFIX}{payload}");
        assert!(ready_endpoint_from_stdout(&line).is_none());
    }
}

#[test]
fn stale_generation_events_cannot_replace_current_runtime_state() {
    let tracker = RuntimeTracker::default();
    let first = tracker.begin_generation();
    let second = tracker.begin_generation();

    assert!(!tracker.mark_ready_if_current(first, 41001));
    assert!(!tracker.mark_error_if_current(first, "stale failure".to_string()));
    assert!(tracker.mark_ready_if_current(second, 41002));

    let status = tracker.status();
    assert_eq!(status.generation, second);
    assert_eq!(status.port, Some(41002));
    assert_eq!(status.error, None);
}

#[test]
fn current_generation_termination_is_visible_to_bootstrap() {
    let tracker = RuntimeTracker::default();
    let generation = tracker.begin_generation();

    assert!(tracker.mark_terminated_if_current(
        generation,
        "Runtime exited before becoming ready".to_string()
    ));

    let status = tracker.status();
    assert!(status.terminated);
    assert_eq!(
        status.error.as_deref(),
        Some("Runtime exited before becoming ready")
    );
}

#[test]
fn packaged_runtime_lives_next_to_the_desktop_executable() {
    let executable = std::path::Path::new("/Applications/Rua.app/Contents/MacOS/rua-desktop");
    let expected_name = if cfg!(windows) { "rua.exe" } else { "rua" };

    assert_eq!(
        packaged_binary_next_to(executable),
        Some(std::path::Path::new("/Applications/Rua.app/Contents/MacOS").join(expected_name))
    );
}
