# ADR-0046: Chat deletion, tombstones, and storage accounting

> **Status:** Proposed

## Context

A Chat owns durable Conversation files and Task state, references Media, and
produces Device-local Tool Call Logs. Deleting only the visible directory is
not sufficient: another paired Device can later reintroduce synchronized
records, shared Media may still belong to retained Chats, and content-addressed
Sync blobs may be referenced by unrelated records.

Users also need useful size information before deletion. Per-Chat attribution
and physically reclaimable bytes are different when Media is shared or Task
storage is already contained in the session directory. Deletion must be
portable, terminal, idempotent, and independent of the current Conversation
Authority or Project Resource Device.

## Decision

### D1 - Storage operations are Chat-addressed

Storage APIs accept complete Agent-scoped or Project-scoped Chat Addresses.
They never accept arbitrary filesystem paths or individual Message chunks as
deletion units. Chat Index metadata supports listing and candidate selection
without opening every transcript.

### D2 - Accounting separates attribution from reclaimability

Under ADR-0053, a source/collaboration relationship is provenance, not deletion
ownership. Deleting the source never implicitly deletes or stops its independent
collaboration Chats or their Tasks. Retained Chats remain discoverable in the
collaboration view with unavailable-source metadata and last effective config.
Late results cannot recreate or wake a deleted source. Explicit deletion of a
collaboration uses its own full Chat Address and ordinary cleanup rules; Project
resource lifecycle still applies independently.

One bounded scan derives session bytes, the Task subset, Device-local Tool Call
Log bytes, referenced Media bytes, attributed bytes, and expected reclaimable
bytes.

Task bytes explain the session composition and are not added twice. Shared
Media may be attributed to several Chats but is reclaimable only when no
retained Chat in the same owner Scope references it. Batch preview de-duplicates
physical bytes across the selected set.

### D3 - Pinned Chats cannot be permanently deleted

Every deletion entry point re-reads the current local synchronized Chat Index
immediately before committing deletion. A pinned Chat is rejected. The user
must explicitly unpin it; no force flag bypasses this invariant.

Frontend disabled state is advisory. The backend revalidation is the authority.

### D4 - Deletion begins with a durable Chat Delete Intent

Any paired Device may create a terminal Delete Intent for a locally known Chat.
The local intent record contains portable Chat identity, a unique intent id,
and deletion time. Replication publication separately assigns the SyncChange
`op_id` and HLC. Neither representation contains a workspace path, Tool log key,
Media path, or target-Device cleanup plan.

The initiating Device commits the intent before destructive local cleanup.
Failure to make the intent durable fails closed and leaves the Chat intact.

### D5 - Delete wins over ordinary Chat state

A committed Chat Delete Intent permanently tombstones that Chat identity and
wins over concurrent or later-arriving title, pin, archive, history, checkpoint,
runtime-note, or Media-reference updates. Inbound apply must not hydrate a
tombstoned Chat.

There is no implicit undelete. A future recovery feature would create a new
Chat identity.

### D6 - Every Device performs the same local apply

Local Chat Deletion Apply stops or interrupts local Chat-owned activity, removes
Conversation and Task storage, updates Chat Index, derives and removes local
Tool Call Logs, and conditionally removes unreferenced Media.

The operation is idempotent. Missing derivatives are success, and duplicate
Delete Intents do not recreate work or fail the tombstone.

### D7 - Media removal is reference-safe

Before publishing or applying Media deletion, the Device evaluates references
from retained Chats in the same owner Scope. Uncertain or malformed reference
state preserves the Media. Batch deletion shares one Scope-level reference
plan instead of rescanning the Scope for every Chat.

### D8 - Sync Blob reclamation remains garbage collection

Storage cleanup never deletes an encrypted content-addressed blob directly by
hash. Chat and Media tombstones plus removal of local references allow the
existing Sync Blob GC to reclaim unprotected content later.

### D9 - Commit and cleanup outcomes are distinct

Success means the initiating Device durably recorded the Delete Intent,
successfully published the required tombstones when Sync is enabled and bound,
and applied the primary local Chat deletion. It does not mean every offline
Device has erased bytes. If derivative cleanup fails after the intent commits,
deletion remains committed and retry must not revive the Chat; current endpoint
responses do not claim remote physical-erasure completion.

## Current implementation gap

The accounting and primary local cleanup paths exist, but the permanent
tombstone protocol is incomplete. The single-Chat Agent and Project endpoints
persist `ChatDeleteIntent` before cleanup; the Storage/batch path publishes
delete facts and cleans up without using the coordinator's intent store.
`SkippedDisabled` can still lead to local-only deletion.

The intent store is a separate JSON sidecar, and `ChatIndexSyncAdapter` Full
Export enumerates only Chats still present in Chat Index. It does not export
delete intents for already removed Chats. Losing the sidecar or missing the
original publication can therefore allow stale replica state to reintroduce a
Chat. Until every deletion path uses one coordinator and permanent tombstones
are included in Full Export/rebuild, the terminal-delete guarantees remain
Proposed.

## Hard invariants

- Deletion is keyed by complete Chat Address, never raw path.
- Pinned state is revalidated by every backend entry point.
- Delete Intent is durable before local destructive cleanup.
- When Sync is enabled, an unbound or failed tombstone publication blocks
  destructive cleanup; disabled Sync currently permits local-only deletion.
- Delete wins over every ordinary fact for the same Chat identity.
- Tombstone knowledge survives removal of canonical Chat files.
- Conversation Authority and Project Resource Binding do not route deletion.
- Local apply is idempotent on every Device.
- Shared Media is preserved while any retained Chat references it.
- Sync blobs are reclaimed only by reference-aware GC.
- Per-Chat attributed bytes are not presented as independently reclaimable.

## Authority, persistence, and failure boundaries

Chat Storage Accounting owns size and reference analysis but performs no
deletion. Chat Delete Intent storage and sync publication own the terminal
portable fact. Chat Deletion Coordinator orders pin validation, intent commit,
and Local Chat Deletion Apply. Each Device owns cleanup of its local Tool logs
and physical files.

A failed intent commit leaves canonical state untouched. Failure after intent
commit cannot roll deletion back; cleanup remains idempotently repeatable.
Offline Devices apply the tombstone when Sync reaches them. Incomplete Media
proof preserves bytes, and Blob GC may finish after the API returns.

## Consequences

- Deleted Chats cannot be resurrected by stale replicas.
- A Device can delete without waiting for another Device or authority.
- Storage estimates remain honest in the presence of shared data.
- Logical deletion and physical reclamation may complete at different times.
- Agent and Project Chats share one deletion protocol.

## Rejected alternatives

- Local filesystem deletion without a replicated tombstone.
- Requiring Conversation Authority or Project Resource Device availability.
- Routing delete through the Device Command Bus.
- Allowing force deletion of pinned Chats.
- Letting a later ordinary record clear the tombstone.
- Deleting individual Conversation subfiles.
- Counting shared Media as fully reclaimable for every Chat.
- Force-deleting Sync blob hashes from Storage cleanup.

## Implementation and verification

- `crates/rua-server/src/services/chat_storage.rs`
- `crates/rua-server/src/services/chat_deletion.rs`
- `crates/rua-server/src/endpoints/storage.rs`
- `crates/rua-server/src/services/chat_index.rs`
- `crates/rua-session/src/chat_index.rs`
- `crates/rua-sync/src/chat_adapter.rs`
- `crates/rua-server/tests/unit_server/chat_deletion.rs`
- `crates/rua-server/tests/endpoints/storage_chats.rs`
- `crates/rua-sync/tests/sync_foundation/chat_adapter.rs`
- `crates/rua-sync/tests/sync_foundation/evergreen_chat_adapter.rs`

Existing tests cover pinned rejection, intent round trips, selected local
cleanup idempotency, accounting, shared Media preservation, and inbound
tombstone precedence. They do not prove that every deletion entry point uses
the coordinator, that tombstones survive sidecar loss, or that Full Export can
repair a missed deletion publication.

## Related decisions

- ADR-0018 defines stable Chat Address and Scope identity.
- ADR-0020 defines durable Session and Message Log storage.
- ADR-0022 defines Conversation Authority without making it deletion authority.
- ADR-0030 defines Chat Media ownership and transfer boundaries.
- ADR-0039 defines tombstone publication through domain facts.
- ADR-0040 preserves applied tombstone state and cursors.
- ADR-0044 prevents stale hydration from becoming a second writer.
