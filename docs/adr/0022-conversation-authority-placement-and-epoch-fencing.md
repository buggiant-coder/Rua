# ADR 0022: Conversation Authority, placement, and epoch fencing

**Status:** Accepted

## Context

A Chat can be viewed and controlled from several Devices while execution and
durable foreground mutation occur on one Device. Local history copies, live
streams, controllers, and synchronization replicas are insufficient to decide
which Device may accept input or finalize a Turn.

Rua therefore needs one business authority per Chat Address, an explicit
placement record, and a generation token that invalidates work accepted by an
older authority. Without all three, a network partition or delayed command can
produce concurrent writers, stale Stop operations, or divergent terminal facts.

The current server implements these concepts in the conversation authority,
placement, router, target, input-intake, and Device Command Bus services.

## Decision

### D1 - One Conversation Authority owns one Chat

For each materialized Chat Address, exactly one Conversation Authority is active
on the authority Device. It owns:

- current placement and Authority Epoch;
- at most one Active Turn;
- the runtime Conversation Input Queue;
- accepted-submission identity and command deduplication;
- the Conversation Driver and ACP binding;
- Stop, interjection, queue, model, and Chat-option controls;
- the Turn Writer and serialized transition to the next Turn or idle;
- the authoritative Conversation Control View.

Deep implementation modules may perform parts of this work, but they are not
independent business authorities.

### D2 - Placement is durable Chat state

Conversation Authority Placement identifies the Device currently permitted to
host authority for the Chat. The Session File is the sole durable authority for
the current placement and epoch.

Chat Index may contain a denormalized placement copy for list display, and Turns
may retain producing-Device provenance. Neither may be used to route a mutation
when the Session File placement is missing or disagrees.

Agent-Scoped Chat placement may be switched through the authority adoption
protocol. Project-Scoped Chat placement is pinned to the Project resource
Device by ADR 0019.

### D3 - Authority Epoch fences every mutation

Initial Chat materialization occurs only when the target accepts the first
input, for both Agent and Project scopes. Draft model/ACP choices remain at the
source; ACP binding and Session startup follow acceptance. Media upload stages
files at the source without installing placement; required files are transferred
and verified before target acceptance. Queries and ordinary controls never
create a missing Chat. Explicit ACP operations on existing Chats retain their
authority-owned behavior.

A rejection before acceptance leaves no materialized Chat. An execution failure
after acceptance retains the Chat, input, and committed failure fact. A lost
acceptance reply is an unknown result, not proof of rejection: the browser keeps
the original submission data in its existing draft/storage lifetime for refresh
reconciliation. This does not add server restart recovery or an exactly-once
guarantee for external side effects.

Remote input confirmation has a finite 30-second window starting after required
media preparation, independent of the shorter control-command window. The
prepared delivery payload stays stable across observation of the same attempt;
a confirmation timeout neither cancels accepted work nor proves rejection. An
unconfirmed HTTP result carries `mode: unconfirmed`, `delivery_unknown`, and the
original Chat, submission, Device delivery, target, and sequence identifiers;
Authority-owned input IDs and acceptance timestamps remain absent until known.
The source preserves its existing submitted draft for canonical-history
reconciliation and does not automatically resend the mutation.

An acceptance reply follows complete canonical admission, including the first
Chat configuration, placement, and user boundary. Post-commit Sync publication
runs under the existing Workspace Runtime task tracker with cancellation and a
finite lifetime; a blocked Sync lock cannot hold the acceptance reply. Optional
Control View enrichment has a bounded best-effort wait and may be omitted.

Each successful initial materialization or authority adoption installs a new
Authority Epoch represented by Rua's sortable HLC tuple
`(wall_ms, logical_counter, device_id)`. It is an opaque generation token to
callers, not a sequential counter or user-visible Turn number. Every
state-changing command and accepted foreground operation is evaluated against
the current Chat Address, authority Device, and epoch.

A receiver that has observed a newer Epoch rejects stale submit, stop,
interjection, queued-input mutation, Chat control, Media commit, and terminal
transition attempts. It returns a typed redirect or conflict containing current
placement information when available.

Accepted submissions, Active Turns, controls, Conversation Live Activity,
terminal facts, and stable Turns carry the Epoch that accepted or produced
them. After replicas converge, higher-Epoch history wins at conflicting
positions; lower-Epoch output may remain available for diagnostics but is
excluded from normal history, Context Projection, Memory, summaries, and UI.

### D4 - Authority switch is target-owned adoption

A user-directed switch sends one adoption command directly to the selected
target Device. The target prepares required stable history and media, verifies
the candidate state, persists its new placement and epoch, constructs local
authority state, and only then acknowledges success.

The source Device does not appoint the target by editing a local projection.
There is no all-peer election or automatic best-Device selection. The target's
successful durable adoption is the linearization point.

### D5 - The old authority is superseded

After adoption, the new authority makes one bounded best-effort supersession
attempt to the previous authority after acknowledging the successful adoption.
Failure does not roll back adoption or create a durable retry queue. A previous
authority that has observed the higher Epoch rejects stale work; any lower-Epoch
facts produced before it learns the supersession lose during convergence.

Unclaimed queue entries do not migrate. They are runtime authority state and
are superseded by the switch. A claimed Active Turn and Device-bound Tasks are
stopped or interrupted according to their lifecycle contracts before a
cooperative handoff; an unreachable old Device is fenced when it later resumes.

### D6 - Controllers route; they do not decide

For existing Chats, the Conversation Authority Router reads current placement, chooses local or
remote transport, includes the epoch, and may follow exactly one higher-Epoch
Authority Redirect with the same command or submission identity. It verifies
the redirect against the target's initial Conversation Control View, stores the
Verified Authority Observation without minting a new Epoch or publishing a new
switch fact, and rejects a second redirect instead of looping. It does not infer
authority from connection presence.

Frontends, CLI adapters, Device Sessions, Active Stream registries, and sync
consumers are controllers, transports, or projections. None can decide whether
a Chat is busy, terminal, or ready for the next input.

### D7 - Control View is the read-only authority projection

The authority publishes an ordered Conversation Control View containing
placement, epoch, revision, Active Turn, queue, ACP state, current Chat options,
and allowed controls. Viewers use it to render and target commands.

The view is not a second writable state machine. A local view of a remote Chat
must not be synthesized from stale history or frontend assumptions.

## Hard invariants

- A Device accepts mutations only for the current Authority Epoch it has
  observed for the Chat Address.
- Authority Epoch is a totally ordered HLC value, never `N + 1` or bare wall
  time.
- Session File placement is the durable routing authority.
- Every mutating remote command is epoch-fenced.
- A stale Device cannot finalize or publish authoritative state.
- Authority adoption is committed by the selected target Device.
- Project-Scoped placement never leaves the resource Device.
- Runtime queue entries do not migrate during authority switch.
- A controller follows at most one verified higher-Epoch redirect per
  foreground operation and preserves command identity.
- Lower-Epoch conflicting history never enters normal model or UI projection.
- Control View is a projection, not an independent authority.

## Authority, persistence, concurrency, and failure boundaries

**Authority.** Conversation Authority owns all foreground Chat decisions. The
Router locates it; Device Session transports commands; Sync carries durable
facts after commit.

**Persistence.** Placement and epoch live in the Session File. Active execution
handles and queue entries are memory-only. Stable history and required Media
must be prepared on the target before adoption commits.

**Concurrency.** A per-Chat transition gate serializes local adoption, input
acceptance, stop/interjection, terminal commit, and queue handoff. HLC ordering,
manifest validation, redirect verification, and higher-Epoch conflict handling
determine the surviving authority generation; the design does not introduce a
distributed election, lease, or quorum.

**Failure.** Unreachable targets, stale epochs, incomplete preparation, and
missing authoritative Session state fail explicitly. A controller does not
queue indefinite intent or fall back to local execution. Recovery reconciles
stale local execution as interrupted rather than replaying it.

## Consequences

- Cross-device control has one routing and fencing model.
- Stable replication cannot grant foreground write authority by itself.
- Authority switches may discard unclaimed queued input and interrupt
  Device-bound work.
- Readers must distinguish durable placement, runtime Control View, and
  transient connection state.
- Every new Chat-scoped mutation must carry or resolve an Authority Epoch.

## Rejected alternatives

- Elect authority from whichever Device receives the request.
- Infer authority from an active stream or local history copy.
- Store placement only in Chat Index or frontend state.
- Permit both old and new Devices to finish already-started writes.
- Transfer runtime queue entries during authority adoption.
- Route stale commands without epoch validation.
- Automatically move a Project Chat away from its resource Device.

## Implementation and verification

Current implementation anchors include:

- `crates/rua-server/src/services/conversation_authority.rs`;
- `crates/rua-server/src/services/conversation_authority_placement.rs`;
- `crates/rua-server/src/services/conversation_authority_router.rs`;
- `crates/rua-server/src/services/conversation_authority_target.rs`;
- `crates/rua-server/src/services/device_command_bus.rs`;
- `crates/rua-server/src/services/conversation_input_intake.rs`.

Verification must cover HLC ordering, local and remote routing, stale-epoch
rejection, target-owned adoption, one-redirect verification and observation,
second-redirect rejection, missing Session placement, Project placement lock,
superseded queue entries, lower-Epoch projection exclusion, and stale terminal
attempts. Relevant suites include
`crates/rua-server/tests/infra/conversation_authority.rs`,
`crates/rua-server/tests/infra/conversation_authority_target.rs`,
`crates/rua-server/tests/infra/device_session_control_view.rs`, and
`crates/rua-server/tests/infra/conversation_input_intake.rs`.

## Related decisions

- ADR 0018: Complete Chat Address.
- ADR 0019: Fixed Project resource binding.
- ADR 0023: Active Turn, Turn Writer, and input queue.
- ADR 0024: Stop, interjection, and Viewer Disconnect.
- ADR 0030: Authority-routed Media transfer.
- ADR 0032: Task authority routing.
- ADR 0037: Device Network Runtime and shared Device Sessions.
- ADR 0044: Live-assisted hydration and cross-device conversation continuation.
