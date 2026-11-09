# ADR-0041: Bidirectional First Sync

> **Status:** Proposed

## Context

Pairing can join two Devices that both contain user-created portable state.
Choosing one side as authoritative would discard valid work. Starting normal
inbound apply before export is captured is also unsafe: remote records can
modify canonical files while adapters are reading them, causing received state
to be republished under the joining Device's author.

Fresh installations complicate the boundary because they contain automatic
seed content. Untouched packaged defaults are initialization, not user intent,
and should not compete with user-modified state from another Device.

## Decision

### D1 - First Sync is bidirectional

Both Devices contribute their locally authored portable state. Pairing does not
define an old-device-wins or joining-device-wins rule. Conflicts are resolved
by data shape. Records with an established shared base use the normal merge
rules; same-id records with no common base use the bootstrap rules below rather
than silently treating an empty string or empty object as a real ancestor.

### D2 - Each Device freezes pre-pair state

Before inbound changes from the selected network may modify canonical files,
each participating Device captures an immutable `FullExportSnapshot` of its
pre-pair state. Publication reads this frozen change set; it does not reread
canonical files after the runtime begins receiving peer data.

The same boundary applies when bridging two existing sync networks: each side
captures pre-merge state before moving to the winning namespace.

### D3 - First Sync reuses Full Export

First Sync invokes the standard domain adapter export catalog in bootstrap
scope. It does not introduce a second serializer, a pairing-specific snapshot
format, or a separate apply path.

Snapshot publication appends one SyncChange per logical record. Bounded
concurrency and priority grouping are execution details and do not pack records
into a shared entry.

### D4 - Seed Manifest filters initialization defaults

Each adapter applies the Seed Manifest boundary while exporting:

- bytes still matching recorded seed output are omitted;
- user-modified seeded content is included as ordinary state;
- mixed projections omit only fields derived solely from untouched seed;
- absence from the manifest never proves that a file is seed-owned.

Explicit restore, reset, or builtin-update actions remain user intent and may
publish even when their resulting bytes equal packaged defaults.

### D5 - Pairing completes identity before First Sync transport

The selected NetworkSecret and Device List control state are durably installed
before the runtime is rebound. The frozen snapshot is then published into the
bound namespace and the readiness-aware First Sync cycle attempts the known
peers.

Pairing success and First Sync convergence are related but distinct. A Device
may be paired while peer transport or later local apply still requires retry.

### D6 - Inbound apply never enters the frozen snapshot

Changes received after the snapshot boundary are handled only by the normal
Inbound Inbox and domain adapters. They are not added to the local pre-pair
snapshot and are not echoed as fresh local authorship.

### D7 - No-common-base collisions preserve independent intent

First Sync distinguishes records present on only one side, byte-identical
records, and same-id records that differ without a common ancestor:

- one-sided records are unioned;
- byte-identical records are a no-op;
- Conversations union canonical Messages by identity;
- structured records merge independent fields and use HLC only for the same
  field;
- free text keeps both versions, with one valid live file and the other in a
  `.conflict` sidecar, regardless of the steady-state Conflict Policy.

First Sync never writes conflict markers into a live prompt, Skill, or other
Agent-readable file. After reconciliation, the merged result becomes the base
snapshot for steady-state three-way merge.

### D8 - Recovery remains explicit

A missed publication or interrupted First Sync can be repaired through Full
Export, Sync Cycle, and Repair Scan. Ordinary startup and every Sync Cycle do
not repeat bootstrap Full Export automatically.

## Current implementation gap

The frozen capture and bootstrap publication flow exists, but it does not yet
meet the success and merge guarantees above. Adapter export errors are logged
and skipped, append failures are logged, and `publish_full_export_snapshot`
still returns success after partial publication. Pairing can therefore report
success and continue into First Sync/Repair without a complete export.

The no-common-base bootstrap rule is also incomplete. Current adapters can use
steady-state three-way merge with an empty base, and current tests do not prove
the required same-id free-text keep-both behavior or construction of a complete
shared baseline after reconciliation. These are core First Sync properties, so
this record remains Proposed.

## Hard invariants

- First Sync preserves eligible user state from both Devices.
- Each exported view is frozen before inbound apply can mutate its source.
- Untouched seed is omitted; modified seed is treated as user state.
- Received peer state is never republished as part of pre-pair export.
- First Sync uses registered DomainSyncAdapters.
- One logical record produces one replica entry.
- Pairing identity persistence precedes bound-runtime publication.
- First Sync does not invent a second merge or apply algorithm.
- Same-id free text with no common base never silently discards one side.
- First Sync never writes conflict markers into a live Agent-readable file.
- A partial First Sync remains retryable and does not roll back pairing secrets.

## Authority, persistence, and failure boundaries

Pairing orchestration owns the order of secret installation, runtime rebind,
snapshot publication, and initial transport attempts. Domain adapters own the
snapshot content. Seed Manifest owns the untouched-seed predicate. Sync Inbox
and adapters own later inbound convergence.

The frozen snapshot is temporary orchestration state, not a durable product
database. Published changes become durable replica state. If capture fails,
pairing must not silently substitute a live reread. If peer readiness fails
after pairing, bounded First Sync retry may continue, while normal periodic or
foreground sync remains capable of convergence.

## Consequences

- Pairing no longer discards independently created portable state.
- Fresh Device defaults do not overwrite real user choices.
- Remote records cannot be echoed under the wrong Device author.
- First Sync performance depends on adapter export quality and bounded append.
- Seed ownership must be testable per domain and per entity.

## Rejected alternatives

- Declare one Device authoritative during pairing.
- Start inbound apply and then export live canonical files.
- Pack the whole bootstrap state into one SyncChange.
- Treat every file created by seed as permanently untouched.
- Run unconditional Full Export after each Sync Cycle.
- Add a pairing-only merge engine.

## Implementation and verification

- `crates/rua-server/src/services/sync_ops.rs`
- `crates/rua-server/src/endpoints/pairing.rs`
- `crates/rua-server/src/sync_migration.rs`
- `crates/rua-server/src/sync_cycle.rs`
- `libs/rua-core/src/seed_manifest.rs`
- `crates/rua-sync/tests/sync_foundation/seed_full_export.rs`
- `crates/rua-sync/tests/sync_foundation/merge_three_way.rs`
- `crates/rua-server/tests/contract_apply_dispatch/full_export.rs`
- `crates/rua-server/tests/endpoints/full_export_v4_regression.rs`

Existing tests cover selected frozen capture, seed filtering, adapter export,
and merge mechanics. They do not prove fail-closed export completion, the
same-id free-text keep-both rule, or a complete common baseline. Warning-only
capture/append failures must not be cited as successful First Sync coverage.

## Related decisions

- ADR-0036 defines pairing identity and membership.
- ADR-0039 defines Full Export through domain adapters.
- ADR-0040 defines durable inbound bookkeeping after the boundary.
- ADR-0042 defines normal incremental apply and repair.
