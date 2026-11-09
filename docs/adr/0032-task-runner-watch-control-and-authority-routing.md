# ADR 0032: Tool Task Runner, bounded Read, and authority routing

**Status:** Accepted

## Context

The durable Task aggregate owns background Tool execution and processes.
ADR-0053 moves Agent/ACP conversations into target Chats and removes the distinct
Subagent executor. Task no longer owns Agent history, steering, or conversation
lifetime; its controls remain independent from mounted viewers.

Tasks may also execute after their originating Turn ends. Their live output,
waiters, input, cancellation, retry, and remote routing cannot be owned by a
mounted Chat viewer or by the Runner process itself.

Current implementations provide `TaskRunner`, typed adapters, Task tools and
HTTP endpoints, Runner Sessions, Task notification delivery, and authority-side
routing through the server.

## Decision

### D1 - Task Runner is a typed execution adapter

`TaskRunner` accepts a `TaskRunRequest` containing Task ID, Attempt number,
immutable Spec, captured Chat authorization/configuration provenance, owning Device context, dependencies,
cancellation token, Task directory, and Runner Session. It returns one bounded
`TaskRunOutput` or a typed Runner error.

The Tool Runner validates the real Tool name and arguments. Generic dispatch
must not bypass available Tools or captured Chat authorization. Agent and ACP
collaboration have no Task Runner adapter in the target design.

### D2 - Runner Session owns live execution interaction

A non-terminal Attempt may have one Runner Session. The session owns canonical
live output records and the Runner-specific input capability.

Task input is actual Tool/process input such as stdin or EOF, when supported.
It is not Agent steering or ACP messaging. Input does not create another
Attempt; explicit Retry does.

### D3 - Task control is Chat-scoped

Create, Read, List, SendInput, Cancel, and Retry operate
through one Task control surface and require the owning Chat Address.

Cross-Chat lookup and control are treated as not found. Project Chat Agent
changes do not change Task ownership or the authorization Agent of an existing
Attempt.

### D4 - Task Read combines inspection and bounded observation

`task_read` returns state, bounded incremental output, cursor metadata, and
terminal results. Zero wait reads immediately; a positive finite wait returns
on new output, terminal outcome, or deadline. Deadline or caller interruption
detaches the waiter without cancelling or failing execution. Dependency waiting
and blocking remain visible. Read replaces Get/Await, not execution lifetime.

Bounded Read is on-demand inspection, not a source-Agent scheduling loop. Under
ADR-0053 D35, the source continues independent work or normally ends its Turn;
long-running Tasks do not require repeated reads, duration estimates, sleep, or
a held waiter to obtain their terminal results.

### D5 - UI subscriptions are not model Tools

Remove model-facing Task_Watch/Task_Unwatch. UI subscribes to canonical Runner
output and can replay by cursor independently of model actions. Subscription
positions are not execution authority or a second log. Intermediate output is
read on demand by the model through task_read, not automatically injected merely
because a viewer subscribes. No dormant model Watcher is revived by Retry.

### D6 - Bounded Tool output and terminal observations

Tool output uses deterministic bounds and explicit resource references, not
LLM progress summarization. Each Attempt returns one bounded terminal result.
Terminal notification and explicit reads share result identity and delivery
evidence to avoid duplicate automatic projection/wake. Notifications project as
Tool observations, never fake user messages. Agent progress summaries instead
belong to chat_read under ADR-0053.

Terminal results also carry continuation responsibility under ADR-0053 D35:
successful acceptance by the source input queue completes delivery, independently
of subsequent model calls or Turn completion. No history-scan consumption checks
or reply-dependent retries are part of Task delivery.
deliver at an active source Turn's safe boundary, or wake an idle eligible source
through Conversation Authority. Persisting a Runtime Note alone is insufficient.
Normal Turn completion permits wake; explicit user Stop suppresses it without
implicitly cancelling the Task. Waiting presentation comes from canonical pending
work/delivery facts, not a model-managed subscription or an artificial Active Turn.

### D7 - Controls route to the execution authority

Task execution is fixed to the Conversation Authority Device and Authority
Epoch that accepted the Task Attempt. A later Chat authority switch does not
move the running Attempt. The Task Device Context records the accepted Device
and Epoch.

Controllers route commands to that Device through the shared Device Session and
server Task authority service. They do not execute locally, alter placement, or
fall back to another Device. The target validates owner, Task ID, Attempt, and
current execution state.

### D8 - Cancellation and completion are linearized

Task Cancel requests Runner cancellation and returns only after truthful
terminal persistence. Natural completion may win a late cancellation, but one
Attempt cannot publish both completed and cancelled outcomes.

Foreground Tool adoption follows the same rule: either ordinary Tool completion
wins, or ownership transfers once to Task Runtime. The Tool is never restarted
and the caller receives exactly one foreground result.

### D9 - Viewers do not own execution

Viewer disconnect, Chat navigation, Turn completion, and SSE loss do not cancel
a Task. Runtime restart removes transient subscriptions and interrupts active
Attempts according to ADR-0031, without replaying execution.

Terminal completion seals Runner output and emits its Runtime Note independently
of UI subscription state.

### D10 - Task Tools expose compact operation-specific observations

Apply ADR-0010's model-facing projection boundary to all six public Task Tools,
foreground adoption receipts, and automatic terminal observations. Do not expose
TaskRecord, Runner Session, or Device Context wholesale merely because those
objects are serializable. Keep canonical records and HTTP contracts separate.

- Create returns Task identity and admission state, with blocking or approval
  reasons when relevant; it does not echo an unbounded instruction.
- List returns bounded discovery entries with short descriptions and an explicit
  indication when the listing is incomplete.
- Read returns state, bounded new output, usable continuation, and available
  terminal evidence. Dependency blocking, approval waiting, read timeout, and
  execution failure remain distinct.
- Cancel reports the actual terminal outcome, including natural completion that
  won the race, without attaching an unrelated full result envelope.
- Retry identifies the new Attempt and its state. It is not an implicit replay
  triggered by an expired read wait.
- SendInput acknowledges acceptance, not proof that the Runner processed the
  bytes or completed the requested operation.

Task lifecycle state must not substitute for the underlying Tool outcome. Expose
structured exit/error evidence when the Runner provides it; never infer a command
exit code from log text or treat completion alone as proof that tests passed.
Necessary resource/device references remain available without routinely emitting
the complete internal authority record.

Read continuations are opaque to the model, bound to Task and Attempt, and
validated on use. Retry must not silently reinterpret an old cursor against a
new Attempt. Terminal state does not imply that all output has been consumed.
Output blocks may coalesce adjacent same-source records and remove presentation
control codes while preserving observed order and raw evidence. Do not invent a
total order between streams where none was recorded.

Automatic observations and Read use the same result identity and delivery
evidence; compact formatting must not create duplicate result wakes or conceal
partial/truncated output. Exact field names belong to tool schemas and contract
tests, not a second Task lifecycle or a generic action-dispatch API.

## Hard invariants

- Runner selection and request payload are typed.
- Task controls require the complete owning Chat Address.
- One Attempt has at most one Runner Session.
- Read timeout never becomes a Task timeout.
- UI subscription never opts the model into intermediate output.
- Explicit reads and automatic terminal delivery do not duplicate result wakes.
- Agent/ACP dialogue is not Task input or a Task Runner.
- Remote control never changes Task execution Device.

## Authority, persistence, concurrency, and failure boundaries

**Authority.** Task Runtime owns lifecycle; Task authority routes commands;
Runner adapters own execution mechanics; Conversation Authority owns delivery
of Task notifications into Chat Turns.

**Persistence.** Task records, Attempts, results, and canonical Runner output
are durable according to Task storage. Waiters, viewer positions, process
handles, and transport subscriptions are runtime-only.

**Concurrency.** Runtime enforces launch limits and one active Attempt. Result
wakes are deduplicated and coalesced. Input ordering is assigned by the Runner
Session. Completion, cancellation, and adoption have one linearized winner.

**Failure.** Unsupported input, stale Attempt, unavailable Device, broken
session, route failure, or permission failure returns explicit state. Commands
are not redirected to another Device, and retries do not duplicate accepted
input because submission identity is stable.

## Consequences

- New Runner kinds implement one trait and common lifecycle tests.
- The Agent can read or wait without owning Task execution.
- Task activity remains available to UI without automatically entering model
  context.
- Remote Task control uses the same Device network rather than a runner-specific
  transport.
- Long-running Tool adoption preserves side-effect identity.

## Rejected alternatives

- Let each Runner expose unrelated public controls.
- Poll Task storage rather than use event-driven bounded Read.
- Require a model Watch call for UI visibility or copy output into another log.
- Treat queued notification as proof of completed result delivery.
- Project delegated Agent or ACP transcripts into the parent model.
- Cancel Tasks when a viewer or originating Turn disconnects.
- Retry remote controls on another Device.

## Implementation and verification

Current implementation anchors include:

- `crates/rua-task/src/runner.rs`;
- `crates/rua-task/src/session.rs`;
- `crates/rua-task/src/runtime.rs`;
- `crates/rua-tool/src/extended/task_tools.rs`;
- `crates/rua-server/src/endpoints/tasks.rs`;
- `crates/rua-server/src/services/task_authority.rs`;
- `crates/rua-server/src/services/task_notification.rs`;
- `crates/rua-server/src/services/device_command_bus.rs`.

### Current implementation gaps

D10 is an accepted target, not a claim that compact output is already implemented.
Current Task Tools include shared result envelopes and serialized live-output
records/cursors that require operation-specific projection. Verify all six Tools
and notification/adoption paths at the model-visible boundary, including empty
output, failure, approval, dependency blocking, cancel/completion races, output
larger than the read budget, cursor replay, and stale cursors after Retry.

ADR-0053 changes are not yet implemented: replace Get/Await and model Watch
Tools; retire Agent/Subagent/ACP Task adapters; add Read/result delivery
coordination and effective Chat snapshot authorization. Historical Watch tests
below describe code to replace, not the amended product contract.

D7 is not yet implemented for Agent-Scoped Tasks:

- `ConversationAuthority::update_authority_placement_guarded` calls
  `TaskRuntime::interrupt_owner` before committing an Agent Chat authority
  transfer. The contract test
  `authority_transfer_interrupts_tasks_and_commits_the_runtime_note_before_handoff`
  in `crates/rua-server/tests/contract_chats_http/tasks.rs`
  explicitly asserts `TaskState::Interrupted`. This is the opposite of keeping
  the running Attempt on its accepted Device and routing later controls there.
- Agent-scoped list, get/result, send-input, watch/unwatch, cancel, and retry
  handlers in `crates/rua-server/src/endpoints/tasks.rs` call the local
  `state.task_runtime` directly. Project-scoped handlers first use
  `ProjectDeviceAction` and `remote_project_task_response`, so only the Project
  surface currently has a remote routing path.
- Agent-visible `task_list`, `Task_Get`, `task_send_input`, `Task_Watch`,
  `Task_Unwatch`, `task_cancel`, and `task_retry` in
  `crates/rua-tool/src/extended/task_tools.rs` similarly resolve a local
  `TaskRuntimeHandle` from `ToolContext` and invoke it directly.

Task Device Context is persisted with Attempts, but the Agent control surface
does not yet use it to select a remote Device. Conformance requires preserving
the original Attempt during Chat authority transfer and routing all subsequent
Agent Task controls to that recorded Device and epoch. Tests must replace the
current interruption expectation and cover remote Agent list/get/result,
send-input, watch/unwatch, cancel, retry, and the equivalent Agent-visible Tool
calls.

Verification must use the six public Tool controls and cover Tool/process execution,
bounded Read, cursor replay, terminal delivery acknowledgement, output
coalescing, input ordering, idempotent transport retries, remote routing,
completion/cancel races, and foreground Tool adoption. Relevant suites include
`crates/rua-tool/tests/all/task_runner.rs` and
`crates/rua-server/tests/contract_chats_http/tasks.rs`.

## Related decisions

- ADR 0022: Conversation Authority routing.
- ADR 0023: Active Turn and input queue.
- ADR 0025: Task Runtime Notes.
- ADR 0026: Task output projection.
- ADR 0031: Durable Task and Attempt lifecycle.
- ADR 0033: ACP Runner projection and permissions.
