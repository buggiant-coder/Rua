# ADR-0042: Incremental apply and Repair Scan

> **Status:** Accepted

## Context

iroh-docs reports which replica entries were inserted or updated. Enumerating
the complete replica after every `SyncFinished` makes the cost of one small
change grow with total history and masks event-processing defects. Conversely,
trusting the live event stream forever leaves no recovery path for missed
events, a rebuilt subscription, or a newly created sidecar database.

Transport completion and canonical domain visibility are separate stages. The
normal path should process only changed records, while a bounded full scan
provides anti-entropy and explicit local catch-up.

## Decision

### D1 - Remote inserts drive incremental apply

Each remote insert event records only its `(author, key, content_hash)` in the
SQLite Inbound Inbox. The apply runtime drains ready rows, reads and decrypts
their SyncChange values, routes them through SyncRegistry, and commits sidecar
state after domain apply.

No complete replica enumeration is performed for an ordinary insert.

### D2 - SyncFinished is transport state

`SyncFinished` records the end of a transport attempt and may request another
drain for content that became available. It does not itself prove domain apply
completion and does not trigger an unconditional Repair Scan.

### D3 - Repair Scan is bounded anti-entropy

Repair Scan enumerates current replica entries once and reconciles them against
applied operations and Inbox rows in bounded batches. It records missing work,
skips already applied entries, persists scan diagnostics, and then drains ready
inbound work.

Repair Scan is required when:

- the SQLite state has never completed a scan;
- the configured 24-hour safety interval is due and power policy permits;
- the replica event subscription reports a discontinuity or is rebuilt;
- a user explicitly requests repair.

The schedule is elapsed-time based, not tied to local midnight.

### D4 - Manual Sync now composes transport and local completion

The user-triggered Sync action selects one best eligible peer and attempts
exactly one Sync Cycle. After a successful transport cycle it runs a local
Repair Scan and waits for this Device's ready Inbox work to drain. If transport
fails, local completion is reported as not started.

It does not fan out to another peer after failure and does not wait for another
Device's canonical files. Success means this transport attempt and local apply
completed, not that the entire network converged.

### D5 - Automatic and targeted cycles remain transport operations

Periodic, write-triggered, app-resume, and targeted Sync Cycles do not inherit
the manual action's unconditional Repair Scan. They rely on incremental events,
content-ready drains, and the independent due-scan policy.

### D6 - Full Export remains outbound repair

Repair Scan recovers inbound visibility from current replica entries. It never
reconstructs or republishes missing local outbound records. Explicit Full
Export is the separate outbound recovery operation.

### D7 - Diagnostics follow one operation across stages

`SyncChange.op_id` correlates publication, transport arrival, inbox recording,
content availability, decrypt/read, domain apply, and canonical visibility.
Per-Device durations use monotonic clocks; cross-Device wall times are
diagnostic only.

Diagnostics may expose opaque operation ids, timing, byte counts, route class,
and sanitized failure categories. They must not expose payloads, secrets,
canonical paths, or plaintext record identities.

## Hard invariants

- Ordinary inbound work is proportional to newly observed entries.
- `SyncFinished` never implies canonical apply completion.
- Repair Scan is not run after every Sync Cycle.
- Repair Scan reconciles in bounded batches and then drains ready work.
- Manual Sync selects one peer and makes only local completion claims.
- Manual Sync does not report Repair Scan or Inbox completion after its
  transport attempt fails.
- Automatic cycles do not silently become full scans.
- Full Export and Repair Scan remain opposite-direction recovery operations.
- Sidecar cursors advance only after successful domain apply.
- Diagnostics preserve content confidentiality.

## Authority, persistence, and failure boundaries

iroh events are authoritative only for observed transport changes. Inbound
Inbox owns durable pending work. Sync Sidecar Store owns applied state and scan
timing. Domain adapters own canonical visibility. The manual endpoint owns the
composition of one transport attempt with local repair and drain.

A failed transport attempt does not discard existing Inbox work. A failed scan
retains its prior successful timestamp so it remains due. A retryable or
dormant item does not block unrelated ready rows. Apply failure is reported as
local completion failure without claiming that the peer transport failed.

## Consequences

- Small updates do not pay full-history scan cost.
- Missed events remain recoverable within a bounded interval.
- User-visible Sync now has precise local semantics.
- Operational metrics can identify publication, transport, content, or apply
  bottlenecks separately.
- Repair behavior must respect mobile lifecycle and power policy.

## Rejected alternatives

- Full replica scan on every `SyncFinished`.
- Removing Repair Scan and trusting events permanently.
- Exact-midnight repair scheduling.
- Treating transport completion as end-to-end success.
- Retrying every peer during one manual Sync action.
- Running Full Export as part of inbound repair.
- Adding a Device Session hint as a substitute for correct incremental apply.

## Implementation and verification

- `crates/rua-server/src/sync_inbound_apply.rs`
- `crates/rua-sync/src/liveactor_apply.rs`
- `crates/rua-sync/src/inbound_inbox.rs`
- `crates/rua-sync/src/stage_diagnostics.rs`
- `crates/rua-server/src/sync_cycle.rs`
- `crates/rua-server/src/endpoints/sync.rs`
- `crates/rua-sync/tests/sync_foundation/inbound_inbox.rs`
- `crates/rua-server/tests/contract_apply_dispatch/`
- `crates/rua-server/tests/endpoints/sync_cycle_peer_selection.rs`

Verification covers insert-only recording, no scan on SyncFinished, due-scan
rules, event-gap recovery, bounded reconciliation, content reactivation, manual
single-peer behavior, local drain completion, and stage timing.

## Related decisions

- ADR-0039 defines publication and domain apply.
- ADR-0040 defines Inbox and sidecar persistence.
- ADR-0041 defines the exceptional First Sync snapshot boundary.
- ADR-0043 constrains when mobile repair work may run.
