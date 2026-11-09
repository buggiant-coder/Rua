# ADR 0033: ACP Chat Mode, client projection, and permission relay

**Status:** Accepted

## Context

Rua uses ACP as a Conversation Driver for user and collaboration Chats.
ADR-0053 removes the separate ACP Task Runner lifecycle. Both origins use the
same protocol client, Tool projection, and permission relay; each Chat retains
its own Authority, history, configuration, and execution environment.

ACP updates are stateful. `raw_input` and `raw_output` are current snapshots,
not arbitrary text deltas, and sparse updates may refer only to a Tool Call ID.
Permission options are runner-owned identifiers with meanings such as allow
once, allow for a session, or decline. Flattening either model loses protocol
semantics.

## Decision

### D1 - ACP Chat Mode is a Conversation Driver

Entering ACP binds the Chat to one configured runner session on the Conversation
Authority Device. ACP output advances the same Active Turn and is committed by
the same Turn Writer and terminal authority as built-in Agent execution.

ACP Enter is itself a Chat-scoped control and may materialize a Lazy Chat's
metadata and ACP binding without creating a Conversation Turn. Read-only runner
or resumable-session discovery does not materialize the Chat.

ACP Chat Mode does not become a Task, create a second Conversation Authority,
or expose runner session identity as Chat identity. Stop, interjection,
placement, queue, and Viewer Disconnect follow the Chat contracts.

### D2 - ACP delegation is a collaboration Chat

chat_create selects a fixed ACP runner and creates an independent collaboration
Chat. chat_send continues it with bounded yield and result return; chat_read
returns bounded progress; chat_stop_turn stops its Turn. There is no ACP Task
Attempt or task_send_input dialogue. Restart interrupts execution without replay;
later explicit requests use the client protocol, never a hidden Task fallback.
Source environment and authorization are inherited under ADR-0028/0053; runner
internal permissions remain separate from enforceable Rua host authority.

### D3 - One ACP Client Projection owns Tool state

Each ACP session maintains one projection keyed by
`{session_id, tool_call_id}`. It applies Tool Call and Tool Call Update messages
using protocol semantics and retains current title, kind, status, input, output,
content, locations, and stable Rua projection identity.

Sparse updates and permission requests are enriched from this accumulated state.
Known values are not replaced by `unknown`, and repeated structured snapshots
are not concatenated as text.

### D4 - Terminal Tool projection is canonical and shared

At terminal Tool status, the projection emits one authoritative Tool Call and
Tool Result containing the complete accumulated input and output. ACP Chat
history and collaboration inspection consume the same projection code.

Presentation modes may choose which detail to display, but they do not define
different parsers or identities. Concurrent Tool calls receive distinct stable
indexes and cannot overwrite one another.

### D5 - ACP Permission Relay preserves exact runner choices

An ACP permission request creates a transient ACP Permission Relay entry scoped
to the ACP session, full Chat ownership, Tool Call, and request identity.
Every runner-provided option is preserved with exact ID, name, kind, and order.

The selected option ID is validated against the pending request and returned to
the runner unchanged. A runner-defined reject option is still a selected option;
ACP `cancelled` is reserved for Stop, timeout, process loss, session close, or
transport cancellation without a user selection.

### D6 - ACP permission is not Rua approval reuse

The relay may reuse waiting and notification infrastructure, but it is not an
Approval Authority `ApprovalRequest`. It does not expose Rua `run_once`,
Chat-level, Agent-level, or permanent reuse choices and does not create
`ApprovalReuseRule` records.

Runner-owned session or always choices remain runner-owned. Rua's host
sandbox, Tool Guard, hard denies, and process constraints remain independent and
may still veto execution.

### D7 - Runner status reports configuration truth

An ACP runner is configured as enabled or disabled. Listing, loading, editing,
or enabling a runner does not start a process, perform a handshake, or persist a
cached availability judgment.

Actual startup is checked when a Chat operation begins. Missing command,
authentication failure, protocol mismatch, and startup timeout are reported on
that operation without changing the configured enabled state. Diagnostics may
probe readiness but remain advisory.

### D8 - Control and placement follow the parent aggregate

ACP controls route through Conversation Authority, current Authority Epoch and
the Chat's inherited device-aware execution binding. The client cannot choose another Device or move a
session to recover from failure.

Session closure cancels unresolved permission relay entries for that session
only. It does not cancel unrelated ACP work in the same Chat or process.

## Hard invariants

- ACP Chat Mode is a Conversation Driver, not a Task.
- ACP delegation uses collaboration Chat lifecycle, not a Task Runner.
- Session identity does not replace Chat or Turn identity.
- Tool state is keyed by session and Tool Call ID.
- Snapshot fields replace prior values according to ACP semantics.
- Permission option IDs and order reach the runner unchanged.
- ACP permission choices never create Rua reusable approvals.
- Enabled status does not claim runtime availability.
- Chat authority and resource binding determine placement and lifecycle.

## Authority, persistence, concurrency, and failure boundaries

**Authority.** Conversation Authority owns ACP Chat lifecycle; the runner owns
option semantics; Rua security owns the
host boundary. ACP Client Projection owns protocol-state interpretation.

**Persistence.** Canonical output is persisted through Turn Writer; collaboration
outcomes return as typed Tool observations to the source. Permission requests and
session process state are transient. Runner configuration persists only enabled
state and declared launch configuration.

**Concurrency.** Tool updates are serialized per ACP session and separated by
Tool Call ID. Prompt operations within one session are ordered. Permission
requests have independent identities and session-scoped cancellation.

**Failure.** Sparse or malformed updates fail without corrupting accumulated
state. Invalid option IDs are rejected. Process loss closes the session and
cancels unresolved relays. No cached health state or local fallback hides the
failure.

## Consequences

- User and collaboration Chat surfaces display consistent Tool state.
- Exact runner-owned permission semantics are preserved end to end.
- Rua approval reuse remains orthogonal to ACP permissions.
- Runner cards avoid misleading cached availability states.
- Shared client fixes benefit both Chat origins.

## Rejected alternatives

- Maintain separate ACP Tool parsers or Task dialogue lifecycles for delegation.
- Append every `raw_input` or `raw_output` snapshot as a delta.
- Map runner permissions to allow/deny booleans.
- Convert runner options into Rua reusable approval rules.
- Select the first option automatically for trusted runners.
- Start runners in the background to determine card availability.
- Let ACP transport choose a replacement execution Device.

## Implementation and verification

**ADR-0053 amendment gap:** retire the ACP Task adapter and control paths.
Existing Task-specific tests are retirement coverage, not target APIs. Verify
fixed executor, inherited environment/security, result/wake delivery, and restart
outcomes through Chat Tools before declaring this amendment implemented.

Current implementation anchors include:

- `crates/rua-acp/src/client.rs`;
- `crates/rua-acp/src/tool_projection.rs`;
- `crates/rua-acp/src/permissions.rs`;
- `crates/rua-acp/src/service.rs`;
- `crates/rua-server/src/services/acp_chat_registry.rs`;
- `crates/rua-server/src/endpoints/acp_chat.rs`;
- `crates/rua-acp/src/task_runner.rs`.

### Current implementation gaps

The permission relay does not currently preserve the complete Chat Address.
`crates/rua-acp/src/permissions.rs::AcpPermissionRequestContext` and
`AcpPermissionRequest` store only `agent_id` and `chat_id`;
`AcpPermissionRelay::create_request` derives its default scope key from those
two strings, while `snapshot` and `decide` filter on the same pair. Agent- and
Project-Scoped Chats may legally share a Chat UUID and execution Agent, so their
pending requests can collide or be exposed through the wrong scoped route.

`create_request_in_scope` supplies a separate session cancellation key, but it
does not repair the request ownership used for snapshot and decision
authorization. Existing
`crates/rua-acp/tests/acp_permissions/relay.rs` coverage verifies exact option
round-trip, task provenance, sparse Tool enrichment, and session-scoped
cancellation; it has no same-UUID cross-scope collision case. The relay context,
serialized request, lookup, snapshot, and decision APIs must carry
`ChatAddress`.

Verification must cover replacement snapshots, sparse updates, interleaved Tool
calls, exact allow and reject option IDs, session-scoped cancellation, absence
of Rua reuse rules, same-UUID Agent/Project scope isolation, enabled runner
listing without launch, Chat placement, and collaboration result routing. Relevant
suites include
`crates/rua-acp/tests/client/tool_projection.rs`,
`crates/rua-acp/tests/acp_permissions/relay.rs`, and
`crates/rua-server/tests/contract_acp_http/acp_routes.rs`.

## Related decisions

- ADR 0022: Conversation Authority routing.
- ADR 0023: Active Turn lifecycle.
- ADR 0024: Stop and Viewer Disconnect.
- ADR 0029: Product Hook permission boundaries.
- ADR 0031: Durable Task Runtime.
- ADR 0032: Task Runner and control.
- ADR 0037: Device Network Runtime and shared Device Sessions.
