use chrono::{Duration, Utc};
use rua_config::types::cron::ScheduleSpec;
use rua_cron::compute_next_run_for_schedule;

fn once_schedule(run_at_offset: Duration) -> ScheduleSpec {
    ScheduleSpec {
        r#type: "once".to_string(),
        cron: String::new(),
        run_at: Some(Utc::now() + run_at_offset),
        timezone: "UTC".to_string(),
        repeat_every_days: None,
        repeat_end_type: None,
        repeat_until: None,
        repeat_count: None,
        extra: Default::default(),
    }
}

#[test]
fn once_schedule_returns_future_run_at() {
    let schedule = once_schedule(Duration::hours(1));
    let next = compute_next_run_for_schedule(&schedule).expect("next run");

    assert!(next > Utc::now());
    assert_eq!(next, schedule.run_at.unwrap());
}

#[test]
fn once_schedule_in_past_without_repeat_has_no_next_run() {
    let schedule = once_schedule(Duration::hours(-1));

    assert!(compute_next_run_for_schedule(&schedule).is_none());
}

#[test]
fn repeated_once_schedule_returns_next_interval() {
    let mut schedule = once_schedule(Duration::hours(-23));
    schedule.repeat_every_days = Some(1);
    schedule.repeat_end_type = Some("never".to_string());

    let next = compute_next_run_for_schedule(&schedule).expect("next repeated run");

    assert!(next > Utc::now());
    assert!(next < Utc::now() + Duration::hours(2));
}

#[test]
fn repeated_once_schedule_stops_after_count() {
    let mut schedule = once_schedule(Duration::days(-3));
    schedule.repeat_every_days = Some(1);
    schedule.repeat_end_type = Some("count".to_string());
    schedule.repeat_count = Some(2);

    assert!(compute_next_run_for_schedule(&schedule).is_none());
}

#[test]
fn repeated_once_schedule_stops_after_until() {
    let mut schedule = once_schedule(Duration::days(-3));
    schedule.repeat_every_days = Some(1);
    schedule.repeat_end_type = Some("until".to_string());
    schedule.repeat_until = Some(Utc::now() - Duration::days(1));

    assert!(compute_next_run_for_schedule(&schedule).is_none());
}
