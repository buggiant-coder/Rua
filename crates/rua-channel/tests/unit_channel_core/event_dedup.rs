use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use rua_channel::core::{EventDedupDecision, EventDeduper};

fn deduper_with_clock(now: Arc<Mutex<SystemTime>>) -> EventDeduper {
    EventDeduper::with_now(
        Duration::from_secs(60),
        2,
        Box::new(move || *now.lock().unwrap()),
    )
}

#[test]
fn allows_first_event_and_rejects_duplicate_within_ttl() {
    let now = Arc::new(Mutex::new(SystemTime::UNIX_EPOCH));
    let mut deduper = deduper_with_clock(now);

    assert_eq!(deduper.check_and_mark("event-1"), EventDedupDecision::New);
    assert_eq!(
        deduper.check_and_mark("event-1"),
        EventDedupDecision::Duplicate
    );
}

#[test]
fn allows_expired_event_again() {
    let now = Arc::new(Mutex::new(SystemTime::UNIX_EPOCH));
    let mut deduper = deduper_with_clock(now.clone());

    assert_eq!(deduper.check_and_mark("event-1"), EventDedupDecision::New);
    *now.lock().unwrap() = SystemTime::UNIX_EPOCH + Duration::from_secs(61);

    assert_eq!(deduper.check_and_mark("event-1"), EventDedupDecision::New);
}

#[test]
fn distinct_events_do_not_collide() {
    let now = Arc::new(Mutex::new(SystemTime::UNIX_EPOCH));
    let mut deduper = deduper_with_clock(now);

    assert_eq!(deduper.check_and_mark("event-1"), EventDedupDecision::New);
    assert_eq!(deduper.check_and_mark("event-2"), EventDedupDecision::New);
}

#[test]
fn capacity_is_bounded_and_evicts_oldest() {
    let now = Arc::new(Mutex::new(SystemTime::UNIX_EPOCH));
    let mut deduper = deduper_with_clock(now);

    assert_eq!(deduper.check_and_mark("event-1"), EventDedupDecision::New);
    assert_eq!(deduper.check_and_mark("event-2"), EventDedupDecision::New);
    assert_eq!(deduper.check_and_mark("event-3"), EventDedupDecision::New);

    assert_eq!(deduper.len(), 2);
    assert_eq!(deduper.check_and_mark("event-1"), EventDedupDecision::New);
}

#[test]
fn clear_drops_all_seen_events() {
    let now = Arc::new(Mutex::new(SystemTime::UNIX_EPOCH));
    let mut deduper = deduper_with_clock(now);

    assert_eq!(deduper.check_and_mark("event-1"), EventDedupDecision::New);
    deduper.clear();

    assert!(deduper.is_empty());
    assert_eq!(deduper.check_and_mark("event-1"), EventDedupDecision::New);
}
