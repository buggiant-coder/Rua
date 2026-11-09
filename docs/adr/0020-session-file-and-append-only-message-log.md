# ADR 0020: Session File and append-only Message Log

**Status:** Accepted

## Context

A Chat needs both compact current metadata and a complete durable chronology.
Putting both concerns into one repeatedly rewritten document makes large Chats
expensive, weakens crash recovery, and encourages commands or compaction to
destroy history in order to change future model input.

The current session subsystem separates `session.json`, Message Log chunks,
Turn Index, Summary Checkpoints, History Replacement records, and disposable
projection data. This separation is a domain boundary: current Chat state and
append-only Conversation facts have different write and recovery behavior.

The historical name Session File does not introduce a second Session identity.
All files remain addressed by the Chat ID within the Workspace selected by the
complete Chat Address.

## Decision

### D1 - Session File is the small authoritative Chat header

The canonical path is:

```text
<workspace-root>/sessions/<chat-id>/session.json
```

The Session File owns current durable Chat metadata, control configuration,
history metadata, and projection pointers. This includes title, activity time,
pinned/archived booleans, preview, tag, Chat Policy, selected Agent,
model/thinking selection, Conversation Authority Placement/Epoch, ACP binding,
Harness configuration, Message Log and Turn Index heads, active checkpoint,
`projection_boundary`, and Context Snapshot. Its `extra` map preserves unknown
fields during schema evolution.

It is not the authority for every Chat-owned value. `pin_order`, list status,
and other stable list projection fields live in Chat Index; complete Chat
Running Config lives in `running_config.json`; and the Projection Exclusion Set
lives in `projection_exclusions.json`. Turn-stable Agent/model/reasoning
assignments are append-only Turn facts. Those seams are governed by ADR 0021,
ADR 0022, ADR 0026 and ADR 0028 rather than being folded into Session File
ownership.

Chat Index may contain denormalized copies for list display, but routing and
control readers use the Session File and reject a missing or conflicting
projection.

It does not contain the complete transcript, large Tool output, terminal
artifacts, or a second copy of Message Log records.

### D2 - Message Log is the canonical conversation chronology

Conversation Messages are appended to sequence-addressed JSONL chunks under the
Chat directory. Each record has a stable message ID, sequence, Turn identity,
role, typed content, timestamps, status, and operation linkage needed for exact
reconstruction.

Message Log order is authoritative. Display projections, model projections,
search, recall, synchronization, and repair consume this chronology; they do
not replace it with rendered text or frontend state.

### D3 - History is append-only

Once a Message Log record is committed, normal Chat commands do not delete,
rewrite, or reorder it. `/clear`, `/compact`, status commands, Skill commands,
Context Projection, Summary Checkpoints, and History Replacement change future
input selection or derived state, not the original chronology.

Projection Exclusions follow the same rule: they select committed non-user
Messages to omit from future model input by stable Message ID. They do not
delete, rewrite, hide from normal history, or resequence those Messages.

Corrections such as stop markers, interjection markers, Runtime Notes, and
selected response variants are represented through new records or authoritative
Turn metadata. They do not mutate an earlier message into a different fact.

### D4 - Turn Index is metadata over the log

Turn Index records Turn lifecycle and sequence relationships, including the
user message, response variants, selected variant, timing, and terminal status.
It accelerates Turn-oriented reads but does not contain an alternate transcript.

Message is the append unit. Tool-use/result closure is a derived safe boundary
for Context Projection, not a requirement to batch multiple messages into one
persistence operation.

### D5 - Header mutation and message append have explicit ordering

The Turn Writer appends complete Messages in observed order and updates the
corresponding Turn metadata. A submitted user boundary is durable before model
output. A completed assistant message is durable before requested Tool Results,
and all results required by a model call are durable before the next model call.

Terminal finalization verifies prior Message writes, appends any required
terminal marker, commits Turn lifecycle, and then publishes the terminal
transition. It does not replay the complete Turn into the log at completion.

### D6 - Durable derived state and disposable projections remain distinct

Summary Checkpoints and published History Replacement records are derived from
the Message Log, but they are durable replicated Conversation State with their
own integrity, reachability, and deletion rules. They are not disposable cache
entries and are not silently regenerated as a side effect of an ordinary read.

The Projection Exclusion Set is durable replicated Chat-owned control state
rather than generated history. It is stored separately from the Message Log so
changing model visibility never changes transcript truth.

Chat Index rows, search indexes, and Projection Cache entries are local
projections. Deleting or rebuilding those projections must not delete canonical
history or durable checkpoint/replacement records. A corrupt or missing local
projection is repaired from authoritative files rather than accepted as
evidence that the Chat or its messages do not exist.

## Hard invariants

- Session File and Message Log are distinct persistence responsibilities.
- Session File is authoritative for current Chat metadata and control
  configuration represented by `ChatSessionHeader`; Chat Index is never the
  fallback authority for placement, model, selected Agent, or policy.
- List ordering/status, Chat Running Config, and Projection Exclusions retain
  their dedicated stores.
- Message Log records are append-only after commit.
- Every persisted Message has stable identity and sequence ordering.
- Commands never erase history to alter future model context.
- Projection Exclusions reference stable Message identities and never alter
  user-visible history.
- Terminal finalization does not duplicate already-written Messages.
- A Tool Result follows its Tool Use identity without textual guessing.
- Derived projections cannot become the source of transcript truth.
- Durable Summary Checkpoints and History Replacement records are not
  disposable Projection Cache state.
- Session File remains small enough for atomic replacement.

## Authority, persistence, concurrency, and failure boundaries

**Authority.** Conversation Authority and its Turn Writer are the only normal
foreground writers for one Chat. Read models, frontends, providers, and sync
adapters cannot append or rewrite records independently.

**Persistence.** Session File and Projection Exclusion Set updates use atomic
replacement. Message Log uses append-only records and stable sequence
allocation. Turn Index and checkpoint pointers must never claim coverage beyond
durable log content. Projection Exclusions, published checkpoints, and History
Replacement records are durable siblings of the log; only explicitly
disposable indexes and caches are freely rebuildable.

**Concurrency.** One Turn Writer serializes writes for the Active Turn. Other
background producers submit typed records through the Conversation State
boundary and wait for a legal chronology point rather than opening the log
directly.

**Failure.** A crash may leave an orphan derived file or a stale projection,
but recovery must not expose a Session pointer to incomplete content. Streaming
output may be persisted incrementally as complete JSONL records carrying
`in_progress` status and later terminalized; a torn JSONL record is rejected or
truncated by storage recovery.

## Consequences

- Full history remains available for audit, UI, recall, branching, and repair.
- Model input can be compacted without destroying the user's transcript.
- Long Chats require chunking, indexes, and projection planning rather than one
  monolithic rewrite.
- Storage code must distinguish authoritative files from caches and read models.
- Disk usage grows with history and is managed by explicit retention or Chat
  deletion, not slash-command side effects.

## Rejected alternatives

- Keep the complete transcript inside `session.json`.
- Delete earlier records when `/clear` or `/compact` is used.
- Store command results only in the frontend.
- Rewrite Tool calls and results into one synthetic message after completion.
- Treat Turn Index as a second message store.
- Reconstruct durable ordering from timestamps or array position.
- Let projection caches serve as recovery authority.

## Implementation and verification

Current implementation anchors include:

- `crates/rua-session/src/store.rs` for Chat storage layout and writes;
- `crates/rua-session/src/store.rs::ProjectionExclusions` for durable
  model-context selection;
- `crates/rua-chat/src/lib.rs` for typed Chat settings and Session projection;
- `crates/rua-session/src/turn_writer.rs` for single-Turn persistence;
- `crates/rua-session/src/lib.rs` for session models and read paths;
- `crates/rua-server/src/services/chat_storage.rs` for server integration;
- `crates/rua-server/src/services/console_chat.rs` for Turn persistence;
- `crates/rua-session/src/runtime_note.rs` for typed appended notes.

Verification must cover monotonic sequences, append recovery, no message replay
at terminal commit, complete Tool call/result ordering, non-destructive clear,
compact and Projection Exclusion behavior, and projection rebuild from
authoritative records.
Relevant suites include
`crates/rua-session/tests/all/message_log_turn_index.rs`,
`crates/rua-session/tests/all/projection_read_path.rs`, and
`crates/rua-server/tests/infra/evergreen_console_persistence.rs`.

## Related decisions

- ADR 0018: Chat Address and materialization.
- ADR 0021: Chat Index projection.
- ADR 0023: Active Turn, Turn Writer, and input queue.
- ADR 0025: Typed durable Runtime Notes.
- ADR 0026: Context Projection ownership.
- ADR 0027: Durable History Replacement.
