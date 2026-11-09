# Local iroh patch

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
