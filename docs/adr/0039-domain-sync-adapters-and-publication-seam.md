# ADR-0039: Domain Sync Adapters and the publication seam

> **Status:** Accepted

## Context

Portable state is owned by many domains with different storage shapes and merge
rules. A central sync module that understands provider config, prompts, skills,
Memory, Chat, projects, and media would become a second implementation of every
domain. Endpoint-level `emit_*` calls and filesystem watchers are also unsafe:
they miss non-HTTP writers, lose semantic operation kinds, and cannot reliably
distinguish a local edit from inbound apply.

The architecture needs a narrow outbound seam after canonical persistence and a
symmetrical inbound seam before canonical mutation. Shared replication
mechanics should remain centralized while domain meaning stays with the domain.

## Decision

### D1 - DomainSyncAdapter is the inbound and export contract

Each syncable domain implements `DomainSyncAdapter`. The adapter declares one
stable domain id, exports current canonical records for Full Export, validates
incoming record kinds, applies domain merge rules, and writes canonical local
state.

The registry routes by domain. It does not interpret domain payloads or choose
domain-specific file paths.

### D2 - Outbound publication starts with a committed domain fact

A domain persistence module reports a typed committed fact only after its local
canonical write succeeds. The publication seam translates that fact into
record identity, operation kind, HLC, SyncChange payload, encrypted blob
context, and replica append.

HTTP endpoints, Console handlers, and other adapters must call domain services.
They must not own publication or construct raw SyncChange values as the normal
write path.

### D3 - Publication is a post-commit side channel

Local CRUD success does not depend on peer availability. Disabled sync,
enabled-without-network, a suspended runtime, or an unbound network causes
publication to skip or fail after the local commit. The local write is not
rolled back.

Unexpected publication failures are observable and repairable through Full
Export. The seam does not add a second per-write durable queue beside the
replica log.

### D4 - The publication seam owns shared sync mechanics

The seam owns Bound Sync Network lookup, HLC allocation, operation creation,
opaque replica append, encrypted inline/blob selection, stage diagnostics, and
write-coalesced Sync Trigger notification. Domain callers provide meaning, not
transport policy.

Trigger delivery is best-effort acceleration. A successful append remains
valid even if the immediate trigger is missed.

### D5 - Inbound routing is shared; application is domain-owned

Inbound infrastructure decrypts and validates a SyncChange, checks idempotency,
and routes it through `SyncRegistry`. The selected adapter validates record
identity and kind, merges against canonical local state and sidecar data, and
commits the domain write.

Only after adapter success may the sidecar advance the applied operation and
record cursor. Inbound apply must not emit a new outbound fact for the same
change.

### D6 - Full Export is the recovery publication path

Full Export asks every registered adapter for current canonical records and any
delete tombstones that adapter explicitly exports, then publishes one change per
logical record. It is used for First Sync, network merge, and explicit outbound
repair. It is not the normal mutation path and is not run after every inbound
apply or Sync Cycle. A locally persisted deletion record is recoverable through
Full Export only after its domain adapter includes that record in export.

Agent Prompt File Sync implements this recovery contract through its domain-owned
file store. Local file operations commit canonical paths, content, and versions
before publishing one committed batch. Rename includes both the old-name deletion
and the new-name write. HTTP handlers do not reconstruct these facts.

Per-file state under each workspace's `.prompt-file-state/` retains deletion
versions independently of Sync availability. A prepared deletion is effective
only when its file is absent; absence alone never creates a deletion. Prepared
live versions match both the installed file timestamp and content hash, retaining
the previous live version until atomic replacement succeeds. Rename prepares
both identities before its atomic move. This lets Full Export recover a crash
between persistence and publication without inventing a second outbound queue.

Local versions advance past stored versions even if the wall clock moves back.
Inbound writes retain their full HLC provenance, so re-export does not make an
old deletion newer. Apply compares deletions against canonical file state,
including offline recreation, rather than trusting an older Sidecar tombstone.
Local commits, export, and inbound apply share a workspace lock; inbound apply
uses the store without outbound publication. Recreated live records retain the
last effective deletion as an optional `deleted_before` HLC in both ordinary
publication and Full Export. Receivers persist this fence with the matching live
version and exclude pre-deletion bytes from merge, even when they never received
the deletion itself. This guarantee covers operations
through this store, not arbitrary external filesystem deletes. Global Prompt
Library uses the same durable file-commit mechanism with a separate root and
`.prompt-library-state/` record directory. Its `prompts` domain retains its own
record identities. Library writes, rename, delete, ZIP import, and explicit
builtin restoration publish committed facts from the Library module; HTTP
handlers do not reconstruct paths or scan the library after a mutation.

Bulk input is validated before committing, then applied one file at a time.
Successful files remain committed and publishable if a later file fails; the
failure identifies the stopped file and completed count. There is no whole-library
transaction or second outbound queue. Archive non-Markdown attachments remain
local files, preserving the existing Markdown-only synchronization scope.

Startup additive seeding remains local initialization. Explicit builtin restore
still only fills missing files, but those writes carry durable live versions and
are exported even when their bytes match a seed hash. Local edits, Full Export,
and inbound apply share the file store; inbound apply does not echo publication.
Deletion and recreation fences preserve provenance across re-export. Recovery
metadata lives outside the downloadable library and is never imported from a ZIP.

### D7 - Registration is an architectural obligation

A portable domain is complete only when its adapter is registered in the
shared export/apply catalog and its normal mutation path emits committed facts.
Contract tests enumerate this catalog so missing registration or publication
coverage is visible.

## Current implementation gap

The adapter registry and shared publication helper exist, but publication
ownership has not fully moved behind domain persistence services. Multiple HTTP
endpoints still call `publish_sync_fact` directly after their local writes.
Current catalog contract tests primarily enumerate a static adapter set and do
not prove that every mutation path enters publication through its domain-owned
service.

## Hard invariants

- Canonical domain persistence commits before outbound publication.
- Ordinary business callers do not construct raw SyncChange values.
- Filesystem events are never the publication authority.
- SyncRegistry routes by stable domain id and does not parse domain formats.
- Domain adapters own record validation, merge semantics, and canonical paths.
- Inbound apply never republishes the received change.
- Sidecar cursor advancement happens only after canonical apply succeeds.
- Publication failure never rolls back an already committed domain mutation.
- Full Export emits one change per logical record and does not imply that
  unregistered tombstone stores are covered.

## Authority, persistence, and failure boundaries

Domain modules are authoritative for canonical state and committed facts.
`sync_publication` is authoritative for turning those facts into encrypted
replica entries. `SyncRegistry` is authoritative only for adapter registration
and dispatch. Inbound Inbox and Sidecar Store own retry and idempotency state,
not business content.

If publication is unavailable, canonical local state remains valid and Full
Export can recover the omission. If an adapter rejects or fails an inbound
change, the inbox retains retry/discard state and the record cursor does not
advance. A missing adapter is a catalog defect, not permission for generic sync
code to write the domain's files.

## Consequences

- New domains integrate through one explicit, testable contract.
- Endpoints remain transport adapters rather than synchronization orchestrators.
- Local-first writes remain available during peer or runtime outages.
- Recovery is explicit without duplicating the replica journal.
- Domain-specific refresh of live managers remains close to domain apply code.

## Rejected alternatives

- A filesystem watcher as the primary outbound mechanism.
- Endpoint-specific sync helpers as the public architecture.
- A central merge dispatcher that knows every domain format.
- Rolling back local CRUD when publication fails.
- A durable publication queue parallel to the replica log.
- Running Full Export after every write, startup, or inbound apply.
- Letting adapters bypass the shared envelope, HLC, or sidecar mechanisms.

## Implementation and verification

- `crates/rua-sync/src/sync_registry.rs`
- `crates/rua-server/src/sync_publication.rs`
- `crates/rua-server/src/sync_domain_catalog.rs`
- `crates/rua-server/src/services/sync_ops.rs`
- domain adapters under `crates/rua-sync/src/`
- `crates/rua-sync/tests/sync_foundation/sync_registry.rs`
- `crates/rua-server/tests/contract_sync_domain_catalog.rs`
- `crates/rua-server/tests/contract_apply_dispatch/`
- `crates/rua-server/tests/endpoints/full_export_v4_regression.rs`

Existing verification covers registry uniqueness, registered adapter routing,
wrong-domain rejection, selected post-commit publication paths, disabled-sync
behavior, and no inbound echo. It does not establish domain-service ownership
for every endpoint mutation path.

## Related decisions

- ADR-0038 defines the operation, ordering, and encryption mechanics.
- ADR-0040 defines durable inbound and sidecar bookkeeping.
- ADR-0041 defines frozen Full Export during First Sync.
- ADR-0042 defines incremental delivery and Repair Scan.
- ADR-0045 applies this seam to Structured Memory.
