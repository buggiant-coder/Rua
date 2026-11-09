# ADR-0040: SQLite Inbound Inbox and Sync Sidecar Store

> **Status:** Accepted

## Context

Inbound replication needs durable work scheduling and idempotency across
process restarts. It must record remote replica entries before applying them,
retry unavailable content, distinguish terminal and non-terminal failures, and
advance record versions only after canonical domain writes succeed.

Whole-file JSON maps made every lookup or transition rewrite unrelated state.
They also could not atomically commit the applied operation, record cursor, and
inbox terminal state. The iroh-docs database is transport-owned and must not be
extended with Rua-specific application bookkeeping.

## Decision

### D1 - One SQLite database backs the transactional core

The Device-local file `sync/sync-state.sqlite` backs the Inbound Inbox and Sync
Sidecar Store's applied-operation, cursor, snapshot-reference, and repair
metadata. Both modules open the shared schema through
`sync_state::open_connection`.

SQLite uses bounded transactions for state transitions, indexed ready-work
queries, counts, and repair metadata.

Conflict policy and pending conflict review entries are still stored in
`conflict_policy.json` and `pending_conflicts.json`. They are Device-local
sidecar state, but they are outside the SQLite transaction boundary and this
record must not describe the entire Sidecar Store as SQLite-backed.

### D2 - Inbound Inbox is the durable apply work queue

The inbox records remote replica coordinates and content hashes keyed by
`(author, key)`. States distinguish pending, retryable, dormant, discarded,
applied, and failed work. Retry metadata and last errors are local operational
facts.

Recording arrival is cheap and does not require decrypting or applying the
payload in the transport callback.

### D3 - Sidecar Store owns idempotency and record cursors

The sidecar stores applied `op_id` records and per-record cursors. A cursor
contains the winning HLC, live/tombstoned state, and optional base-snapshot blob
reference used by merge algorithms.

Base snapshot bytes remain in the sidecar snapshot directory rather than in
SQLite rows. Canonical domain content remains in domain-owned files or
databases.

### D4 - Apply completion is one bookkeeping transaction

After a domain adapter successfully commits canonical state, one SQLite
transaction records the operation as applied, advances all affected cursors,
and moves the inbound row to its terminal applied state.

If canonical apply fails, that transaction does not commit. The item remains
eligible for the appropriate retry or failure transition.

The canonical domain write and the later SQLite bookkeeping transaction are not
one cross-store transaction. A crash between them leaves the Inbox row
non-terminal; retry must therefore make domain apply idempotent and then commit
the operation, cursors, and Inbox state together.

### D5 - Missing content is durable but not busy-looped

An entry whose encrypted blob content is not yet readable moves through bounded
retry and may become dormant. Blob-ready notification can reactivate matching
dormant rows. The inbox does not continuously scan or spin while a provider is
unavailable.

### D6 - Repair metadata lives beside inbox state

The database records Repair Scan attempts, last successful completion,
duration, counts, and errors. This state decides whether the daily safety scan
is due; it is not synchronized and does not represent canonical business data.

### D7 - The store is rebuildable operational state

The database may be reconstructed from current replica entries and canonical
local state. Losing it can require a Repair Scan and reapplication decisions,
but it must not delete canonical domain content or change the sync network
identity.

## Hard invariants

- Inbound arrival is persisted before asynchronous apply work is considered
  accepted.
- Applied `op_id`, record cursor, and inbox applied state commit atomically.
- A cursor never advances before canonical domain persistence succeeds.
- Domain apply must tolerate replay after a crash between canonical persistence
  and SQLite bookkeeping.
- Inbox and sidecar state are Device-local and never replicated.
- `sync-state.sqlite` is not the canonical store for any product domain.
- iroh-docs internal storage is not used for Rua bookkeeping tables.
- Missing blob content does not cause an unbounded retry loop.
- Tombstone cursor state survives canonical file removal.
- Base snapshots remain independently addressable for merge.
- Conflict policy and pending conflict JSON are not part of the atomic apply
  transaction.

## Authority, persistence, and failure boundaries

Inbound Inbox owns durable work lifecycle. Sync Sidecar Store owns idempotency,
version cursors, tombstone state, conflict metadata, and base references.
SQLite owns the transactional apply bookkeeping; the existing JSON files own
conflict policy and pending review entries. A domain adapter owns the canonical
write. The apply coordinator owns the order between those authorities.

Database-open or transaction failure prevents apply completion from being
acknowledged. Domain write failure leaves bookkeeping uncommitted. A later
duplicate operation is skipped only when its applied record is durable.
Corrupt or missing operational state invokes repair; it does not authorize
guessing from file absence.

## Consequences

- Ready work, idempotency, cursors, and repair diagnostics scale with indexed
  rows instead of whole-file JSON size.
- Crash recovery retains pending work and safely retries a canonical write that
  may already have completed.
- Tombstones and cursors remain available after canonical deletion.
- Repair scanning can reconcile in bounded batches.
- SQLite schema migration becomes part of the sync operational contract.

## Rejected alternatives

- Whole-file `inbound_inbox.json`, `applied_ops.json`, and `cursors.json`.
- One SQLite database per inbox and sidecar concern.
- Extending iroh-docs `docs.redb` with application tables.
- Marking an operation applied before the domain write.
- Treating canonical file absence as a tombstone.
- Keeping unavailable content in a hot retry loop.

## Implementation and verification

- `crates/rua-sync/src/sync_state.rs`
- `crates/rua-sync/src/inbound_inbox.rs`
- `crates/rua-sync/src/sidecar.rs`
- `crates/rua-sync/src/merge_store.rs`
- `crates/rua-server/src/sync_inbound_apply.rs`
- `crates/rua-sync/tests/sync_foundation/inbound_inbox.rs`
- `crates/rua-sync/tests/sync_foundation/sidecar_cursor.rs`
- `crates/rua-sync/tests/sync_foundation/sidecar_snapshot.rs`
- `crates/rua-server/tests/contract_apply_dispatch/`

Verification covers the SQLite schema, ordered ready work, retry and dormant
transitions, atomic apply bookkeeping, duplicate operations, cursors, snapshots,
and repair metadata. It does not prove that conflict policy or pending conflict
state participates in the same database or transaction.

## Related decisions

- ADR-0038 defines SyncChange identity and HLC ordering.
- ADR-0039 defines adapter success before sidecar advancement.
- ADR-0042 defines incremental recording, draining, and Repair Scan.
- ADR-0046 relies on durable tombstone knowledge for Chat deletion.
