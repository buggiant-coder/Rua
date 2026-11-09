# ADR 0026: Context Projection owns model-call assembly

**Status:** Accepted

## Context

Model-visible input can originate from durable Conversation State, current
Agent instructions, Project instructions, runtime environment, capabilities,
Skills, running configuration, Runtime Notes, and one-call guidance. If Console
Chat, Agent runtime, Hooks, and provider adapters each modify a message list,
there is no single authority for order, role, token measurement, cache identity,
or replay behavior.

Evergreen Chat already owns durable history through Message Log, selected
variants, Summary Checkpoints, retained ranges, and History Replacement. It
should not be paired with a second prompt-history system hidden inside runtime
Hooks or provider-specific request builders.

The current implementation has dedicated projection preparation in
`rua-agent` and server-side context projection services. This ADR defines the
ownership boundary those paths must preserve.

## Decision

### D1 - Every conversation-driving model call captures Model Call Context

At each conversation-driving Model Call Boundary, runtime captures one immutable
typed Model Call Context. It contains the current inputs needed for that call,
including:

- Base Agent Contract and Agent prompt context;
- Project Instructions and Workspace context when applicable;
- collaboration or execution mode;
- Execution Environment and path contract;
- Effective Capability Snapshot and Tool schemas;
- selected model, reasoning, and provider options;
- Turn-stable running limits;
- current user input and the configured context/output reserve;
- optional one-call runtime guidance.

These values are call inputs, not automatically durable Conversation Messages.

### D2 - Context Projection is the sole conversation-request assembler

Context Projection is the only component allowed to produce the normalized
request used to continue an Active Turn. It combines Model Call Context with
selected durable Conversation State, applies canonical history transforms,
chooses a valid projection plan, renders ordered roles and Tool schemas, and
measures the final assembled input.

Other subsystems contribute typed values. They do not receive or return a
complete mutable provider request and do not choose their own final role or
position.

### D3 - Durable history and current context remain distinct

User, Assistant, Tool Result, and model-visible Runtime Note records come from
durable Conversation State. Current System and Developer guidance, capability
catalogs, execution environment, and call-only guidance are rebuilt for each
model call.

Summary Checkpoints and History Replacement summarize or substitute only
durable Conversation State. They never absorb the current Base Agent Contract,
Tool schemas, Project Instructions, or other current-world inputs.

### D4 - Projection applies canonical transforms in one order

Projection resolves selected variants, the durable Projection Exclusion Set,
active checkpoints, retained raw messages, valid History Replacements, Tool
Result truncation, typed Runtime Note visibility, and emergency bounded
fallback before provider serialization.

Second-pass Tool Result truncation may advance in count-based batches through
`tool_result_compact.projection_truncation_batch_size` (positive integer,
default `1` for the existing rolling behavior). For a batch size greater than
one, let `C` be the number of completed Tool Use/Result pairs in the effective
history after replacement and `B` the batch size. The boundary is
`floor(C / B) * B`; truncate results before that boundary's `recent_n` window,
keeping both that window and all results after the boundary intact. Parallel
Tools count individually; model calls, retries, and wall time do not count.
Thus an unchanged append-only history has a stable truncation boundary between
batches, without a timer, process-local cursor, or another durable state file.
History edits, exclusions, checkpoint/replacement changes, and explicit config
changes rebuild the boundary from the newly effective history; cache reuse may
not override their correctness. Final input measurement, checkpoint source
projection, execution-time output limits, and raw Message Log ownership retain
their existing contracts. This does not enable earlier Compression or raise the
context limit; larger batches can retain more content between boundaries.

Projection Exclusions omit selected committed Assistant, reasoning, Tool Use,
and Tool Result Messages from future model calls without changing the Message
Log or Conversation UI Projection. User Messages cannot be excluded. A Tool Use
and its Tool Result group is excluded as one protocol unit even when the user
selects only one member.

A Summary Checkpoint or History Replacement is applicable only when its source
coverage remains valid under the active Projection Exclusion Set. If it is not
applicable, Projection must use an exclusion-aware replacement/checkpoint or
reconstruct a view that preserves every non-excluded durable fact. It must not
drop an unrelated prefix merely because one later Message is excluded.

Tool Use and Tool Result closure defines safe history boundaries. The projector
must not split an incomplete tool group, emit an unmatched result, or include
activity-only delegated execution as parent model context.

### D5 - Measurement covers the final current input

The Projection Planner measures the fully assembled current request, including
current input, current guidance, and visible Tool schemas. The Context Target
Band applies its explicit reserve and maintenance thresholds;
`max_input_length` (bounded by the selected model's known context window) is the
hard input ceiling. Callers and providers must not add a second undocumented
margin or silently truncate the normalized request.

After compaction or fallback, Projection rebuilds and remeasures the request.
No unchecked over-limit request may reach a provider.

### D6 - Provider adapters serialize but do not make prompt policy

Provider adapters map the normalized request into provider wire formats and
stream results. They may perform protocol-required role or envelope encoding,
but they cannot add, remove, reorder, summarize, or reinterpret model-visible
content.

Provider-specific synthetic call/result pairs for asynchronous Tool observations
are projection output, not new canonical Conversation Messages.

Collaboration replies use this observation path with stable request/result
identity, not user-role input or child transcript forwarding (ADR-0053).
Direct yield results, deferred delivery, and Read compete on common delivery
evidence; a previously completed provider call does not receive a second result.
chat_read progress is a bounded on-demand cached summary of public activity,
never paginated child Tool history. task_read output retains deterministic
truncation and references rather than using the Agent summary model.
Projection consumes the claimed complete Chat configuration epoch, not mutable
Agent defaults; late results cannot mutate an already issued provider request.

### D7 - Inspection observes the actual request boundary

Prompt inspection captures the normalized request after Context Projection and
before provider invocation. The default diagnostic snapshot records structure,
section identifiers, token estimates, model, and Tool identities without message
bodies. Explicit local diagnostics may opt into content.

Inspection cannot become a parallel request builder or modify the captured
request.

### D8 - Bounded maintenance calls are explicit exceptions

Summary Checkpoint and History Replacement generation are maintenance model
calls over a runtime-selected durable history range. They may construct their
own fixed, no-Tool request instead of using the conversation Context Projection,
provided that they:

- do not continue the Active Turn or present their output as assistant speech;
- do not inject current Agent, Project, capability, Skill, or execution
  environment sections into the content being summarized;
- use a dedicated bounded prompt and output limit;
- carry Chat/model-call identity and diagnostic metadata; and
- publish output only through the owning checkpoint or replacement quality and
  authority boundary.

No other caller may treat this exception as permission to assemble an
independent conversation request.

Summary Checkpoint input preserves the selected projected Messages' roles,
content parts, and complete Tool Use/Result groups through `ContextSummaryRequest`
and the summary generator. The normal provider adapter serializes that history;
the maintenance request adds fixed compaction instructions and the previous
checkpoint, with no callable Tools. Internal `raw_input` and metadata must not be
flattened into a second text transcript. The summary budget measures this complete
request, including its instructions, and does not reuse a conversation request's
token-calibration offset. A failed blocking summary retains a bounded suffix of
complete message groups, the active user request, and the latest work plan.

## Hard invariants

- Exactly one component assembles model-visible input that continues an Active
  Turn.
- Every conversation-driving model call uses an immutable Model Call Context.
- Maintenance model calls are limited to the typed Summary Checkpoint and
  History Replacement contracts.
- Current guidance is rebuilt and is not compressed as conversation history.
- Durable storage does not imply model visibility.
- Projection Exclusions never mutate durable Messages or hide them from normal
  Conversation history.
- User Messages cannot be excluded, and Tool Use/Result closure is preserved.
- Provider adapters do not own prompt policy.
- Projection never emits an unmatched Tool protocol boundary.
- Final token measurement occurs after all transforms and schemas are applied.
- Current user input and the configured reserve participate in fit decisions.
- An over-limit request is never sent unchecked.

## Authority, persistence, concurrency, and failure boundaries

**Authority.** Context Projection owns request composition. Conversation State
owns durable facts, runtime services own current typed inputs, Tool Disclosure
owns schema visibility, and providers own only wire serialization.

**Persistence.** Model Call Context is normally runtime state. Durable Messages,
Projection Exclusions, checkpoints, replacements, and typed notes remain in
their canonical stores. Provider-ready request blobs are not persisted as
conversation truth. Optional LLM Debug Capture is device-local diagnostic data
outside the Session and is never synchronized.

**Concurrency.** Each Model Call Boundary captures one coherent snapshot. A
mid-Turn configuration or capability change affects a later boundary according
to its owning contract, never the request already in flight.

**Failure.** Missing required context, invalid projection, unsafe Tool grouping,
or input overflow fails before provider invocation. Compaction and emergency
projection are bounded recovery paths; provider serialization cannot silently
drop content to make a request fit.

## Consequences

- Prompt ordering and token accounting become inspectable and testable.
- Hooks and runtime services need typed contribution APIs instead of arbitrary
  message mutation.
- Current capability and environment changes appear at their permitted model
  boundary without rewriting history.
- Provider integrations become thinner and more consistent.
- Projection planning remains responsible for combining compression,
  replacement, and Tool-result bounds.

## Rejected alternatives

- Let each caller assemble its own provider message list.
- Keep unrestricted before-model-call Hooks that mutate `ChatRequest`.
- Build one static System prompt at Turn start for all later model calls.
- Persist provider-ready requests as Conversation State.
- Put current capabilities into Summary Checkpoints.
- Let providers silently truncate or reorder input.
- Use message-count side channels to identify current prompt sections.

## Implementation and verification

Current implementation anchors include:

- `crates/rua-server/src/services/context_projection.rs`;
- `crates/rua-agent/src/llm_summary.rs`;
- `crates/rua-agent/src/projection_preparation.rs`;
- `crates/rua-agent/src/projection_tool_results.rs`;
- `crates/rua-session/src/store.rs`;
- `crates/rua-session/src/store.rs::ProjectionExclusions`;
- `crates/rua-session/src/history_replacement.rs`;
- `crates/react-agent/src/react_loop.rs`.

The current Summary Checkpoint and History Replacement paths are the D8
exceptions. `crates/rua-agent/src/llm_summary.rs::summarize_with_context` and
`generate_history_replacement` build dedicated `ChatRequest` values and invoke
the provider directly. They attach model-call identity metadata when available,
but do not pass through
`crates/rua-server/src/services/context_projection.rs`; they must remain
bounded maintenance requests rather than become alternate Active Turn prompt
assemblers.

The existing projection suites exercise Active Turn request assembly,
Projection Exclusions, and checkpoint/replacement consumption. They do not
assert that the maintenance builders remain no-Tool, omit current-world
sections, and cannot be reused as an alternate conversation request path.

### Current implementation gaps

- `WorkspaceSessionStore::load_projection_exclusions` trusts the stored
  `format_version`, embedded `chat_id`, and Message IDs. Local HTTP updates
  validate ownership, reject User Messages, and expand Tool groups, but inbound
  Sync or an external file edit can bypass those checks. Load/apply or final
  projection must fail closed against malformed or forbidden exclusions.
- The current implementation disables every Summary Checkpoint and History
  Replacement whenever any Message is excluded, but then begins raw reads at
  the existing `projection_boundary`. If that boundary follows a compacted
  prefix, unrelated non-excluded context represented only by the checkpoint can
  disappear. The fallback must preserve that prefix, rebuild an exclusion-aware
  checkpoint, or reject an exclusion it cannot apply safely.

Verification must cover deterministic section order, selected variants,
Projection Exclusions, stored-file validation, rejection of user-message
exclusion, grouped Tool closure, exclusion interaction with an existing
checkpoint and replacement, preservation of unrelated prefix context, typed
note visibility, single-Tool output limits, exact final measurement, provider
adapter fidelity, inspection at the real invocation boundary, and negative
checks that the two maintenance request builders cannot acquire
conversation-driving context or Tools. Relevant suites include
`crates/rua-server/tests/unit_context_projection/message_log_projection.rs`,
`crates/rua-session/tests/all/projection_read_path.rs`,
`crates/rua-agent/tests/unit_context/history_replacement_checkpoint.rs`, and
`crates/rua-server/tests/console/system_context_accounting.rs`.

## Related decisions

- ADR 0020: Session File and Message Log.
- ADR 0025: Runtime Notes.
- ADR 0027: Durable History Replacement.
- ADR 0028: Turn-stable running configuration.
- ADR 0029: Product Hook boundaries.
- ADR 0032: Task Watch projection.
