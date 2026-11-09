# ADR 0035: Chat CLI single-request adapter and resident server boundary

**Status:** Accepted

## Context

The Console and command line are different Chat clients. A Console page is a
mounted viewer that can observe multiple Turns, reconnect, and remain attached
while the Chat is idle. A CLI invocation submits one input, emits that input's
Turn output, and exits.

Forcing the CLI to coordinate the Console submit and live endpoints would expose
viewer replay and reconnection concerns without useful CLI behavior. Running
Chat logic directly in the CLI process is more dangerous: the resident server
already owns Workspaces, persistence, Conversation Authority, Projects, Tasks,
security, Device Network, and synchronization.

The current CLI and server provide dedicated Agent and Project Chat adapters and
contract tests for streamed one-request behavior. Non-idle Chats use the same
Conversation Input Queue as other clients; the response waits for the submitted
input's eventual Turn rather than streaming unrelated work.

## Decision

### D1 - Console submit/live remains the mounted-viewer protocol

Console clients use separate Chat-scoped command and observation routes. Submit
accepts an input through Conversation Authority; Live Activity attaches a
viewer to current control state, replay, and future activity.

Viewer reconnect, multiple observers, queue display, and observation of Turns
created elsewhere remain Console concerns. The Console does not use the CLI
combined request as its normal transport.

### D2 - CLI Chat is one request and one response stream

The resident server exposes a Chat-scoped CLI adapter for both address variants:

```text
POST /api/agents/{agentId}/chats/{chatId}/console/chat
POST /api/projects/{projectId}/chats/{chatId}/console/chat
```

One request submits exactly one input. Its response streams only the accepted
input's eventual Turn activity and closes after that Turn's authoritative
terminal event or an explicit error. The input may start immediately or wait in
the canonical Conversation Input Queue.

### D3 - The adapter uses the canonical Conversation Input Queue

A Chat CLI Invocation is one blocking request for one accepted input. An idle
Chat starts it immediately; a non-idle Chat may queue it. The request remains
open until that input is claimed and its Turn reaches terminal state.

The HTTP request carries a stable submission identity and a preselected expected
`turn_id`, allowing the stream filter to ignore unrelated current or future
Turns while the input waits. It does not attach as a general viewer.

### D4 - Chat ID is selected before submission

When no Chat ID is supplied, the CLI creates a v4 ID before sending the request
and reports it to the caller. Reusing that ID after the previous Turn is
terminal continues the same scoped Chat and its durable Conversation State.

The route is authoritative. Any request-body Chat ID must match it. For a Lazy
Chat, first successful acceptance materializes that exact address.

### D5 - Agent and Project CLI paths share one domain service

Agent-Scoped and Project-Scoped adapters use the same response event contract,
queue-capable acceptance rule, Turn Writer, terminal rule, and error semantics.

Scope remains meaningful below the adapter. Agent-Scoped Chat follows switchable
Conversation Authority Placement. Project Chat routes to its fixed resource
Device and uses its resource binding and current Turn Agent assignment.

### D6 - CLI requires the resident server

Before Chat execution, the CLI connects to the configured resident server. If
the server is unavailable, the command fails without accepting a Turn.

The CLI does not automatically start or stop the server, construct an embedded
AppState, open Chat or Project stores directly, initialize Device Network, or
execute Project work locally. Server lifecycle is an explicit operator concern.

### D7 - One-shot streaming does not create viewer ownership

The adapter may subscribe internally to activity for the accepted input and
Turn. That subscription is scoped to the request and is not a mounted
Conversation Live Activity Viewer.

Client disconnect ends response delivery but does not implicitly Stop an
accepted or running Turn. The caller must use the explicit Stop command when it
intends to end backend work.

### D8 - Management commands remain separate

List, show, update, delete, stop, send, and stream are explicit operations over
the same scoped Chat resources. There is no separate create command required
for Chat conversation state; first Send materializes a Lazy Chat.

Management commands do not bypass authority or storage services merely because
they originate from the local command line.

## Hard invariants

- One CLI Chat request represents one accepted input.
- The response follows only that input's eventual Turn.
- A non-idle Chat may queue a CLI Chat Invocation through the canonical intake.
- CLI Chat never creates a private queue or execution runtime.
- CLI disconnect is not User Stop.
- Chat identity comes from the complete scoped route.
- The CLI never opens a second Chat runtime or AppState.
- Project execution never falls back to the CLI host Device.
- The response closes only after authoritative terminal state or error.

## Authority, persistence, concurrency, and failure boundaries

**Authority.** The resident server owns all domain state and execution. The CLI
is a transport client and formatter. Conversation Authority owns the atomic
idle check, acceptance, Turn identity, execution, and terminality.

**Persistence.** Only server-side services open Session, Project, Task, security,
and synchronization stores. The CLI may persist ordinary client configuration,
but never Chat state for the request.

**Concurrency.** Conversation Authority serializes CLI acceptance with Console
submissions. An idle winner starts; later accepted inputs join the same queue.
Submission and command IDs provide idempotency, and an accepted CLI response
selects events by input and Turn identity.

**Failure.** Connection failure before acceptance creates no Turn. A policy or
authority rejection returns `409 Conflict`; ordinary busy state may queue.
Disconnect after acceptance does not cancel work. Stale
authority, server shutdown, or terminal failure is returned as explicit typed
stream or command state without starting an embedded fallback.

## Consequences

- CLI semantics remain simple without weakening shared Chat behavior.
- The resident server is the single owner of process-wide state.
- Sequential invocations can continue a Chat by reusing its address.
- Scripts may wait behind previously accepted interactive work while retaining
  one-input/one-terminal-stream correlation.
- WebUI replay and multi-viewer complexity stays out of the command-line client.

## Rejected alternatives

- Make CLI call Submit and then manage the general Live endpoint.
- Make Console use the one-request CLI adapter.
- Maintain a second CLI-only queue beside Conversation Authority.
- Start the resident server automatically from every Chat command.
- Fall back to an embedded AppState when the server is unavailable.
- Call Agent Runtime directly from the HTTP handler.
- Treat response-stream disconnect as Stop.

## Implementation and verification

Current implementation anchors include:

- `crates/rua-cli/src/chat.rs`;
- `crates/rua-cli/src/agent.rs`;
- `crates/rua-server/src/endpoints/chat_cli.rs`;
- `crates/rua-server/src/endpoints/console.rs`;
- `crates/rua-server/src/endpoints/projects.rs`;
- `crates/rua-server/src/services/conversation_input_intake.rs`;
- `crates/rua-server/src/services/conversation_authority.rs`.

Verification must cover both scoped routes, generated and reused Chat IDs,
immediate and queued execution, rejection conflicts, input-to-Turn
correlation, response closure only after terminal commit, disconnect without
Stop, server-unavailable behavior, and absence of embedded runtime creation.
Relevant suites include
`crates/rua-cli/tests/all/chat.rs`,
`crates/rua-cli/tests/all/agent_chat.rs`,
`crates/rua-cli/tests/all/project_chat.rs`,
`crates/rua-server/tests/contract_console_http/cli_chat.rs`, and
`crates/rua-server/tests/contract_projects_http/cli_chat.rs`.

## Related decisions

- ADR 0018: Scoped Chat identity and Lazy materialization.
- ADR 0019: Project fixed resource binding.
- ADR 0022: Conversation Authority routing.
- ADR 0023: Input queue and Active Turn.
- ADR 0024: Viewer Disconnect is not Stop.
- ADR 0034: Interactive Terminal remains a separate runtime.
- ADR 0049: Tauri Desktop Shell and local resident server.
