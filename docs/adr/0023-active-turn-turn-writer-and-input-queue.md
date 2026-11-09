# ADR 0023: Active Turn, Turn Writer, and Conversation Input Queue

**Status:** Accepted

## Context

Earlier runtime shapes allowed stream presence, frontend state, persisted Turn
status, and a separate run snapshot to disagree about whether a Chat was busy.
That creates a dangerous interval in which the UI appears terminal while the
backend still queues input behind an execution object that has not handed off.

The product model needs one foreground execution identity and one owner for
input acceptance, persistence, terminality, and queue advancement. Runtime
handles are necessary, but they must not become a second business lifecycle.

The current server exposes `ConversationActiveTurn`, queue snapshots, allowed
controls, and a per-Chat transition gate through Conversation Authority.

## Decision

### Result-driven Turns and collaboration requests (ADR-0053 amendment)

Collaboration requests reuse the existing acceptance, queue, claim, and
interjection contracts below. They carry stable source/request association but
do not create Task Attempts or a second input queue. Runtime-only queue loss on
restart must settle accepted calls as interrupted using durable correlation,
never silently abandon or automatically replay them.

Returned results are different: accept them through a typed result-observation
entry, not accept_submission or a fabricated user input. An active Turn receives
them at a legal model-call boundary; an idle Chat can claim one runtime-triggered
Turn, with explicit result provenance and no fake user-message ID/sequence.
Single Active Turn and Turn Writer rules still apply. Yield affects only the
calling wait, never target lifetime; direct/deferred delivery shares one result
identity. Current Turn-index required user sequence and Task Watch input intake
are implementation gaps to fix, not exceptions to this contract.

### D1 - Active Turn is the only foreground execution identity

Each accepted input claimed for execution creates one durable `turn_id`. That
Conversation Turn is the Active Turn until it reaches a durable terminal state.

There is no public or durable `run_id`, Run Snapshot, or Run Authority beside
the Turn. Provider streams, cancellation tokens, Tool handles, approval waits,
and Driver sessions are Active Turn Execution resources keyed by `turn_id`.

### D2 - At most one Active Turn exists per Chat Address

Conversation Authority owns zero or one non-terminal Active Turn. Its Control
View may expose the Turn ID, phase, authority Device and Epoch, Driver, canonical
user message ID, queue count, and allowed controls.

Transient phases such as preparing, streaming, waiting for approval, running a
Tool, stopping, and committing refine the Active Turn. They do not create new
durable identities or authorize another writer.

### D3 - Accepted input enters one authority-owned queue

Every queue-capable interactive submit receives a Submission ID. Conversation
Authority deduplicates the submission and creates an authority `input_id` when
accepted.

If the Chat is idle, the authority claims the input and assigns its `turn_id`.
If a Turn is active, the input enters the runtime-only FIFO Conversation Input
Queue. A queued input has no `turn_id` until it is claimed.

Queue order, edit/delete eligibility, pause state, Send Now, and drain behavior
belong to Conversation Authority. Frontend optimistic state and transport
pending markers are projections only.

The Chat CLI adapter uses the same acceptance and queue contract. If the Chat is
busy, its submission may enter the authority queue; the CLI keeps one SSE
response open and filters live activity until that submission is claimed and
its assigned Turn reaches terminal state. CLI does not create a second queue,
pre-assign a queued `turn_id`, or construct a second runtime.

### D4 - One Turn has one Turn Writer

All durable user, assistant, thinking, Tool, approval-audit, Runtime Note,
variant, usage, and terminal updates for an Active Turn pass through one Turn
Writer keyed by `turn_id`.

The writer appends each complete Message once in observed order. It does not
guess Turn identity from text, timestamp, array position, or "last running"
status. Display transports may observe events but cannot replay the Turn into
persistence at terminal time.

### D5 - Model-call boundaries are durable before continuation

The user boundary is committed before the first model output. Each complete
assistant Message and each complete Tool Result is committed before the next
model call that depends on it.

A Tool Use and its results need not be one storage batch. Context Projection
derives whether a boundary is closed from Tool Call IDs. In-progress Turns may
therefore publish coherent durable checkpoints without being terminal.

### D6 - Terminal commit and queue handoff are one transition

Conversation Authority finalizes a Turn in this order:

1. persist all completed output, including bounded interrupted partial output;
2. append any required stop or interjection marker;
3. commit terminal lifecycle and usage metadata;
4. under the per-Chat transition gate, claim the next eligible queue entry or
   clear the Active Turn;
5. publish the terminal and resulting Control View transition;
6. run post-commit sync, notification, and cleanup work.

When a viewer observes the authoritative terminal event, the authority has
already installed the next Turn or is ready to accept new input as idle.

### D7 - Queue state is intentionally runtime-only

Unclaimed queue entries are not Conversation State and are not synchronized.
Restart or authority replacement clears them. Only a claimed input becomes a
durable Conversation Turn.

This boundary prevents delayed input from executing unexpectedly after a
process restart or authority move. Durable retry belongs to an explicit Task or
client resubmission protocol, not the Conversation Input Queue.

## Hard invariants

- A Chat has at most one non-terminal Active Turn.
- `turn_id` is the only foreground execution identity.
- Queued inputs do not receive Turn IDs before claim.
- One Turn Writer owns all durable writes for a Turn.
- Complete model-call dependencies are durable before the next call.
- Terminal publication follows terminal persistence and queue handoff.
- Queue state is runtime-only and authority-owned.
- Chat CLI uses the canonical Conversation Input Queue and waits for its own
  accepted submission.
- Frontends and streams cannot declare a Turn terminal.

## Authority, persistence, concurrency, and failure boundaries

**Authority.** Conversation Authority accepts input, owns the queue, claims
Turns, invokes the Turn Writer, and publishes Control View transitions. Drivers
produce candidate events but do not own terminality.

**Persistence.** Claimed inputs, Turn metadata, Messages, variants, Runtime
Notes, and terminal facts are durable. Queue entries and execution handles are
not. Accepted-submission records may be retained only as needed for bounded
idempotency.

**Concurrency.** A per-Chat transition gate serializes submit, queue mutation,
Stop, interjection, terminal commit, and next-Turn claim. Tool work may execute
concurrently inside one Turn, but its writes converge through the same writer.

**Failure.** Driver EOF, provider error, or stream loss is only a terminal
candidate. The authority persists an explicit outcome before publication. On
restart, durable in-progress work is reconciled as interrupted; queued input is
not replayed.

## Consequences

- Busy/idle state is derived from Conversation Authority, not producer presence.
- CLI, WebUI, remote controllers, and Task notifications share one acceptance
  policy.
- Queue operations require accepted `input_id` values and current epoch.
- Runtime cleanup cannot delay the public handoff to the next Turn.
- Tests can assert one lifecycle instead of reconciling Run and Turn objects.

## Rejected alternatives

- Keep a separate Conversation Run identity.
- Decide busy state from an active stream registry.
- Assign Turn IDs while inputs are still queued.
- Let endpoints append user-only Turns before authority claim.
- Replay all streamed output into storage at terminal time.
- Persist the input queue for automatic restart replay.
- Allow each Driver or transport to finalize independently.

## Implementation and verification

Current implementation anchors include:

- `crates/rua-server/src/services/conversation_authority.rs`;
- `crates/rua-server/src/services/conversation_input_intake.rs`;
- `crates/rua-server/src/services/console_chat.rs`;
- `crates/rua-session/src/turn_writer.rs`;
- `crates/rua-server/src/services/active_stream_broadcast.rs`;
- `console/src/api/types/chat.ts`.

Verification must cover duplicate submissions, idle start, busy enqueue,
queued edit/delete, Send Now, concurrent terminal and submit, incremental
Message durability, no terminal replay, restart queue loss, and exact
`turn_id` targeting. Relevant suites include
`crates/rua-server/tests/infra/conversation_input_intake.rs`,
`crates/rua-server/tests/infra/conversation_authority.rs`, and
`crates/rua-session/tests/all/message_log_turn_index.rs`.

### Current implementation gap

The desired public ordering is terminal persistence, Active Turn/queue handoff,
then terminal/control publication. Some producer completion paths currently
emit a terminal callback before `finish_active_turn_locked` has completed the
handoff. Tests cover terminal events and next-input progression separately but
do not consistently prove that every externally observed terminal event implies
the next Turn is already installed or the Chat is idle. This is a code ordering
gap; it does not change the accepted single-transition contract.

## Related decisions

- ADR 0020: Session File and Message Log.
- ADR 0022: Conversation Authority and epoch fencing.
- ADR 0024: Stop and interjection semantics.
- ADR 0025: Runtime Notes.
- ADR 0026: Context Projection.
- ADR 0031: Durable Task Runtime.
- ADR 0035: Chat CLI single-request adapter and resident server boundary.
