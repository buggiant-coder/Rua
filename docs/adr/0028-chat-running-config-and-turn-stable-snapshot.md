# ADR 0028: Complete Chat configuration snapshots and inherited authorization

**Status:** Accepted

## Context

Agent configuration supplies useful defaults, but a materialized Chat needs
predictable behavior over time. If every Turn rereads mutable Agent defaults,
editing one Agent changes old Chats without an edit to those Chats. Project
scope previously introduced another possible configuration layer, which made
precedence dependent on where a Chat lived rather than what the Chat owned.

Configuration also cannot change halfway through an Active Turn. A Turn may
perform several model calls, Tool calls, retries, and projections. Resolving
settings independently at each step would make one Turn internally inconsistent.

The current API and control view expose Chat-addressed running configuration,
while session storage owns the complete configuration snapshot and current head.

## Decision

### D1 - Agent Running Config is a creation-time template

Agent Running Config defines defaults for newly materialized Chats. On first
materialization, Rua copies the complete set of Chat-owned runtime options
from the assigned Agent into the Chat.

Later Agent default edits affect future Chats only. Existing Chats do not retain
a live inheritance relationship with the Agent template.

Turn execution, history-replacement recovery and model-switch preflight resolve
behavior exclusively from the captured or persisted Chat configuration. The
running-config projection accepts no Agent/default resource input. Preflight
also uses the Chat's current model, not the global default model. Missing or
invalid Chat configuration is an error, not a reason to load Agent defaults.
Workspace-shared memory infrastructure is initialized separately; its credentials
are not Turn behavior and are not copied into portable Chat snapshots.

Agent Running Config is device-local Agent configuration and is not synchronized
as Chat state. Only the complete Chat-owned copy created at materialization is
portable.

### D2 - Chat Running Config is complete and Chat-owned

Every materialized Agent-Scoped or Project-Scoped Chat owns one complete Chat
Running Config. It contains every option exposed as Chat runtime behavior and
consumed per Turn.

The config is not a sparse override. Execution never overlays current Agent
defaults onto a saved snapshot. New Chat creation follows D9 and completes before
execution. Materialize Harness,
Hook revisions, model/reasoning, Tool Selection Policy and external source selections,
security policy and environment binding in one complete immutable snapshot; a coherent
current head identifies its epoch. Under ADR-0058, complete Tool configuration freezes
selection intent, not an expanded inventory of the current product catalog.

### D3 - Both Chat scopes use the same rule

For an Agent-Scoped Chat, the fixed scope Agent supplies the initial template.
For a Project-Scoped Chat, the Agent selected at first materialization supplies
the template.

An explicit normal Project Chat Agent switch atomically materializes the new
Agent's complete contribution in a new epoch. It does not change Project resource
binding. Collaboration Chats instead fix executor identity at creation; changing
executor requires a new Chat (ADR-0053).

### D4 - Lazy Chat captures config at materialization

A Lazy Chat has no durable running configuration. Opening a route or changing a
browser draft option does not freeze defaults.

The first accepted stateful operation atomically materializes the Chat and its
complete running configuration. Branching creates a new Chat and copies the
source Chat Running Config to preserve behavioral continuity.

### D5 - Turn claim creates an immutable effective snapshot

When Conversation Authority claims an input and creates the Active Turn, it
captures the complete Chat configuration epoch, including effective authorization.
Every model call, Tool call, retry, context projection, and runtime decision in
that Turn uses this same immutable value. ADR-0058 permits product catalog and
Device-readiness changes to affect later Model Call capability snapshots under
that frozen policy; it does not permit changes to the claimed configuration or
authorization, or replacement of an already captured Tool executor.

An edit while a Turn is active does not restart, mutate, or interrupt that Turn.
It applies when a later queued or newly submitted input is claimed.

The complete Effective Turn Running Config is runtime state. Durable Turn
history records the compact Agent/model/thinking provenance required by its
own contracts rather than serializing a second full copy of
Chat configuration into every Turn.

### D6 - Complete immutable snapshots and a coherent current head

Chat storage owns immutable complete snapshots and a separately identified
current head/epoch, written atomically. Core fields are typed and
validated; unknown fields are preserved. Materialization completes before the
Chat can execute. Exact storage layout follows the session publication contract.

The snapshot is portable Conversation State and synchronizes with the Chat.
Skill configuration records the selected name, description, source and resource-relative
directory, not the Skill body. Agent and Project Skill bodies are read from their
resource at runtime and enter model context through the `skill` Tool Result.
Existing Tool Results retain the content actually loaded; configuration does not
duplicate it or freeze a second copy. A missing or out-of-root resource is an
explicit loading error, not permission to substitute another Skill.
Device-local executable paths, credentials, runtime inventory, and other
non-portable infrastructure values are excluded.

Chat model selection shown by history responses and live control views is a
projection of this same current configuration, including the separate Thinking
selection. Other Session fields cannot override a complete snapshot or fill an
intentionally absent selection. A previously claimed Active Turn keeps its captured model, while a
later Turn uses the current configuration.

### D7 - Explicit updates publish epochs and inherited configuration

Chat configuration APIs read and replace the complete Chat Running Config.
Agent configuration APIs edit only the template for future Chats.

A Chat may explicitly replace its config with a fresh copy of current Agent
defaults. That is a one-time copy, not restored inheritance. Route naming and
response shape must not imply Chat ownership while mutating Agent-wide state.

UpdateChatConfiguration validates all categories and atomically publishes a new
epoch. Source Chat security/environment changes propagate to its collaborations;
child executor-specific model/prompt/Tools are not replaced by the source's.
Children must confirm the inherited revision before new execution. Do not solve
divergence by extra permission-difference approvals or merging two profiles.
Active Turns keep their captured epoch; update admission and new execution are
coordinated. Deleting the source leaves retained children with their last
materialized configuration, not a live Agent-default dependency.

### D8 - Current configuration must be complete

A materialized Chat reads one complete configuration snapshot. Missing snapshots,
invalid data and unresolved heads fail through normal validation. Reads never
reset settings or fill missing fields from current Agent defaults.

### D9 - Creation materializes the complete configuration

Ordinary Agent/Project Chat operations share the configuration capture module.
The operation supplies its captured Agent assignment, running configuration,
Project resource root and explicit Chat selections. Capture gathers security,
capability, tool configuration and prompt content and returns materialization
inputs; it does not persist, publish or adopt a configuration. Those effects
remain with the existing operation and its authority/commit ordering. This shared
assembly is not a cross-store transactional read guarantee.

Create the complete snapshot and head during Chat creation on its Authority
Device. A Lazy Chat waits for its first accepted stateful operation. Project
draft creation also captures the complete configuration, and branching inherits
the source configuration. Running-config APIs project or replace the running
section within that complete snapshot; there is no standalone running file.

For an Agent Lazy Chat, both model and target Device remain local draft choices.
Neither choice creates a Session or transfers Authority. Sending input, uploading
media or entering/resuming ACP first calls the shared materialization endpoint.
The selected Device creates the complete configuration and initial Authority
placement directly; a remote controller then obtains that Session through the
existing history transfer before proceeding. Repeating creation on that Device
returns the existing configuration, never recaptures defaults. Failure retains
the draft. Once creation is sent, its target/model selection stays locked until
the outcome is confirmed, including across a reload: timeout is not proof the
target did not create the Chat. The same target can be retried idempotently.

Only a materialized Chat may use the Authority migration endpoint; a zero-message
Chat is still materialized once it has its complete snapshot. A model-only header
is not a materialized Chat. Subsequent input reads the existing Chat without
rewriting its scope or rebuilding its defaults. Project resource binding remains
fixed and is not an Agent draft target selection.

Session replication must not roll back a confirmed Authority placement. The
publication HLC orders Session snapshots, not authority transfers: a delayed
publication may carry an older authority epoch. Under the Session write lock,
inbound snapshots retain the local placement unless the incoming authority epoch
is strictly higher. Equal epochs retain the already confirmed placement. This
does not relax control-command epoch validation or rewrite configuration epochs.

An explicit **Update Configuration** remains a preview/confirm operation, bound
to the exact previewed current value and epoch. Configuration replacement and
Turn claim are serialized. Reading options never writes configuration. ADR-0058
requires the current Tool policy format at every configuration write boundary;
normal reads/execution do not detect-and-repair historical formats or
apply field-wise default overlays. Stored conversation history is preserved.

### D10 - Configuration selection is independent of live tool discovery

Reading, initializing, previewing or saving Chat configuration performs only
static validation. It never starts an MCP connection, waits for startup, calls
`tools/list`, or probes a plugin. MCP management and connection diagnostics
remain on the Agent MCP surface, not in Chat runtime options.

Capability configurations copy the Agent's explicit Tool Selection Policy,
external MCP server identities and disabled-tool selections, registered plugin
identities, and local Tool/Skill settings. These are declarations, not callable
schemas. No transport command, environment, URL, credential, connection state or
discovered schema is saved in those selections. An offline selected source can
recover without replacing Chat configuration. A newly configured external source
is not adopted by an existing Chat until an explicit configuration update.

Rua-owned product capabilities follow ADR-0058: Default-Enabled selects newly
provided product Tools unless disabled, including bundled Browser/Computer and
native Phone Use on supported execution Devices. Restricted requires exact allowed
Tool identities. Catalog growth under a saved policy does not reread Agent defaults,
rewrite Chat epochs, or automatically enroll arbitrary external sources.

At the Model Call Boundary, resolve that frozen policy and source selection against
the live registry to produce both exposed schemas and executable tools. Unavailable,
disabled, or not-allowed Tools are excluded; selection does not grant permission.
Match MCP Tools by their registered server identity and original Tool name, not
ambiguous string-prefix wildcards. Old exact inventories are not implicitly empty
blacklists. ADR-0058 requires a one-time data cutover outside the application;
old-format compatibility is not retained.

## Hard invariants

- Every materialized Chat owns one complete running configuration.
- Agent defaults are copied, not inherited dynamically.
- Agent and Project scopes use the same Chat config model.
- No Project Running Config layer exists.
- One Active Turn uses one immutable configuration/policy snapshot; each Model Call
  resolves its own immutable capability snapshot from that policy.
- Mid-Turn edits never change in-flight behavior.
- Immutable complete snapshots and current head are portable Chat state;
  device-local credentials and resource paths are resolved by explicit bindings.
- Agent Running Config remains device-local template state.
- The full Effective Turn Running Config is not duplicated into Turn history.
- Device-local paths and credentials never enter the snapshot.

## Authority, persistence, concurrency, and failure boundaries

**Authority.** Chat-addressed configuration changes route to Conversation
Authority. Agent services own templates. Turn execution reads only the snapshot
captured at claim time.

**Persistence.** Complete config is atomically stored beside the Session File
and synchronized with the Chat. Chat Index may project list-visible summaries
but cannot become configuration authority.

**Concurrency.** A config replacement and Turn claim serialize through the Chat
authority boundary. The Turn observes either the old complete config or the new
complete config, never a field-wise mixture.

**Failure.** Invalid candidates are rejected before replacement. Source/child
publication has recoverable version evidence and blocks new stale execution
until coherent. Missing or invalid snapshots fail closed without default fallback.

## Consequences

- Existing Chats remain stable when Agent defaults change.
- Explicit Project Agent switches publish complete epochs, not silent overlays.
- Config changes have a clear next-Turn activation point.
- Runtime construction becomes simpler because it consumes one complete value.
- Configuration duplication per Chat is intentional and portable.

## Rejected alternatives

- Sparse per-field Chat overrides over live Agent defaults.
- A Project-level configuration layer between Agent and Chat.
- Re-resolve Agent or Project config for every model call.
- Apply edits immediately to an Active Turn.
- Store the complete config in Chat Index.
- Persist device-local resolved executable paths.
- Keep hidden legacy precedence after migration.

## Implementation and verification

**ADR-0053 amendment gaps:** ChatRunningConfig::resolve currently clones Agent
defaults and overwrites selected fields. Complete security snapshots, epoch/head
storage, and inherited child publication/admission must be implemented and tested.
A configuration_epoch test name or running_config JSON is not proof of these
semantics. Existing implementation anchors below are not a conformance claim.

Current implementation anchors include:

- `crates/rua-config/src/types/agents.rs`;
- `crates/rua-config/src/types/session.rs`;
- `crates/rua-session/src/store.rs`;
- `crates/rua-server/src/services/console_chat.rs`;
- `crates/rua-server/src/services/conversation_authority.rs`;
- `console/src/components/RunningConfigOptions/index.tsx`.

### Current implementation gaps

ADR-0058's policy representation, current-format Agent/Chat edits, and product
catalog growth now use the same configuration epoch and capability snapshot paths.

The current storage uses immutable complete snapshots and a current head.
Running-config reads project that snapshot, and writes publish a new epoch.
New Agent/Project Chats and branches must supply the complete configuration;
there is no standalone running-config writer, migration initializer, or sync record.
This storage cleanup does not itself prove every execution/backfill path captures
configuration at the correct boundary; the claim-time requirements above remain
mandatory and require their own tests.

Verification must cover first materialization, both Chat scopes, Agent-default
changes, Project Agent switching, branch copy, concurrent edit and Turn claim,
mid-Turn immutability, atomic write failure, and
sync round trip. Relevant suites include
`crates/rua-server/tests/infra/session_context_snapshot.rs`,
`crates/rua-server/tests/infra/project_chat_materialization.rs`, and
`crates/rua-config/tests/unit_agents_config/running.rs`.

## Related decisions

- [ADR 0058: Tool selection policies](0058-tool-selection-policies-and-default-enablement.md).
- ADR 0018: Lazy Chat materialization.
- ADR 0019: Project-Scoped Chat Agent selection.
- ADR 0020: Session storage boundaries.
- ADR 0023: Turn claim and Active Turn.
- ADR 0026: Per-model-call Context Projection.
- ADR 0029: Chat-materialized Hook configuration.
