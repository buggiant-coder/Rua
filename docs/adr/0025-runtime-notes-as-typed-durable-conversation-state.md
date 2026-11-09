# ADR 0025: Runtime Notes as typed durable Conversation State

**Status:** Accepted

## Context

Runtime subsystems need to record facts that affect a conversation without
pretending those facts were spoken by the user, assistant, or Tool. Examples
include Task Attempt completion, runtime failure, capability changes, Hook
activity, and asynchronous Tool observations.

An earlier shape treated every note as a Background Task result and stored
pending notes in Session header extras with an injection flag. That couples
chronology to prompt assembly, loses typed meaning, and permits the same fact to
be injected differently by multiple Hooks.

The current session model provides a tagged Runtime Note envelope and appends it
through Conversation State. Frontend projection reads the typed note directly
from the canonical Message representation.

## Decision

### Collaboration results and model observation (ADR-0053 amendment)

Add a typed collaboration-result payload carrying stable request/result identity,
target Turn provenance, truthful outcome, bounded reply, and resource references.
It is neither a TaskAttemptTerminal payload nor ordinary user input. Source
Authority accepts it independently of user-input acceptance; when idle it may
start a result-triggered Turn, and when active it delivers at a safe observation
boundary. Explicit Stop suppresses wake until user resumption, not result storage.

Yield/direct ToolResult, deferred notification, and explicit Read coordinate one
canonical result and automatic delivery evidence. A submitted receipt cannot be
replaced retroactively or receive another result on the same provider call ID.
Projection may synthesize a valid stable tool pair referencing the original
request. UI subscriptions never opt the model into intermediate raw output.
There is no model Task_Watch/Unwatch or persistent Agent mailbox.

Current append_runtime_note requires an existing Turn and may park notes for
active execution; it is not sufficient for idle result acceptance. Extend typed
session storage and Turn trigger representation without fake user messages.

### D1 - Runtime Note has a small typed envelope

Every Runtime Note contains:

```text
runtime_note_id
kind
created_at
payload
```

`kind` selects a versioned payload contract. Producers may not share one large
struct filled with unrelated optional fields. The current tagged enum rejects
unknown kinds; forward-compatible preservation would require an explicit
unknown envelope or migration rather than coercion to a known kind.

### D2 - Runtime Notes are Conversation State

A Runtime Note is appended to the Message Log with ordinary sequence and Turn
provenance. It participates in synchronization, history replacement reachability,
branching, rollback, search policy, UI reconstruction, and Context Projection
according to its type.

It is not stored only in Session File extras, transient live events, Hook logs,
Task tables, or frontend caches.

A note that arrives at an unsafe append point may be held in the durable
`.pending_runtime_notes.json` staging file beside the Chat. That staging file is
delivery state, not a second chronology: the note becomes canonical only when
it is appended once to the Message Log, after which the staging entry is
removed.

### D3 - Producers submit facts; the subsystem owns chronology

Task Runtime, Conversation runtime, Memory, capability management, and Hook
infrastructure submit typed note payloads to one Runtime Note subsystem. They do
not append arbitrary Chat Messages or choose final model roles themselves.

The subsystem validates the payload, assigns identity, selects the first legal
chronology point, appends once, and triggers ordinary durable publication.

### D4 - Notes wait for a legal Model Call Boundary

If a model response or Tool group is in progress, a note waits until appending
it cannot split provider-required call/result structure or mutate an in-flight
request. Once appended, it remains visible to every later applicable projection;
there is no mutable `injected_at` flag.

When no Active Turn exists, a runtime-owned fact may append immediately or cause
a dedicated notification Turn according to the producing subsystem's contract.

### D5 - Model visibility is type-specific

Durability does not imply model visibility. Each Runtime Note kind declares how
Context Projection treats it:

- model-visible typed context;
- asynchronous Tool observation;
- bounded terminal Task report;
- activity-only record excluded from model input;
- UI-hidden audit or causality record.

The renderer cannot infer policy from free-form text, source name, or the fact
that a note is persisted.

### D6 - Initial kinds remain semantically distinct

The currently implemented `RuntimeNotePayload` families are Task Attempt
Terminal, Conversation Runtime Terminal, Memory Signal, Capability State, and
Task Watch Output. Hook Activity is part of the proposed Product Hook protocol
in ADR 0029, but the current Runtime Note enum does not yet contain a durable
Hook Activity variant.

Stop and Interjection are Turn lifecycle markers, not Runtime Notes. Current
Tool catalogs, Skill catalogs, effective Harness, execution environment, and
running limits are Model Call Context and are rebuilt rather than recorded as
notes on every call.

Reaching a configured tool-round limit is a Conversation Runtime Terminal fact
with reason `max_iterations`, not a user Stop. The runtime appends this Note after
the last complete Tool group and before its tool-free summary. The Note records
the limit and that unfinished work remains eligible for later continuation; it
survives history reload and is visible in subsequent Context Projection. The
terminal event reports `max_iterations` and the Turn is interrupted rather than
reported as normal completion.

### D7 - Notes never impersonate conversation authors

Runtime Notes are not user messages, assistant responses, or System instructions
in canonical storage. Context Projection may render a typed model-visible note
into a provider-valid role, but that rendering is derived and retains runtime
provenance.

The UI similarly renders by note kind and must not assume every note is a Task
card or ordinary transcript bubble.

## Hard invariants

- Every Runtime Note has stable identity, kind, timestamp, and typed payload.
- Notes are appended once in Message Log chronology.
- Producers cannot write Session extras as an alternate note store.
- Pending-note staging cannot be read as canonical Conversation chronology or
  as proof that the note has been delivered.
- Model visibility is defined by note kind, not by persistence alone.
- Notes never claim user or assistant authorship.
- Stop and interjection remain Turn markers.
- Current capability state is rebuilt, not accumulated as note history.
- An in-flight model request is never mutated by a late note.

## Authority, persistence, concurrency, and failure boundaries

**Authority.** The producing subsystem owns the fact; the Runtime Note subsystem
owns validation and append semantics; Conversation Authority and Turn Writer
own placement in active Chat chronology; Context Projection owns model rendering.

**Persistence.** Notes live in the Message Log and use its sequence, atomicity,
sync, and recovery guarantees. Auxiliary producer stores may retain source
details, but cannot replace the Conversation record.

**Concurrency.** Concurrent producers enqueue typed notes. Ordering is assigned
at append time under the Chat writer boundary. Notes that arrive during an
unsafe Tool/model segment wait for the next legal boundary.

**Failure.** Invalid payloads fail before append. A failed append does not set a
delivery flag or pretend the note was consumed. Duplicate producer delivery is
deduplicated by `runtime_note_id`.

## Consequences

- Runtime-originated facts remain durable and auditable without corrupting
  speaker identity.
- New note kinds require a payload schema and explicit projection policy.
- Task and Hook systems can share the envelope without sharing domain payloads.
- UI and model projections may intentionally differ while reading one canonical
  record.
- Session header remains focused on current Chat state and pointers.

## Rejected alternatives

- Extend one Task-shaped note struct with optional fields.
- Keep pending notes only in Session File extras.
- Inject a note once and mark it consumed with mutable state.
- Render every durable note as a System message.
- Store notes only in Live Activity or frontend memory.
- Encode Stop and Interjection as generic runtime notes.
- Let producers append arbitrary role-bearing messages.

## Implementation and verification

Current implementation anchors include:

- `crates/rua-session/src/runtime_note.rs`;
- `crates/rua-session/src/store.rs`;
- `crates/rua-agent/src/task_runtime_notes.rs`;
- `crates/rua-server/src/services/task_notification.rs`;
- `crates/rua-server/src/services/context_projection.rs`;
- `console/src/pages/Chat/owned/projection.ts`.

The current Product Hook run/contribution lists are process-memory diagnostics,
not Runtime Notes. Durable Hook Activity coverage remains an implementation
requirement of Proposed ADR 0029.

Verification must cover typed round trips, sequence ordering, deduplication,
append during idle and active execution, safe-boundary deferral, sync, rollback,
branching, UI projection, and model-visible versus activity-only policy.
Relevant suites include `crates/rua-session/tests/all/runtime_notes.rs`,
`crates/rua-server/tests/infra/runtime_notes.rs`, and
`crates/rua-agent/tests/unit_hooks/runtime_note_injection.rs`.

## Related decisions

- ADR 0020: Append-only Message Log.
- ADR 0023: Turn Writer and Active Turn.
- ADR 0026: Context Projection.
- ADR 0029: Product Hooks.
- ADR 0031: Durable Task Runtime.
- ADR 0032: Task Watch and control.
