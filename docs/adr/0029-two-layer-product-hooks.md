# ADR 0029: Two-layer Product Hooks

**Status:** Proposed

## Context

Rua needs user-configurable automation around a Chat's lifecycle and around
individual model and Tool operations. The extension model must work for Agent-
Scoped and Project-Scoped Chats, preserve Conversation Authority and security,
and avoid exposing mutable internal runtime objects as a public protocol.

The repository already contains meaningful Product Hook infrastructure:
`ProductHookService`, Agent/User, Plugin, and Project definition registration,
Project manifest discovery, scope composition, Session File persistence of a
materialized `hook_configuration`, in-memory Turn freezing, command/plugin
handler execution, candidate-completion integration, Tool-before and
Tool-observer services, permission-request integration, terminal observers, and
contract tests. These pieces prove several event boundaries and execution
mechanisms.

They do not yet establish that one complete, versioned Active Turn Hook and
Operation Hook product protocol is implemented across every Conversation
Driver, every configuration source, persistence path, ordering rule, failure
policy, and recovery path. This ADR therefore records the intended architecture
as Proposed rather than declaring the whole design delivered.

## Decision

### D1 - Product Hooks have two layers

The proposed product model has:

- **Active Turn Hooks**, which extend lifecycle boundaries of one durable Active
  Turn identified by Chat Address and `turn_id`;
- **Operation Hooks**, which extend canonical model-call, Tool-call, and
  permission-request boundaries reached while that Turn executes.

The internal `react-agent::AgentHook` remains an implementation SPI. It is not
the public product protocol and does not define cross-Driver semantics.

### D2 - Active Turn Hooks follow the durable Turn lifecycle

An Active Turn Hook begins only after Conversation Authority claims accepted
input and assigns a `turn_id`. Queued input is outside this scope. The scope ends
only after terminal state is durably committed and authority handoff completes.

Hooks cannot assign Turn identity, decide placement, commit terminal state,
advance the queue, or decide that a Chat is idle. They apply independently of
whether the Driver is built-in Agent, ACP, or another declared Driver.

### D3 - Candidate Completion is a pre-commit gate

A Driver may propose Candidate Completion before terminal commit. Applicable
Stop Hooks evaluate the same immutable proposal. Acceptance permits normal
terminalization; a valid continuation result keeps the same `turn_id` and adds
typed provenance-bearing context for a later Driver step.

Continuation is available only when the Driver declares a compatible versioned
completion capability. An accept-only Driver cannot silently ignore a Hook that
may request continuation.

### D4 - Operation Hooks bind to canonical operations

Operation Hooks attach to Rua-owned model-call and Tool-call boundaries, not
to one ReAct-loop implementation. Work performed entirely inside an external
runner is not reconstructed as synthetic Rua operations.

`tool.before` may deny or replace complete not-yet-executed arguments when its
event contract permits. Replacement is revalidated and passes normal Tool Guard,
File Guard, safety, and approval evaluation. `tool.after` may add typed feedback
but cannot rewrite the canonical Tool Result or its success/failure fact.

### D5 - Permission Hooks advise; Approval Authority decides

A `permission.request` Hook runs only after fixed policy requires approval and
before interactive approval. Its one-shot outcomes are Allow, Deny, or
NoDecision for the exact request and final argument digest.

Aggregation is Deny over Allow over NoDecision. Hook failure is NoDecision
unless the event contract explicitly fails the enclosing operation. Hooks
cannot create remembered Chat or Agent permission rules, bypass hard denials,
or replace Approval Authority.

### D6 - Configuration is materialized and ordered deterministically

Hook Definitions have qualified owner identity and immutable revisions. Agent
and Project selections are additive. Duplicate selection of the same qualified
revision executes once while retaining provenance.

The intended Chat configuration stores enabled revisions, parameters,
presentation policy, failure policy, and an explicit dependency DAG. Distinct
Hooks receive no implicit priority from Agent, Project, Plugin, discovery order,
map iteration, or completion timing.

An Active Turn captures one effective Hook set with its Chat configuration
epoch. Later configuration edits apply to later claimed Turns.

### D7 - Event capabilities and failures are bounded

Each event defines typed input, allowed output, aggregation, timeout, and
failure behavior. There is no universal mutable Hook context or universal action.

Launch failure, timeout, malformed output, protocol mismatch, cancellation, and
crash are execution failures, not implicit Allow, Deny, continuation, or success.
Post-commit observers cannot rewrite committed facts.

### D8 - Authority and security are non-Hookable

Hooks cannot replace or veto Chat Address, Authority Placement, Authority Epoch,
Turn claim, Turn Writer ordering, queue handoff, Stop, mandatory cancellation,
runtime limits, Tool Guard, File Guard, safety scanning, or durable Conversation
State ownership.

External command handlers use structured program, argument, working-directory,
timeout, stdin, and stdout contracts. Runtime Authority resolves device-local
launch details on the authority Device; Hook configuration does not persist
resolved executable paths.

### D9 - Current implementation is a foundation, not full conformance

The following are implemented foundations as of this decision:

- a process-local `ProductHookService` registry for native, user/Agent, Plugin,
  and Project definitions with revision records and enable/binding operations;
- Plugin declaration loading and `.rua/hooks.json` Project manifest discovery;
- deterministic Agent + Project + Chat binding composition, validation of
  dependency/order constraints, and deduplication of a selected definition
  revision;
- materialized `hook_configuration` persisted in Session File metadata, restored
  when a Turn is claimed, and frozen for that Turn in process memory;
- command and Plugin Handler execution with bounded timeout, cancellation,
  authority-device matching, and declared resource checks;
- wired events `turn.before_completion`, `turn.terminal`, `tool.before`,
  `tool.after`, `tool.failed`, and `permission.request`;
- Candidate Completion continuation for the Rua Driver and explicit
  `accept_only` compatibility rejection for the ACP Driver;
- contract tests for each of these concrete services and for Chat configuration
  materialization/update behavior.

The following remain proposed until demonstrated end to end:

- one public, versioned two-layer protocol and event envelope; the current event
  strings and service-specific outcome JSON are implementation contracts, not
  that completed protocol;
- the complete Active Turn event catalog, including preparation/error behavior,
  and canonical model-call Operation Hooks; current code covers Candidate
  Completion, terminal observation, Tool boundaries, and permission requests;
- uniform lifecycle support across all Conversation Drivers; ACP is currently
  `accept_only` at Candidate Completion and cannot run continuation-capable Stop
  Hooks;
- the single durable Chat Configuration Snapshot/current-head protocol across
  Harness, Hooks, model, and reasoning, with concurrent heads resolved by the
  existing HLC rules; the current Hook epoch is a generated
  `hook-config-<uuid>` inside the persisted Hook snapshot;
- durable definition/default storage and durable Hook Activity. Definitions,
  activations, frozen Turn snapshots, run records, and contribution records are
  currently held by `ProductHookService` in process memory, while only the
  materialized Chat Hook configuration is restored from Session File metadata;
- restart/idempotency behavior for synchronous gates and a durable post-terminal
  observer outbox;
- proof that every legacy/internal Hook path is either behind these boundaries
  or cannot bypass Conversation Authority, Context Projection, Approval
  Authority, and fixed security checks.

## Hard invariants

- Product Hooks never become Conversation or Approval Authority.
- Active Turn Hooks require an existing durable `turn_id`.
- Operation Hooks attach only to canonical Rua operation boundaries.
- Canonical user input and committed Tool Results are not rewritten afterward.
- Hook-produced argument changes are fully revalidated.
- Hook failure is not silently converted into a business decision.
- One Active Turn uses one frozen effective Hook set.
- Implemented infrastructure must not be described as full protocol completion.

## Authority, persistence, concurrency, and failure boundaries

**Authority.** Conversation Authority owns Turn lifecycle; operation owners own
canonical model and Tool actions; Approval Authority owns approval facts.
Product Hooks can contribute only the outcomes granted by a typed event.

**Persistence.** The target design gives definitions, revisions, defaults, Chat
selection, and material Hook Activity durable owning stores. In the current
foundation, `hook_configuration` is persisted in the Session File, while
definitions/default activations, frozen Turn entries, runs, and contributions
are process-local. Hook output is not automatically Conversation State unless
an event defines a typed durable record.

**Concurrency.** Dependency order is deterministic. Mutators run serially over
validated candidates; independent observers may run concurrently but report in
resolved order; event-specific gates define aggregation.

**Failure.** Every invocation has bounded timeout and explicit failure outcome.
Unknown side effects prevent automatic retry. Missing capability, invalid
configuration, or Driver incompatibility fails explicitly rather than disabling
the Hook silently.

## Consequences

- Existing Product Hook services can evolve behind a coherent product boundary.
- Extension authors receive narrower, auditable capabilities than middleware.
- Agent and Project Hook sources can compose without source-based precedence.
- Drivers need explicit completion capabilities for continuation semantics.
- Acceptance of this ADR does not occur until implementation and tests prove the
  complete two-layer contract.

## Rejected alternatives

- Declare the current service infrastructure as the completed Hook product.
- Expose `react-agent::AgentHook` directly as the public protocol.
- Provide one mutable context and action type for every event.
- Order Hooks implicitly by source or completion timing.
- Let Hooks replace fixed safety or authority mechanisms.
- Dynamically read current defaults during an Active Turn.
- Reconstruct external-runner operations that Rua does not own.

## Implementation and verification

Implemented anchors include:

- `crates/rua-server/src/services/product_hooks.rs`;
- `crates/rua-server/src/services/plugin_hook_definitions.rs`;
- `crates/rua-server/src/services/project_hook_manifest.rs`;
- `crates/rua-server/src/services/candidate_completion.rs`;
- `crates/rua-server/src/services/product_tool_before.rs`;
- `crates/rua-server/src/services/product_tool_observer.rs`;
- `crates/rua-server/src/services/product_permission_request.rs`;
- `crates/rua-server/src/services/terminal_observer.rs`.

Current contract coverage under
`crates/rua-server/tests/contract_product_hooks_http/` verifies parts of this
foundation, including composition, persisted Chat configuration restoration,
handler execution, Candidate Completion, Tool mutation/observation, permission
aggregation, and terminal observers. Full acceptance requires public protocol
versioning, model-call and remaining lifecycle events, HLC configuration-head
tests, durable definition/default and Hook Activity tests, restart/outbox
behavior, broader cross-Driver coverage, and negative tests proving Hooks cannot
bypass authority or security.

## Related decisions

- ADR 0023: Active Turn ownership.
- ADR 0024: Stop and interjection.
- ADR 0025: Typed Runtime Notes.
- ADR 0026: Context Projection ownership.
- ADR 0028: Chat configuration and Turn snapshots.
- ADR 0033: ACP permission relay boundaries.
