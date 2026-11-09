use llm_multiplex::KeySelector;

#[test]
fn single_key_always_returns_zero() {
    let sel = KeySelector::new(1);
    assert_eq!(sel.acquire(), 0);
    assert_eq!(sel.acquire(), 0);
    assert_eq!(sel.in_flight_for(0), 2);
    sel.release(0);
    assert_eq!(sel.in_flight_for(0), 1);
}

#[test]
fn three_keys_balance_evenly() {
    let sel = KeySelector::new(3);
    let mut counts = [0usize; 3];
    for _ in 0..30 {
        let idx = sel.acquire();
        counts[idx] += 1;
    }

    for i in 0..3 {
        assert_eq!(
            sel.in_flight_for(i),
            10,
            "key {i} has {} in-flight",
            sel.in_flight_for(i)
        );
    }
    assert_eq!(counts, [10, 10, 10]);
}

#[test]
fn picks_least_in_flight() {
    let sel = KeySelector::new(3);

    let first = sel.acquire();
    let second = sel.acquire();
    let third = sel.acquire();
    let mut picked = vec![first, second, third];
    picked.sort();
    assert_eq!(picked, vec![0, 1, 2]);

    sel.release(second);
    assert_eq!(sel.acquire(), second);
}

#[test]
fn round_robin_tie_break() {
    let sel = KeySelector::new(3);

    let a = sel.acquire();
    let b = sel.acquire();
    let c = sel.acquire();

    assert_eq!(sel.in_flight_for(0), 1);
    assert_eq!(sel.in_flight_for(1), 1);
    assert_eq!(sel.in_flight_for(2), 1);

    let mut picked = vec![a, b, c];
    picked.sort();
    assert_eq!(picked, vec![0, 1, 2]);
}
