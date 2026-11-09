# ADR-0045: Structured Memory Aggregate

> **Status:** Accepted

## Context

Long-term Memory must be searchable, attributable, synchronizable, and distinct
from Conversation history and context-window management. A Markdown memory file
cannot preserve stable identities, lifecycle, device-local resource context,
relations, visibility, or derived search state. A single flat database row is
also too shallow for evidence, terms, resources, and pinned context.

The runtime already exposes explicit Memory tools and a `MemoryManager`
boundary. The durable model must make that manager the only authority while
allowing derived indexes and multi-device replication to remain replaceable.

## Decision

### D1 - Structured Memory is the only durable Memory model

Rua does not maintain `MEMORY.md`, a directory of Memory Markdown files, or a
contract that combines Memory with context compaction. Conversation State,
Runtime Notes, Context Projection, and Task state remain separate domains.

All long-term Memory reads and writes go through `MemoryManager`.

### D2 - Memory is an Aggregate

One Memory identity combines a canonical record with terms, source evidence,
resource descriptors, links, and relations. `memory.db` stores these parts in
normalized SQLite tables and reconstructs a `MemoryRecord` Aggregate for
callers. Visibility grants and pinned slots are separate canonical records that
reference Memory ids; they are not embedded members of `MemoryRecord`.

Stable Memory identity belongs to the record, not to a search document or
embedding vector.

### D3 - Agent writes express intent

`memory_write`, `memory_get`, `memory_search`, `memory_update`, and
`memory_delete` are thin adapters over MemoryManager. Runtime context supplies
the owning Agent, Chat/Turn evidence, Device identity, and host profile rather
than trusting the model to invent them.

Normal writes provide content and semantic terms. MemoryManager validates,
normalizes, timestamps, persists, and publishes the resulting Aggregate.

### D4 - Terms have explicit retrieval roles

Human-facing labels, search terms, derived terms, and resource tokens are stored
as typed terms with normalized forms and provenance. This separates display
grouping from retrieval aliases, translations, path fragments, and
maintenance-generated terms.

### D5 - Sources are soft Conversation links

Source evidence stores stable Chat, Turn, and Message references and optional
quotes. It does not copy complete transcripts or tool output into Memory.
Conversation Recall may resolve the surrounding evidence when it is locally
available.

### D6 - Device-local resources remain device-qualified

Local paths, directories, applications, model files, and similar resources are
represented by typed resource descriptors carrying the owning Device context.
A Memory synchronized to another Device must not imply that the referenced
path exists locally.

URLs and other portable resources may be marked portable; local resource
identity remains explicit.

### D7 - Lifecycle and relations preserve evolution

Records use explicit active, superseded, expired, and deleted states. Relations
such as supersedes, contradicts, supports, related, same-subject, and resource
alias remain separate structured records. AutoDream and MemoryManager may
maintain these facts; ordinary conversation storage does not become automatic
long-term Memory.

This lifecycle model defines canonical aggregate behavior and search filtering.
It does not by itself guarantee that every deletion can be reconstructed by
Full Export after a missed publication or loss of local sync sidecar state.

### D8 - Search projections are derived

Search combines lexical/FTS matches, normalized-term and resource expansion,
relation expansion, and optional vector retrieval. Exact matches receive
protection from being displaced by fuzzy-only results.

FTS documents, embeddings, vector databases, and graph projections are
rebuildable Device-local indexes. Missing derived state may reduce search
quality but does not change canonical Memory correctness.

### D9 - Slots are explicit pinned context

Memory slots and ordered slot items are canonical user-controlled context
selection. Prompt assembly may render enabled slots, preserving source and
Device context. Slots are not automatic summaries of every conversation.

### D10 - Sync carries canonical live Aggregate records

The `structured_memory` adapter publishes live Memory, relation, slot, and
visibility records. Inbound apply writes directly through structured store
operations and does not call Agent-facing tools or republish received changes.
Derived search artifacts are never synchronized.

Deletion replication and recovery are not acceptance evidence for this ADR.
Their required durable-tombstone and export behavior belongs to ADR-0046. The
current adapter excludes deleted Memory records from Full Export, relation and
slot deletion removes rows physically, and the publication hook does not cover
all of those tombstones.

## Hard invariants

- `MemoryManager` is the sole long-term Memory authority.
- Conversation history and Context Projection are not Memory.
- Canonical Memory has stable ids and explicit lifecycle state.
- Terms, sources, resources, links, relations, visibility, and slots remain
  structured; visibility and slots retain their own record identities.
- Local resource references preserve their owning Device context.
- Search indexes and embeddings are derived and rebuildable.
- Agent tools do not choose authoritative Agent or Device provenance.
- Inbound sync apply does not invoke Agent-facing Memory tools.
- Deleted and superseded records are excluded from normal search.
- No Markdown compatibility store is maintained.

## Authority, persistence, and failure boundaries

`MemoryManager` owns validation and application behavior.
`StructuredMemoryStore` owns canonical SQLite persistence. Search projection and
vector components own disposable indexes. `StructuredMemorySyncAdapter` owns
portable serialization and inbound merge, while the general sync layer owns
delivery and cursors.

A search-index failure must not lose a committed Memory record. Sync failure
must not roll back a local Memory write. Missing Conversation evidence remains
a soft unresolved reference. Invalid inbound identity or payload shape fails
before mutation and leaves the sync cursor unadvanced.

## Consequences

- Memory can be explained, filtered, related, and synchronized as structured
  user data.
- Retrieval remains useful without an embedding provider.
- Device-specific resources do not masquerade as local on another Device.
- AutoDream has explicit maintenance targets without owning raw conversations.
- Schema and migration tests are required as the Aggregate evolves.

## Rejected alternatives

- Markdown files as canonical long-term Memory.
- A combined MemoryStore and ContextManager contract.
- One opaque JSON row per Memory item.
- Copying full transcripts into Memory evidence.
- Letting Agents assign authoritative provenance or numeric ranking internals.
- Syncing embeddings, FTS tables, or graph caches.
- Automatic storage of every conversation as long-term Memory.

## Implementation and verification

- `crates/rua-agent/src/memory_manager.rs`
- `crates/rua-agent/src/memory_tools.rs`
- `crates/rua-memory/src/types.rs`
- `crates/rua-memory/src/structured_store.rs`
- `crates/rua-sync/src/structured_memory_adapter.rs`
- `libs/sqlite-vecstore/`
- `crates/rua-memory/tests/unit_memory/structured_store.rs`
- `crates/rua-sync/tests/sync_foundation/structured_memory_adapter.rs`
- `crates/rua-agent/tests/unit_memory/`

Verification covers Aggregate round trips, lifecycle filtering, terms and
resources, relation expansion, visibility, slots, index rebuild, tool context
provenance, live sync snapshots, and no inbound publication echo. It does not
establish recoverable Full Export tombstones for deleted Memory, relation, or
slot records.

## Related decisions

- ADR-0026 keeps Context Projection separate from long-term Memory.
- ADR-0039 defines the domain adapter and publication seam.
- ADR-0040 supplies sync idempotency and tombstone cursors.
- ADR-0046 owns durable deletion, tombstone, and recovery guarantees; this ADR
  does not claim that Structured Memory deletion currently satisfies them.
