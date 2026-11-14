# Local iroh patch

Upgrade checklist, platform pitfalls, and measured acceptance baseline:
[Rua iroh upgrade and sync recovery guide](../../docs/iroh-upgrade-and-sync-recovery.md).
Verify equivalent upstream behavior and the listed regressions before removing a local patch.

Base: crates.io iroh 1.0.0-rc.0 (MIT OR Apache-2.0); remaining files unmodified.
Registered as a workspace member so its dev-dependencies participate in the
touched-crate Clippy check, like the other vendored iroh crates.

`src/socket/remote_map/remote_state.rs` retains at most eight pre-connection
handshake transmits (64 KiB total, usable for ten seconds) and immediately
tries them on newly discovered, non-local paths. Previously a fresh HTTPS
lookup result could wait behind exponential retransmission of a handshake
sent to an obsolete DNS address. Clear the flight when a connection is added;
never replay established application traffic. Duplicate discovery results do
not trigger additional sends.

Regression: `rua-device-network --test contract_session discovery_handoff`.
New handshakes also try the selected path plus all other known paths, and
always initiate discovery unless a lookup is already running. A stale selected
path from a silently restarted peer must not trap subsequent connections.

Related upstream issue: https://github.com/n0-computer/iroh/issues/4600 .
Remove this patch once an upstream release provides equivalent behavior and
the delayed-discovery regression passes against that release.

Relay backpressure fix (same blocked-peer-actor failure as upstream
https://github.com/n0-computer/iroh/pull/4512): retain the original handshake
send ordering and delayed-discovery replay. If polling a relay send finds its
bounded queue full, drop that Initial datagram and let QUIC retransmit. Do not
wait for relay capacity inside the peer actor, and do not queue additional
handshake send tasks. Direct sends keep their existing behavior. This prevents
an offline relay from blocking address queries, new direct paths, and shutdown.

Regression: `cargo test -p iroh --lib backpressure_tests` uses the real 256-item
relay send queue and checks control responsiveness, delayed direct discovery,
shutdown, and absence of stale handshake delivery after capacity returns.

Backport of upstream https://github.com/n0-computer/iroh/pull/4444:
`src/socket/transports/relay/actor.rs` keeps answering priority route queries
while waiting out relay reconnect backoff (up to 16 seconds). A disconnected
relay immediately answers that it has no route, so it cannot stall other
relay lookups. Cancellation also interrupts the wait without shortening
normal reconnect backoff.

Regression: `cargo test -p iroh --lib relay_backoff_` checks prompt route replies
and cancellation during a ten-second backoff using Tokio's paused clock.

Backport of upstream https://github.com/n0-computer/iroh/pull/4551
(merged as `43e232f`): deduplicate pending path-open retries. Each retry tick
tries an address on every connection, so requeueing once per failed connection
multiplied the next tick's work. Keep the upstream 333 ms retry timing and one
queued entry per distinct address. The enqueue operation is extracted solely
to exercise the production retry insertion in the regression test.

Regression: `cargo test -p iroh --lib path_retry_work_stays_bounded` models
four exhausted connections across twenty retry ticks and verifies that both
distinct addresses remain scheduled without multiplying work.

Backport of upstream https://github.com/n0-computer/iroh/pull/4284
(merged as `4eea244`): a path abandoned by one connection no longer closes that
address on every sibling connection. Each QUIC connection validates and abandons
its own paths. This preserves a working replacement connection when an older
connection reports a delayed path failure during network recovery.

Regression: `cargo test -p iroh --lib abandoned_path_does_not_close` uses two
real multipath QUIC connections and a UDP forwarder, then checks that abandoning
one primary path does not emit an abandonment on the healthy sibling.

Connection-local path selection: retain the peer-wide selected address as a dial
hint, but choose established transmission paths using each connection's own RTTs
and existing switching threshold. A fast path on an old connection must not mark
a new connection's working path as Backup or close it when that connection has
not established the preferred address. The path watcher now records the selected
path for its own connection. This extends the isolation enforced by #4284 to path
selection; it does not change discovery, handshake replay, or QUIC retry timers.

The same real-connection regression also retains an old connection on one address
and a new one on another, then verifies that the new connection keeps an Available
path. Before this change it deterministically became Backup.

As in upstream https://github.com/n0-computer/iroh/pull/4296, redundant direct
paths are pruned even if selection is unchanged, so path IDs remain available
for later relay and NAT paths. The regression checks retirement of the unused
path while preserving the new connection's own working path.

Android network-change readiness: `src/socket.rs` does not require a visible
kernel default route on Android. netwatch 0.17 invokes `/system/bin/ip route
show table 0`, which ordinary Android app UIDs cannot query (`Cannot bind
netlink socket: Permission denied`). Treating that unknown route as absent
holds the QUIC recovery notification for the five-second default-route wait.
Still require a usable IPv4 or IPv6 interface; other platforms retain their
existing route check. APK network callbacks continue to use the official
`Endpoint::network_change()` API; no additional connection manager is added.

Related upstream failure mechanism: https://github.com/n0-computer/iroh/issues/4657
(the upstream report reproduces the delayed notification on a route-less LAN).
Validate this platform branch with native APK network-loss/recovery acceptance;
a macOS-only predicate test cannot exercise Android's route permissions.
