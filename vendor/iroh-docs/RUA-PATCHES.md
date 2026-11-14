# Sync completion and transport close

Upgrade checklist, platform pitfalls, and measured acceptance baseline:
[Rua iroh upgrade and sync recovery guide](../../docs/iroh-upgrade-and-sync-recovery.md).
Verify equivalent upstream behavior and the listed regressions before removing a local patch.

The live actor releases an accepted peer/namespace exchange when the sync
protocol finishes, before waiting for QUIC stream shutdown. The same owned task
keeps the connection and streams alive until all queued data and FIN are drained.
A close failure is logged without completing the exchange a second time, so an
old close cannot clear the state of a later sync. Actor shutdown still cancels
the task through its existing JoinSet.

Previously a peer killed just after reconciliation could keep AlreadySyncing
active until QUIC timed out. The public net::handle_connection API still waits
for transport close; the live actor uses the internal staged result.

Regression: cargo test -p iroh-docs --lib completed_sync_does_not_wait_for_the_previous_peers_fin
uses a real Docs router, deliberately withholds a client's FIN after protocol
completion, and verifies the completion event and a second sync from that peer.
