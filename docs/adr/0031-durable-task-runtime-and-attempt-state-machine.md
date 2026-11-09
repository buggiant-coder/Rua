# ADR 0031: Durable Task Runtime and Attempt state machine

**Status:** Accepted

## Context

Long-running work previously appeared in several overlapping mechanisms:
background Tools, Agent delegation, ACP execution, and workflow-specific task
systems. Each carried different identity, cancellation, retry, output, and
recovery rules. This duplication made it difficult to answer whether work
survives a Turn, who owns it, and whether retry can repeat side effects.

A Task is not every Tool Call. It is independently tracked work that may outlive
the Conversation Turn that created or awaited it. Different executors still
need typed behavior, but they should share one durable lifecycle and control
model.

ADR-0053 narrows Task to independently tracked background Tool work. Agent and
ACP collaboration execute as target Chat Turns, not Task Attempts. Existing
Agent/SubAgent/ACP adapters are implementation paths to remove, not supported
target Runner kinds. Tool execution retains the process-level runtime, Attempt
lifecycle, dependency graph, sessions, and storage.

## Decision

### D1 - Task is the tracked-work aggregate

A Task has one stable `task_id`, one owning Chat Address, one immutable Task
Spec, one lifecycle, one cancellation subtree, one Runner kind, and an ordered
sequence of Attempts.

Task creation records accepted work before launch and returns the `task_id`
without waiting for terminal completion. Waiting is an explicit bounded
`task_read` operation; there are no separate foreground and background Task
modes.

Task identity is distinct from Chat ID, Turn ID, Tool Call ID, ACP session ID,
process ID, and transport request ID. Those values may be stored as origin or
execution provenance but cannot replace `task_id`.

### D2 - One process-level Task Runtime owns all Tasks

Each Rua server process hosts one durable Task Runtime. It indexes Tasks by
owning Chat Address and Task ID, owns the Runner Registry, enforces process and
per-Chat concurrency, persists state, publishes activity, and performs startup
reconciliation.

Process-level ownership does not make Tasks globally visible. Lookup, control,
result access, and subscriptions remain scoped to the owning Chat Address.
Control state is stored transactionally in `data_dir/task-runtime.sqlite`;
large execution content is stored in the owning Chat's Task directory.

### D3 - Task Spec is immutable

The Task Spec records owner, origin, instruction, typed Runner selection,
context references, dependency IDs, and other execution intent accepted at
creation. It is not rewritten by retry or by a later Chat Agent selection.

Runner input names the real permitted Tool and typed arguments. Agent, Subagent,
and ACP conversation kinds are removed; metadata cannot select hidden executors.

### D4 - Attempt is the unit of execution

Each execution of the immutable Task Spec creates the next positive Attempt
number. The Attempt records authorization provenance, captured effective Chat
configuration epoch, execution Device provenance,
start and terminal times, state, error, bounded result, artifacts, and result
references.

Only one Attempt may be non-terminal for a Task. Terminal states are completed,
failed, cancelled, or interrupted. A completed Task is not retried; rerunning it
requires a new Task.

### D5 - Retry is explicit and side-effect aware

Failed, cancelled, or interrupted Tasks may be retried explicitly. Retry keeps
the same Task ID and Spec, increments the Attempt number, and uses the retrying
Turn's effective Chat authorization snapshot for the new Attempt. Execution
Agent identity is provenance, not permission to reload mutable Agent defaults.
Inherited configuration admission applies to new Attempts; running Attempts
retain their captured configuration.

There is no automatic retry after process loss or ambiguous failure because the
prior Attempt may already have produced side effects. Retry re-evaluates
dependencies and concurrency before launch.

### D6 - Durable control state and large execution data are separate

Task records and Attempt transitions use the Task Runtime's durable control
store. Large Runner output, Task context, full results, and artifacts use
Task-owned files and references rather than inflating SQLite rows or the parent
Chat transcript.

Each terminal Attempt produces one bounded Task Result and one typed terminal
Runtime Note for the owning Chat. The parent receives a durable summary and
references, not the complete execution transcript.

### D7 - Startup and shutdown reconcile non-terminal work

Runtime handles, subprocesses, ACP sessions, and watchers are not durable.
After restart, every previously non-terminal Attempt becomes `interrupted` with
recovery provenance. The runtime does not resume or replay it automatically.

Controlled shutdown stops accepting new work, requests cooperative cancellation,
waits for a bounded process-shutdown grace period, terminates remaining owned
execution, seals available output, and persists interruption. The grace period
is not a Task deadline.

### D8 - Task and Conversation remain separate aggregates

Conversation Authority owns Active Turn, input queue, Turn Writer, and Chat
terminality. Task Runtime owns independently tracked work. Waiting for a Task
does not convert it into an Active Turn, and viewer detachment does not cancel it.

Chat deletion cancels non-terminal owned Tasks and removes their control records
and Chat-owned Task directory. External or shared resources referenced by a
`TaskResourceRef` retain their own lifecycle and are not implicitly deleted.
Branching may carry terminal result references but never live execution.

Deleting a source Chat does not delete retained collaboration Chats or their
Tasks. Task controls are Create, Read, List, Cancel, SendInput, and Retry under
ADR-0032. The dependency graph contains Tool Tasks, not Chat requests or Agent
Turns. Result notifications are Tool observations, not user inputs.

## Hard invariants

- One Task has one immutable Spec and ordered Attempts.
- At most one Attempt is non-terminal per Task.
- Task ownership is the complete Chat Address.
- Tool execution is typed; Agent/Subagent/ACP conversations are not Task Runners.
- Retry is explicit and increments Attempt number.
- Restart never automatically replays non-terminal work.
- Every terminal Attempt has one bounded result.
- Task execution is distinct from Active Turn execution.
- Task creation returns after durable acceptance rather than waiting for Runner
  completion.
- Runner execution cannot recursively submit Tasks or mutate the Task graph.

## Authority, persistence, concurrency, and failure boundaries

**Authority.** Task Runtime owns Task lifecycle and Attempt transitions. The
owning Chat authorizes creation and control. Runner adapters own execution
mechanics but cannot rewrite Task identity or state directly.

**Persistence.** Control records are durable before Runner launch and after each
terminal transition. Large content is stored by reference. Runtime handles and
subscriptions remain memory-only.

**Concurrency.** Runtime enforces configured process and per-Chat limits before
launch. Dependency readiness is evaluated from terminal upstream Attempts.
Exactly one transition wins completion versus cancellation races.

**Failure.** Startup, protocol, process, or session failure becomes a truthful
Attempt outcome. Crash and shutdown become interrupted. Unknown side effects
prohibit implicit replay; callers may inspect and explicitly retry.

## Consequences

- Background Tool work shares one Attempt lifecycle; Agent and ACP conversation
  execution remains with Chat Authority, not another Task adapter.
- Parent Chats remain bounded because detailed Task output stays outside normal
  Conversation history.
- Durable records support inspection after Turn and viewer lifetimes end.
- Runner adapters must satisfy one common contract suite.
- Workflow orchestration can build on Tasks without becoming another runtime.

## Rejected alternatives

- Keep separate task systems for each Runner kind.
- Treat every foreground Tool Call as a Task.
- Use one untyped metadata payload to select execution.
- Restart a Tool Call when transferring it to Task Runtime.
- Automatically resume interrupted Attempts.
- Let Runner execution mutate the Task graph recursively.
- Make Task state part of frontend or Active Turn state.

## Implementation and verification

**ADR-0053 amendment gap:** current code still exposes Get/Await/Watch/Unwatch
and Agent/Subagent/ACP Runner adapters. Replace these with the six-Tool contract
and Chat collaboration; preserve dependency scheduling and explicit Retry.
Authorization must consume effective Chat snapshots. Existing adapter tests
are retirement coverage, not evidence that the amended contract is implemented.

Current implementation anchors include:

- `crates/rua-task/src/model.rs`;
- `crates/rua-task/src/runtime.rs`;
- `crates/rua-task/src/store.rs`;
- `crates/rua-task/src/session.rs`;
- `crates/rua-task/src/runner.rs`;
- `crates/rua-server/src/services/task_authority.rs`.

Verification must cover record-before-launch, immutable Spec, Attempt numbering,
completion/cancellation races, explicit retry, dependency blocking, concurrency
limits, startup reconciliation, controlled shutdown, result persistence, Chat
scope isolation, and deletion cleanup. Relevant suites include
`crates/rua-task/tests/unit_runtime/runtime.rs`,
`crates/rua-task/tests/unit_runtime/session.rs`, and
`crates/rua-server/tests/contract_chats_http/tasks.rs`.

## Related decisions

- ADR 0018: Chat Address ownership.
- ADR 0023: Active Turn boundary.
- ADR 0024: Stop and Tool transfer semantics.
- ADR 0025: Terminal Task Runtime Notes.
- ADR 0032: Tool Task Runner, bounded Read, and routing.
- ADR 0033: ACP Runner permission relay.
- ADR 0046: Chat deletion tombstones and storage accounting.
