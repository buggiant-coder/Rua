use rua_sync::hlc::{HlcClock, HlcTimestamp};

#[test]
fn monotonic_same_ms() {
    let device_id = [1u8; 8];
    let mut clock = HlcClock::new(device_id);
    let t1 = clock.now();
    let t2 = clock.now();
    let t3 = clock.now();
    assert!(t1 < t2);
    assert!(t2 < t3);
}

#[test]
fn monotonic_rapid_fire() {
    let device_id = [42u8; 8];
    let mut clock = HlcClock::new(device_id);
    let mut prev = clock.now();
    for _ in 0..1000 {
        let cur = clock.now();
        assert!(cur > prev, "HLC not monotonic: {:?} vs {:?}", prev, cur);
        prev = cur;
    }
}

#[test]
fn counter_increments_on_clock_stall() {
    let device_id = [7u8; 8];
    let mut clock = HlcClock::new(device_id);
    let t1 = clock.now();
    let t2 = clock.now();
    if t2.wall_ms == t1.wall_ms {
        assert!(t2.counter > t1.counter);
    }
}

#[test]
fn sortable_bytes_roundtrip() {
    let ts = HlcTimestamp {
        wall_ms: 1715900000000,
        counter: 42,
        device_id: [0xAB; 8],
    };
    let bytes = ts.to_sortable_bytes();
    let restored = HlcTimestamp::from_sortable_bytes(&bytes);
    assert_eq!(ts, restored);
}

#[test]
fn sortable_string_roundtrip() {
    let ts = HlcTimestamp {
        wall_ms: 1715900000000,
        counter: 0,
        device_id: [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08],
    };
    let s = ts.to_sortable_string();
    let restored = HlcTimestamp::from_sortable_string(&s).unwrap();
    assert_eq!(ts, restored);
}

#[test]
fn byte_ordering_matches_hlc_ordering() {
    let a = HlcTimestamp {
        wall_ms: 100,
        counter: 0,
        device_id: [0; 8],
    };
    let b = HlcTimestamp {
        wall_ms: 100,
        counter: 1,
        device_id: [0; 8],
    };
    let c = HlcTimestamp {
        wall_ms: 101,
        counter: 0,
        device_id: [0; 8],
    };
    assert!(a.to_sortable_bytes() < b.to_sortable_bytes());
    assert!(b.to_sortable_bytes() < c.to_sortable_bytes());
}

#[test]
fn resume_preserves_monotonicity() {
    let device_id = [3u8; 8];
    let mut clock = HlcClock::new(device_id);
    let t1 = clock.now();
    let t2 = clock.now();

    let mut clock2 = HlcClock::resume(device_id, t2);
    let t3 = clock2.now();
    assert!(t3 > t2);
    assert!(t3 > t1);
}

#[test]
fn different_devices_break_ties() {
    let a = HlcTimestamp {
        wall_ms: 500,
        counter: 0,
        device_id: [0x00; 8],
    };
    let b = HlcTimestamp {
        wall_ms: 500,
        counter: 0,
        device_id: [0x01; 8],
    };
    assert!(a < b);
}
