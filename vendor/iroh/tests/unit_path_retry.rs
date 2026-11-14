//! Exercise the actor's retry queue under multiple exhausted connections.
use super::*;

#[test]
fn path_retry_work_stays_bounded_across_connection_fanout() {
    let addrs = [
        transports::Addr::Ip("127.0.0.1:10001".parse().unwrap()),
        transports::Addr::Ip("127.0.0.1:10002".parse().unwrap()),
    ];
    let mut pending = VecDeque::new();
    for addr in &addrs {
        queue_path_retry(&mut pending, addr);
    }
    for _ in 0..20 {
        // Each retry tick drains addresses and tries every live connection.
        // Four exhausted connections then requeue the same address.
        for addr in std::mem::take(&mut pending) {
            for _ in 0..4 {
                queue_path_retry(&mut pending, &addr);
            }
        }
        assert_eq!(
            pending.len(),
            addrs.len(),
            "connection fanout must not multiply queued address retries"
        );
        for addr in &addrs {
            assert!(pending.contains(addr));
        }
    }
}
