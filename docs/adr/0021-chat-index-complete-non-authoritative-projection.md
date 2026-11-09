# ADR 0021: Chat Index as a complete non-authoritative projection

**Status:** Proposed

## Context

Each materialized Chat has a small authoritative Session File, while each Agent
or Project Workspace also needs efficient Chat listing, filtering, sorting,
pagination, and search. Opening one Session File for every list row creates N+1
filesystem I/O and makes database pagination depend on values fetched after the
query.

Conversely, making SQLite the only authority would put Chat-owned metadata in a
repairable local database and make deletion or rebuild capable of losing durable
state. Stable list data and transient runtime activity also have different
lifetimes and must not be forced into one persistence model.

The current implementation uses a scoped `chat-index.sqlite` and projects Chat
metadata through the session and server Chat Index modules.

## Decision

### D1 - Chat Index is complete for durable list reads

Every durable field needed to render, filter, search, sort, or paginate a Chat
list is present in Chat Index. Normal Agent-Scoped and Project-Scoped list
requests return these fields from one scoped database query.

A list request must not open Session Files, Message Log chunks, Turn Indexes,
Summary Checkpoints, or History Replacement files to complete ordinary rows.
If a required list field is absent, the defect is in schema or projection
maintenance and must be corrected there.

### D2 - Chat-owned domain state remains authoritative

Chat-owned domain stores remain authoritative: Session File owns typed history
metadata and projection pointers; `running_config.json` owns complete Chat
Running Config; append-only Turn facts own Turn-stable Agent/model/reasoning;
and Conversation Authority owns placement/epoch. Chat Index is a denormalized,
repairable read model. Removing or rebuilding the Index must not remove any of
those facts.

A missing Index row is not proof that a Chat does not exist. A present Index row
is not sufficient authority to execute, route, or mutate a Chat whose Session
File is absent or invalid.

### D3 - Index scope follows Workspace ownership

Each Agent or Project Workspace has its own:

```text
<workspace-root>/chat-index.sqlite
```

Rows are keyed within that scope and correspond to materialized Chats owned by
the same Workspace. The row primary key can therefore be the Chat ID, but the
database path plus that ID form the scoped identity; callers must not combine
rows from different Workspace databases as if the bare ID were global.
Cross-scope listing combines explicit scoped queries at the application layer.

Lazy Chats have no row until materialization. Deleted Chats are removed through
the Chat deletion boundary, not by treating list omission as deletion.

### D4 - Stable and transient list fields remain separate

Stable fields such as title, channel, activity time, pin order, tag, and preview
are projected into Chat Index. Runtime observations such as Active Turn phase,
queue depth, or live connectivity remain owned by runtime authorities.

A list endpoint may merge a scoped runtime snapshot into Index rows. It must not
persist transient activity merely to make the response uniform, and it must not
perform one runtime or Session File lookup per row when a batch projection is
available.

### D5 - Authoritative commit precedes projection update

For a durable list-visible mutation, the writer:

1. validates and commits the authoritative Session File change;
2. upserts the corresponding Chat Index row;
3. publishes the stable post-commit synchronization fact;
4. reports success after the required local projection update completes.

The Session File and SQLite writes are not one cross-store atomic transaction.
An interruption may leave a missing or stale row, which repair reconstructs
from authoritative state.

### D6 - Repair is explicit and deterministic

Startup or maintenance repair scans authoritative Session Files and rebuilds
missing or stale rows. Normal list reads do not repair inline and do not hide
projection bugs through Session File read-through.

Schema evolution uses idempotent migrations and deterministic defaults. After
migration, normal code uses one current schema rather than permanent old/new
query branches.

### D7 - Synchronization replicates facts, not SQLite files

Stable Chat metadata synchronizes as Conversation or Chat facts. A receiving
Device applies those facts to its own Session File and local Chat Index through
the same projection boundary.

The `chat-index.sqlite` file is not copied between Devices and is never treated
as a portable database authority.

## Hard invariants

- Chat Index contains every durable field required by normal list queries.
- Chat-owned domain stores remain authoritative; Chat Index is not a control
  plane authority.
- Normal list reads do not open per-Chat durable files.
- Lazy Chats are not listed as materialized Chats.
- Transient runtime state is not authoritative Index data.
- Index loss or corruption cannot delete Chat-owned facts.
- Repair flows from Session Files to Index, never the reverse.
- Synchronization never transfers the SQLite database itself.

## Authority, persistence, concurrency, and failure boundaries

**Authority.** Session, Running Config, Turn and Conversation Authority stores
decide their respective durable facts. Chat Index may answer reads but cannot
authorize execution, placement, or mutation.

**Persistence.** Index updates occur after authoritative commits. Migrations and
rebuilds are local and idempotent. Legacy denormalized columns may exist, but
runtime routing must not read them as current authority.

**Concurrency.** Writers serialize a Chat mutation through its authoritative
service, then upsert the row. List queries observe a coherent SQLite snapshot;
they may temporarily see an older row after a crash but never synthesize a new
authoritative value.

**Failure.** A projection failure is reported and repairable. The Session File
commit remains valid. A missing Session File fails execution and routing closed;
an orphan Index row may be removed during repair.

## Consequences

- List latency scales with the paginated query rather than returned Chat count.
- New durable list features must update Session schema, Index schema,
  projection code, synchronization apply, and rebuild tests together.
- The Index intentionally duplicates stable metadata to match read patterns.
- Runtime overlays require their own batch snapshot contract.
- Projection health becomes observable instead of hidden by fallback reads.

## Rejected alternatives

- Open every Session File after querying the Index.
- Make Chat Index the sole metadata authority.
- Store Active Turn and Live Activity as durable Index truth.
- Copy SQLite databases through Workspace Sync.
- Treat a missing row as proof of Chat deletion.
- Keep `chats.json` as a fallback or mirror.
- Maintain permanent parallel schema implementations.

## Implementation and verification

Current implementation anchors include:

- `crates/rua-session/src/chat_index.rs`;
- `crates/rua-server/src/services/chat_index.rs`;
- `crates/rua-server/src/services/chat_metadata_store.rs`;
- `crates/rua-server/src/endpoints/chats.rs`;
- `crates/rua-server/src/services/chat_storage.rs`;
- `crates/rua-sync/src/chat_adapter.rs`.

### Current implementation gaps

The current code implements scoped SQLite CRUD, paginated list reads, pin/order
and tag filtering, and idempotent schema migration, including removal of the
legacy duplicate `session_id` column. It does not yet provide the complete
proposed projection:

- `crates/rua-session/src/chat_index.rs::ChatIndexRecord` has no dedicated
  preview field even though `crates/rua-session/src/store.rs::ChatMetadata`
  persists `preview`;
- no explicit startup or maintenance scan rebuilds missing or stale Index rows
  from Session Files;
- `crates/rua-server/src/endpoints/chats.rs::update_pin_order_core` writes
  `pin_order` directly to SQLite and publishes the resulting Index row, while
  the authoritative `ChatMetadata` schema persists `pinned` but has no
  `pin_order`. Index loss therefore loses the ordering fact and D2/D5 are not
  conformant for batch reorder.

Because completeness, repair, and recoverable pin ordering are central to this
decision, the record remains Proposed until these gaps are closed. They are not
permission for normal list reads to fall
back to per-Chat Session File reads. Existing
`crates/rua-session/tests/all/chat_index.rs` cases verify SQLite CRUD,
filtering, and ordering, but do not prove preview projection or recovery of
preview and pin order after deleting or rebuilding the Index.

Verification must prove one-query durable list reads, complete row projection
including preview, authoritative persistence of pin order, rebuild from Session
Files, idempotent schema migration, scope isolation, and separation of transient
runtime overlays. Existing
coverage includes
`crates/rua-session/tests/all/chat_index.rs`,
`crates/rua-server/tests/infra/chat_metadata_store.rs`, and
`crates/rua-server/tests/endpoints/chats_endpoints.rs`.

## Related decisions

- ADR 0018: Chat Address and Lazy Chat.
- ADR 0019: Project-Scoped Chat storage ownership.
- ADR 0020: Session File and Message Log authority.
- ADR 0022: Conversation Authority placement.
- ADR 0025: Runtime Notes as Conversation State.
- ADR 0028: Chat Running Config persistence.
