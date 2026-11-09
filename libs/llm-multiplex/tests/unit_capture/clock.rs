use std::time::Duration;

use crate::clock::{CaptureClock, ClockSnapshot};

#[test]
fn suspension_is_separated_from_clock_adjustment() {
    // 60 seconds awake + 287 seconds asleep. Wall time also moved forward 2 s.
    let snapshot = ClockSnapshot::from_elapsed(
        349_000,
        Duration::from_secs(60),
        349_000.0,
        Some(Duration::from_secs(347)),
    );
    assert_eq!(snapshot.elapsed_ms, 60_000.0);
    assert_eq!(snapshot.wall_elapsed_ms, 349_000.0);
    assert_eq!(snapshot.elapsed_including_suspend_ms, Some(347_000.0));
    assert_eq!(snapshot.suspend_elapsed_ms, Some(287_000.0));
    assert_eq!(snapshot.wall_clock_adjustment_ms, Some(2_000.0));
}

#[test]
fn forward_and_backward_wall_jumps_are_not_called_sleep() {
    for wall_elapsed in [600_000.0, -600_000.0] {
        let snapshot = ClockSnapshot::from_elapsed(
            1,
            Duration::from_secs(3),
            wall_elapsed,
            Some(Duration::from_secs(3)),
        );
        assert_eq!(snapshot.suspend_elapsed_ms, Some(0.0));
        assert_eq!(
            snapshot.wall_clock_adjustment_ms,
            Some(wall_elapsed - 3_000.0)
        );
    }
}

#[test]
fn unavailable_native_clock_does_not_fabricate_sleep_or_zero() {
    let snapshot = ClockSnapshot::from_elapsed(10_000, Duration::from_secs(3), 10_000.0, None);
    assert_eq!(snapshot.elapsed_including_suspend_ms, None);
    assert_eq!(snapshot.suspend_elapsed_ms, None);
    assert_eq!(snapshot.wall_clock_adjustment_ms, None);
    assert_eq!(snapshot.wall_monotonic_delta_ms, 7_000.0);
}

#[test]
fn native_snapshots_share_a_stable_origin_and_report_wall_timestamps() {
    let clock = CaptureClock::new();
    let before = clock.snapshot();
    std::thread::sleep(Duration::from_millis(10));
    let after = clock.clone().snapshot();
    let later_clock = CaptureClock::new();
    assert_eq!(clock.process_clock_id(), later_clock.process_clock_id());
    assert!(later_clock.started_at_process_ms() - clock.started_at_process_ms() >= 10.0);
    assert!(before.observed_at_unix_ms >= clock.started_at_unix_ms());
    assert!(after.elapsed_ms - before.elapsed_ms >= 10.0);
    assert!(after.wall_elapsed_ms - before.wall_elapsed_ms >= 10.0);
    if cfg!(target_os = "macos") {
        assert!(after.elapsed_including_suspend_ms.unwrap() >= 10.0);
        assert!(after.suspend_elapsed_ms.unwrap() < 5.0);
        assert!(after.wall_clock_adjustment_ms.unwrap().abs() < 5.0);
    }
}
