# ADR 0053: Collaboration Chats and background Tool Tasks

**Status:** Accepted

## Reading guide

This record preserves the confirmed design decisions with stable D-numbers.
Later clarifications are incorporated into earlier clauses; D23 records the
rejected writable-Subagent alternative, while D24 defines its replacement.
The target design has no separate Rua Subagent executor.

| Topic | Decisions |
|---|---|
| Chat ownership, creation, executor, and context | D1, D10-D12, D14, D16, D23-D25, D30 |
| Input, outcomes, yield, recovery, and clarification | D2-D4, D9, D13, D21-D22, D31 |
| Caller wake, Stop, resumption | D5-D8, D35 |
| Parallel work, Turn release, visible background waiting | D35 |
| Human navigation, retention, and intervention | D17-D19 |
| Bounded progress inspection | D15, D20 |
| Authorization, snapshots, inherited configuration | D26-D28 |
| Resource admission and Task simplification | D29, D32-D33 |
| Compact model-facing Tool observations | D34; ADR-0010, ADR-0032 |

The protocol mechanics cover request/result identities, yield handoff,
result-driven execution, and configuration propagation gates. These are
proposed implementation designs, not a claim of existing support.

## Context

Long-running Tool Tasks and collaboration Chats have unpredictable completion
times. The source Agent cannot reliably estimate those times and must not act as
a scheduler by repeatedly reading, sleeping, or blocking until work finishes.
Ending a source Turn releases model execution; it does not prove the user's work
is complete. Background results, not model-managed waiting, drive continuation.

Agent delegation currently uses Task Runner Sessions for conversation history,
steering, and final results. Extending that surface into bidirectional Agent
communication would make a Task both an execution aggregate and a conversation.
Wrapping a Chat Turn in a Task Attempt would instead create competing lifecycle
and cancellation owners for the same Agent work.

## Confirmed design decisions

### D1 - Collaboration execution belongs to the target Chat

Each collaboration with a configured Agent or an ACP runner executes
through the target Chat's Conversation Authority and Active Turn. It does not
create a corresponding Agent Task or Task Attempt. Messages continuing the same
collaboration reuse its Chat rather than creating one Chat per invocation.

Sending from another Chat is asynchronous relative to the caller; that fact does
not make the recipient Turn a Task. Finishing the sender's Turn does not by itself
terminate the recipient's Turn. Stopping recipient execution is a Chat Turn
control, not a second Task cancellation path.

Tasks remain independently tracked background Tool work. A collaboration Chat
may own Tasks for such work, including a long-running test command. Shared
execution primitives and resource limits may be reused without adding a second
durable lifecycle to the Agent Turn.

### D2 - Requests to a target Chat reuse input and interjection

Requests sent by another Agent enter the target Chat through the existing
Conversation Authority input path. They reuse the Conversation Input Queue,
claiming, passive interjection, explicit Send Now, and Stop semantics defined
by ADR-0023 and ADR-0024. Sender identity does not introduce a different Turn
lifecycle or a parallel Agent mailbox. This describes request delivery to the
callee, not result delivery back to the caller; results follow D4.

Normal input to a busy Chat uses the existing queue and passive interjection
rules; it is not injected into the continuing Turn under a new same-Turn
steering protocol. Explicit active interjection targets an accepted `input_id`
and follows the existing control checks. Idle input follows ordinary Chat
acceptance and Turn claiming, including existing queue pause rules.

Queue persistence, restart behavior, and authority handoff also retain their
existing contracts. Agent-to-Agent routing must integrate with this input path,
not replace it. This decision does not claim that the current transport already
supports cross-Chat sender identity or replies.

### D3 - Final replies return to the requesting Chat automatically

Each cross-Chat request records its source Chat Address and request identity.
The recipient Turn's final reply returns automatically to that request's source
Chat, without requiring a recipient `chat_send` call or caller polling. Routing
is per request, not a broadcast to all participants or a lookup by Agent identity
alone. The returned content is the final reply, not the recipient's internal
Tool transcript or complete execution history.

The recipient returns clarification questions through the current invocation's
reply under D31, not a reverse `chat_send` to the caller. Automatic reply arrival and the
source Chat's decision to execute are distinct: D4 defines result projection
and D5 defines wake-up. Non-success and interjected outcomes follow D9 and D13.

### D4 - Returned replies are Tool results, not conversation inputs

The calling Agent is still serving the user in its source Chat. The callee's
reply is a result of the caller's Tool invocation, not a new user request or an
ordinary input submitted by another participant. Return delivery therefore
projects the reply as a Tool result/observation associated with the originating
request and Tool invocation. It does not enqueue the reply as Chat input or
apply interjection merely because the reply arrived.

Automatic return in D3 means result delivery, not automatic input submission or
a new reciprocal request. Source and callee Chat execution remain independent;
Tool-result projection does not require an Agent Task wrapper.

For asynchronous delivery, reuse the established runtime-note and valid paired
Tool-protocol projection approach where applicable. Do not append an unmatched
Tool result or duplicate a result against a provider call ID that already has
its submission receipt. Exact receipt/result projection and deduplication remain
part of the pending Tool contract. Result-driven execution follows D5; it must
not be implemented by disguising that result as conversation input.

### D5 - Asynchronous results actively wake the caller

A final result arriving after the caller's Turn has completed normally must
actively wake the source Chat to process it. The caller need not poll, keep a
Tool wait open, or wait for the next human message. The wake is a runtime
execution trigger associated with the pending Tool result, not a new user
message, a recipient request, or an insertion into the Conversation Input Queue.

Conversation Authority remains the single execution owner. If a caller Turn is
already active, deliver the result through the existing safe Tool-observation
boundary rather than starting a concurrent Turn. If idle after normal completion,
start a new Turn to continue processing the delegated result; never reopen or
rewrite the terminal Turn. Reuse existing asynchronous notification/wake and
Tool-result projection mechanisms where applicable, without an Agent Task
wrapper. Consumed or duplicate result delivery must not cause duplicate wakes.

The source's resulting answer does not automatically become a new request to
the callee or a reply to be bounced back. Follow-up requests remain explicit
Tool calls. User Stop preserves automatic result wake as specified in D6.

### D6 - User Stop pauses user input, not background result wake

User Stop interrupts the current Active Turn and conditionally pauses the user
input queue under ADR-0024. Late Task and collaboration results remain accepted,
stored, and processed through the normal result-driven wake path. They require
no explicit user resumption. A result arriving while the old Turn is stopping
must remain eligible for processing after that Turn has terminated.

Result-driven execution must neither claim paused user inputs nor clear their
pause, including when the result starts a new Turn. This changes no cancellation
rules for the source Turn's foreground work. Delegated work follows D7.

### D7 - Caller Stop does not cascade to collaboration Chats

Stopping the source Chat terminates only its current Active Turn under the
existing Stop contract. It does not recursively stop configured Agent or ACP
collaboration Chats that have already accepted delegated work.
Those Chats retain their own Conversation Authority and Turn lifecycle.

Their results still return and wake the caller under D6. Stopping a collaboration requires an explicit control targeting that
Chat; a request to stop all related work requires explicit controls for the
intended Chats, not an implicit change to single-Chat Stop semantics. This
decision does not weaken cancellation of the source Turn's own foreground Tools
under ADR-0024.

### D8 - User queue resumption is independent of result processing

Explicit user queue controls retain ADR-0024's acceptance, pause, and claiming
rules. Processing a Task or collaboration result does not resume queued user
input. Results use the existing safe observation boundary when active, or start
a follow-up Turn when idle, with no concurrent Active Turns and no duplicate
wake for an accepted result. Viewer reconnection is not a queue resume action.

### D9 - Success, failure, and explicit Stop all return outcomes

A delegated request must not silently disappear because the callee failed or
was explicitly stopped. Normal completion returns the final reply; failure
returns a failure outcome with an error explanation; explicit Stop returns a
stopped outcome. Available partial output may accompany a non-success outcome
only when clearly marked incomplete, never presented as a successful conclusion.

All these outcomes return through the same caller Tool-result projection and
follow D5-D8's wake, Stop, and resumption rules. The collaboration layer does not
automatically retry a failed or stopped request. The caller decides whether to
issue a follow-up or retry; delivery deduplication is not execution retry.

Ordinary interjection that hands execution to another Turn is not by itself a
failed or explicitly stopped delegation. It ends the old invocation with an
incomplete interjected outcome under D13, rather than keeping that invocation
pending across the handoff.

### D10 - Collaboration context is isolated by source Chat

Each collaboration Chat has one source Chat identified by its full Chat Address.
Different source Chats delegating to the same configured Agent use separate
collaboration Chats; sharing Agent configuration does not share conversation
history. Follow-up requests within an existing collaboration reuse that Chat.
Matching executor identity alone must not select an existing collaboration or
attach to the executor's human-facing Chat.

The first version does not support multiple source Chats sharing one
collaboration Chat. This is a collaboration-origin relationship, not a transfer
of Conversation Authority or lifecycle ownership: the target Chat still owns
its history and Turns. Replies and clarification questions return through the
invocation's Tool-result path under D31. Authorization and execution scope follow
D25-D28 rather than being inferred from the source relationship alone.

### D11 - Send, Stop, and Read are the collaboration Tool surface

`chat_send` accepts a message with exactly one explicit destination: an existing
Chat Address, a configured Agent selected for a new Chat, or a fixed ACP runner
selected for a new Chat. Executor selection explicitly creates a Chat and submits
its first request. Missing or conflicting destinations are errors, not an implicit
creation or a lookup of an existing Chat by executor identity. Continued work uses
the returned Chat Address. `chat_stop` stops work and `chat_read` inspects it;
creation does not require a separate Tool.

A collaboration Chat is either executing an Active Turn or has no Active Turn.
Completion, failure, interjection, and Stop are terminal facts of individual
Turns, not persistent paused/resume-required Chat states. A subsequent Send can
start work without a separate resume operation. Source user input queue
pause under D6 remains independent.

Send to an active collaboration requests prompt interjection: model streaming
can be interrupted, while already-started Tools reach a safe boundary without
silently cancelling their side effects. The accepted input takes precedence over
another model iteration of the old request. Preserve the old partial output and
settle its request as interjected before handing off to the next Turn.

Stop ends the targeted current execution and settles already-accepted, unstarted
requests as cancelled through the normal result path. It must not leave those
requests silently paused or immediately drain them into another Turn. Stop does
not create persistent collaboration wake suppression. Authority fencing and
command idempotency prevent retries from stopping a newer execution.

Send and Read expose bounded, runtime-derived request progress: waiting for a
safe boundary, handing off, started, or terminal, with a blocking Tool when known.
These are request observations, not additional Chat states, LLM-generated
summaries, or promises of completion time. Current request progress is refreshed
in source model-call context without Tool polling. Progress alone does not start a new
source Turn; terminal results retain D5's continuation rules. Stop abandons work,
not an acceleration mechanism for a slow Send.

### D12 - Tool invocation supplies explicit input, not caller history

Calling a collaboration Chat follows ordinary Tool semantics: supply input,
perform work, and return a result. The callee being an Agent does not implicitly
grant it the caller's conversation history. A new collaboration receives the
explicit instructions, materials, and references provided by its caller, plus
the executor's normally resolved configuration and capabilities. It does not
automatically inherit or fork the source Chat's dialogue or Tool transcript.
Follow-up calls continue the target Chat's own history.

### D13 - Interjection settles the old invocation without implicit continuation

Once a cross-Chat request is claimed into a callee Turn, ordinary interjection
ending that Turn settles the corresponding invocation with an incomplete,
interjected outcome. Available output is marked partial. The outcome follows
the ordinary Tool-result return and wake rules; it is not a successful final
answer or a runtime failure, and it does not silently disappear.

The newly claimed message belongs to its own invocation and Turn. Its final
reply is not reused to settle the old request, and the old request does not
remain pending across later Turns. The callee Chat retains its own history;
continuing the earlier work requires an explicit subsequent instruction rather
than hidden request execution tracking. This changes result correlation, not
the existing queue and interjection state machine.

### D14 - The first version permits only one level of delegation

Only a source Chat that is not itself a collaboration Chat may create
collaboration Chats. A collaboration Chat cannot create another collaboration
Chat. Enforce this through tool exposure and authorization at the creation
boundary, not merely through prompt instructions. Calling a configured Agent
or ACP executor does not bypass this rule.

Collaboration Chats may use their permitted ordinary Tools and create separate
background Tool Tasks, and return answers or clarification questions through
their invocation results. Additional collaborators are created by the source Chat,
not by a delegated worker. Tool Task submission must not provide an alternative
path to create nested Rua collaboration Chats.

This bounds Rua-managed delegation; it does not claim to control an external
ACP runner's internal implementation or native subagent facilities. Recursive
Rua collaboration is outside the current scope and requires a later explicit
decision on delegation and resource bounds.

### D15 - Progress inspection returns a bounded summary, not an execution log

`chat_read` exposes concise progress to the calling Agent without submitting
input to the callee or interjecting its Turn. The model-facing progress result
summarizes established findings or completed work, current activity, blockers,
and meaningful changes. It does not page through the callee's full conversation,
Tool arguments, Tool results, or raw execution transcript. Truncating the tail
of a log is not a substitute for summarizing it.

Enforce a finite total model-facing output budget, including when inspecting
multiple collaborations. Coalesce repetitive activity; repeated reads without
new activity return a compact unchanged indication rather than re-emitting
the same summary. Cursor or revision metadata identifies summary freshness,
not a way to reconstruct the full internal transcript in the caller's context.

Derive the summary from available public execution activity without requiring
the callee to submit a checkpoint or make an extra reporting Tool call. Distinguish
observed activity from inferred progress: missing activity is not proof of a
deadlock, and unknown blockers or completion estimates must not be invented.
Detailed records remain in the target Chat for authorized UI inspection, not
automatic forwarding to the caller. Final reply delivery remains governed by
D3-D4 and is separate from progress inspection.

Summary generation follows D20. Concrete output budgets require implementation
design; progress inspection is not a new Agent communication protocol.

### D16 - Project-origin collaboration stays in the Project by default

A collaboration created from a Project-Scoped Chat defaults to a new Chat in
the same Project namespace. It uses the existing Project Resource Binding and
resource Device under ADR-0019; selecting another configured Agent does not
silently move execution to that Agent's Workspace or another Device. This
default applies to configured Agent and ACP collaboration executors.

The selected executor supplies its execution configuration and available Tools
under the caller's authorization policy (D26) and existing Project execution,
Tool Guard, and approval rules. Configured read-only
restrictions are not lifted. Project scope inheritance is not inheritance of
the source Chat's conversation history or a grant of unrestricted file access.

The Chats share the Project's resource root, not a private working copy per
collaboration. File-write isolation requires an explicit mechanism such as a
worktree. Conversation and runtime storage remain in Rua-managed Project
storage, not in the external Project Resource Root. A missing binding does not
silently fall back to an Agent Workspace. D25 generalizes execution-directory
inheritance to non-Project Chats; explicit overrides remain a separate concern.

### D17 - Source deletion does not delete the collaboration Chat

Deleting a source Chat does not cascade deletion to its collaboration Chats.
Their conversation records and independent lifecycle are retained; the source
relation is provenance, not a deletion-ownership edge. Source deletion does not
by itself cancel the retained collaboration's execution or delete its owned
Tasks. Controls and deletion for those Chats remain explicit and independently
authorized.

Retain the source address as provenance with an unavailable/deleted-source
presentation. Do not reparent the collaboration to another source, promote it
to an unrestricted root, or bypass D10/D14 merely because its source was deleted.
A final result remains in the retained collaboration. Delivery to a deleted
source must not recreate its history or trigger execution there; the source's
deletion/tombstone rules govern late and retried deliveries.

Authorized users need a collaboration discovery path independent of opening
the source Chat. A retained collaboration must not become unreachable merely
because the usual source-detail navigation is gone. Navigation follows D18;
direct human participation follows D19. Retention is
subject to the target Chat's own namespace and resource lifecycle; this decision
does not preserve a deleted Project or grant access to unavailable resources.

### D18 - One Chat page provides two list views

Use two views within the same Chat page: "My conversations" for user-originated
Chats (the default) and "Collaboration conversations" for Agent-created Chats.
Do not introduce a separate top-level collaboration management page or a second
Chat storage, search, or detail implementation. Origin determines the view;
selecting a view is not an authorization grant or a change to Chat identity.

Both views reuse the existing list, search, and Chat detail capabilities within
their authorized scopes. The collaboration view supports discovery independently
of the source Chat, including a deleted/unavailable-source indication, and can
filter by source, executor, and status. The source Chat may retain a collaboration
shortcut, but it is not the only navigation path. Detailed activity inspection
in this UI does not change D15's bounded model-facing progress summary.

### D19 - Users may message collaboration Chats directly

Authorized users may send messages directly in a collaboration Chat without a
separate takeover mode. Preserve the human sender's identity and use the same
Conversation Authority input, queue, and interjection controls as other Chat
input. Opening the Chat is observation only; sending input is an explicit
intervention.

If human input interjects an active delegated Turn, settle that invocation with
the incomplete interjected outcome in D13 and return it to its caller under the
existing result/wake rules. The reply to the new human input belongs in the
collaboration Chat; do not attribute it to the previous delegated invocation or
automatically forward it as that invocation's result.

Human participation does not reclassify the Chat into the user-originated view,
replace its source provenance, broaden executor permissions, or lift the
one-level delegation restriction. Existing Chat authorization and executor
capability checks continue to apply.

### D20 - Progress summaries are generated on demand and cached

Generate a progress summary when `chat_read` requests it, not continuously while
the callee works. Reuse a cached summary for the same activity version; no new
activity means no repeated summary model call. For updates, use the previous
bounded summary plus newly available public activity rather than repeatedly
summarizing the complete execution history. Bound summarizer input as well as
output; incremental processing must not assume every unseen suffix fits a
single model request.

Reuse existing model-call infrastructure with a progress-specific summary task.
Do not create a summary Agent, interject the callee, or modify its canonical
history. Track the covered activity version so concurrent activity does not
make a cached summary appear newer than its evidence. Summary generation adds
bounded read latency and model cost only when needed by inspection.

If generation fails, return the known execution status and explicit summary
unavailability (or a clearly identified older cached summary), never raw logs
as a substitute. Waiting for or cancelling an inspection does not cancel the
callee's work. Model selection and concrete input/output/time budgets remain
implementation details to resolve under existing provider and security rules.

### D21 - Submission yields after a bounded wait, not an execution timeout

`chat_send`, including explicit creation with its first message, waits for a bounded
yield window after submitting the request. If the requested Turn reaches an
outcome within that window, return that outcome directly as the Tool result.
Otherwise return an acceptance receipt identifying the Chat, request, and known
queue/execution state. A receipt is not a successful completion result.

Yield expiration ends only the caller's wait; it does not cancel, restart, or
re-submit the callee request and does not create a Task wrapper. An accepted
request may still be queued rather than running when the receipt is returned.
The Tool requires a message for initial creation. Never assert acceptance merely because a
transport wait expired without an authoritative acknowledgement.

Direct return and deferred result delivery share one request identity and a
single delivery handoff. Completion racing with yield expiration must neither
lose the outcome nor project it twice. A directly returned outcome does not
also cause an asynchronous result notification or an extra caller wake. After
receipt return, the final outcome is delivered under D3-D9 and D13 with valid
Tool-protocol projection and deduplication. These are two delivery timings of
the same result contract, not separate execution lifecycles.

Use the existing bounded-wait/yield conventions where applicable. Concrete
parameter names, default duration, and caps remain part of Tool schema design.

### D22 - Restart reports interrupted work instead of resuming it

Restart does not automatically resume, replay, or retry interrupted Task or Chat
execution. Task Attempts retain ADR-0031's `interrupted` recovery outcome.
Collaboration calls whose execution was interrupted, or whose accepted input
was lost with the runtime-only queue, return a non-success outcome explaining
the restart to the caller. Do not silently leave an accepted call pending.

Preserve the minimal request/acceptance correlation and result-delivery facts
needed to report that outcome after recovery. This is not a second persistent
input queue or an execution scheduler. Result notification uses the existing
Tool-result path and caller wake/Stop rules; notifying the caller of failure
does not authorize restarting the interrupted work. Re-execution requires an
explicit new Chat request or Task retry.

Recovery preserves already committed terminal outcomes. If work completed
before restart but delivery was pending, deliver the existing result rather
than rerunning the work or falsely marking it failed. Report unknown partial
side effects honestly. Temporary loss of contact with a remote execution Device
is not proof of a restart or failed execution; reconcile with authoritative
state rather than inventing an outcome or resubmitting work.

### D23 - Do not broaden Subagent capabilities without manageable controls

The proposed change to make Subagents general writable workers is rejected.
Existing read-only restrictions remain while Subagents exist: collaboration
Chat execution, direct human messaging, or background Tool submission must not
grant them general Shell, file-edit, Memory-write, or other mutating capabilities.
The existing narrowly controlled handoff exception is not permission to expose
general writing Tools; its replacement, if needed, requires explicit design.

The rationale is product-level accountability as well as authorization: current
Subagents lack sufficient management and inspection surfaces for complex
side-effecting work. Sharing a Chat implementation alone does not establish
that those controls are adequate. A writable temporary Agent would also erase
the useful distinction from a configured Agent.

D24 resolves the executor choice by replacing Subagents with a preconfigured
read-only Agent. The prohibition on broadening permissions remains applicable
to that replacement and to the existing implementation until it is removed.

### D24 - Replace the Subagent executor and provide read-only and writable Agent presets

Remove the distinct Rua Subagent executor from the target design. Collaboration
has two executor kinds: a configured Rua Agent and an external ACP runner.
Provide two preconfigured Rua Agents: a read-only Agent for research and review,
and a writable Agent for explicitly authorized implementation and verification.
These are ordinary managed Agent instances, not templates the user must manually
instantiate, special executor kinds, or ACP substitutes. Multiple independent
collaboration Chats may use either Agent without creating temporary Agent
identities or sharing conversation history. Keep the user's primary Agent intact.

Implement the read-only preset with ADR-0058's ordinary Restricted Tool Selection
Policy: exact allowed identities expose reading and retrieval Tools such as read, grep, glob,
web_search, and web_fetch. Do not expose Shell, file editing, Memory mutation,
general code execution, or other writable Tools. Do not add a special read-only
Agent type, runtime permission mode, filesystem sandbox, or new security layer
for this preset. "Read-only" describes its configured Tool surface, not a claim
of OS-level isolation or zero incidental cache/log writes.

Any generic Tool-dispatch or Task submission capability must honor the same
available Tool set, not expose a route to disabled Tools; omit such an entry
point from the preset if it cannot do so. Read-only web retrieval is not a
blanket grant of every web/browser/MCP capability. Reuse ordinary configuration
management and existing authorization checks. Default Tool backfill, new builtins,
MCP/plugin exposure, and configuration reload must not silently broaden the
read-only Tool set. Unknown or newly available capabilities are not an implicit
grant to this preset. Restricted selection is preserved in its Chat/Turn snapshots,
independently of the caller's inherited authorization. It is not converted to the
Default-Enabled mode used by ordinary Agents when migrating configuration.

The writable preset exposes the reading, editing, command execution, and
verification Tools needed for implementation, subject to ordinary Tool Guard,
Approval, and D26-D28's inherited authorization/configuration. Writable does not
mean unrestricted or approval-free, and delegation never grants permission to
publish, destroy, or modify outside the user's authorized scope. Both presets
remain subject to D14 when used as collaboration Chat executors.

Provision through ordinary Agent configuration and management. Presets have
inspectable/editable model and Tool settings, use the normal model-resolution
path, and report missing model configuration rather than assuming a provider.
Provisioning is idempotent: do not overwrite user changes or recreate an
intentionally deleted preset on every startup. Editing a preset into a writable
configuration must not leave its effective capabilities advertised as read-only.
Runtime-owned delegation guidance follows ADR-0008 and selects from actual
available Agent identities and capabilities, not invented names in static text.

Use the standard Agent management and collaboration Chat inspection surfaces.
Do not retain a hidden Subagent mode, compatibility executor alias, dedicated
Task handoff writer, or parallel configuration/lifecycle. Update callers to the
configured Agent path and remove obsolete Subagent-specific execution paths.
Existing historical records need not be erased or rewritten. An external ACP
runner's own native subagent implementation remains outside this Rua executor
taxonomy, as noted in D14.

### D25 - Chat execution environment is independent of executor identity

Treat the Chat's default execution environment and working directory as an
explicit Chat-level property, distinct from the selected Agent's Workspace and
the Chat's managed conversation storage. A user-originated Project Chat defaults
to its Project Resource Root and binding. A user-originated Agent Chat defaults
to that Agent's Workspace on the execution Device.

A collaboration Chat inherits the source Chat's effective default execution
environment and resource directory, whether the source is Project-scoped or
Agent-scoped. Selecting Agent B from Agent A's ordinary Chat therefore keeps
A's Chat resource directory; it does not silently switch to B's Workspace.
The same principle applies to ACP collaboration. D16's Project binding and
fixed-device rules remain unchanged.

Inheritance concerns execution defaults, not conversation history, Agent
configuration, tool availability, security identity, or storage ownership.
The selected Agent still supplies its execution configuration and available
Tools, not replacement security authority; caller authorization follows D26.
Conversation data stays with the target Chat's canonical address/storage.
Do not overload an Agent Workspace path to stand for all of these concerns.
Explicit Tool or Hook runtime/cwd declarations retain their existing semantics;
a default directory is neither a filesystem sandbox nor an override of those
declarations.

Retain a device-aware execution binding independently of the source Chat's
continued existence. An absolute path alone is not portable across Devices;
unavailable resources must not silently fall back to the executor's Workspace.
Exact storage and placement representation must align with current Chat
Authority and resource contracts, rather than inventing another Chat aggregate.

### D26 - Delegation retains caller authorization, not executor privileges

Preserve the existing delegated-execution authorization boundary when replacing
Agent Task Runners with collaboration Chats. For a request from Agent A to
Agent B, B contributes its execution configuration and available Tools, but
Rua-owned operations are evaluated using the source execution's snapshotted
authorization policy (D27): Security
Profile, Tool Guard, File Guard, Approval Mode, and applicable approval scope.
Checking only the initial Chat Tool call under A and then running Tools under
B's Security Profile is rejected as a privilege-escalation path.

Do not merge or intersect two Security Profiles and do not substitute B's
profile when A denies an operation. B's Tool availability remains a separate
constraint: selecting a read-only preset does not expose writable Tools merely
because A could authorize them. Generic dispatch and background Tool Tasks
must preserve the delegated authorization identity instead of resetting it to
the selected executor. Moving execution into a Chat or inheriting a resource
directory does not confer additional authority.

For ACP, preserve the existing boundary: caller authorization governs Rua's
submission, process launch, enforceable host restrictions, and controls;
runner-internal permissions continue through ACP Permission Relay. Do not
claim that Rua checks every external runner operation or reinterpret its
permission options as Rua approvals.

Record execution-Agent and authorization-Agent provenance distinctly. The
target Chat remains the owner of its execution and history; authorization
inheritance does not restore an Agent Task wrapper. D27 defines the configuration
lifetime; provenance is not a live lookup of the source Agent's latest policy.

### D27 - Snapshot authorization with the complete Chat configuration

Materialize Harness configuration and security/authorization configuration into
the Chat's complete Configuration Snapshot when it is created. For collaboration,
the executor contributes its selected execution configuration and available
Tools, while authorization is copied from the creating source execution's
effective configuration, not from a freshly loaded Agent default or the target
executor's more permissive profile. Store these contributions coherently in one
configuration epoch alongside the Chat's execution environment.

Later edits to, reassignment of, or deletion of the source Agent do not silently
change or invalidate the existing Chat's materialized configuration. Agent IDs
remain provenance rather than required live policy lookups. Follow-up requests,
direct human input, and background Tool execution use the applicable Chat/Turn
snapshot instead of changing authorization merely because the sender or selected
Agent changed. Execution-Agent defaults are likewise not re-read per Turn.

Reuse explicit UpdateChatConfiguration semantics for intentional changes: show
the diff, atomically produce a new epoch, and leave an active Turn on its claimed
epoch. Source Chat updates propagate inherited configuration under D28; a
snapshot is not a permanent fork of the source Chat's security policy.
Do not freeze transient approval requests or copy source-Chat approval
grants into a different Chat; security policy configuration and runtime approval
decisions remain distinct. External resource or credential availability is also
not guaranteed by a configuration snapshot.

This extends the existing complete Chat snapshot principle to security policy
and requires coordinated changes to ADR-0012's Agent-ID-based policy lookup.
The proposal to bind only an Agent ID and continuously follow its current policy
is rejected. Authorization and propagation follow the effective source Chat
configuration, not live Agent defaults. D28 replaces the proposed separate
approval for continuing a collaboration with a divergent security snapshot.

### D28 - Agent defaults and inherited Chat configuration have different lifetimes

Agent configuration supplies defaults when a Chat materializes its configuration;
the Chat is the carrier of effective execution configuration. Editing or deleting
an Agent default is not the same operation as updating an existing Chat.

If Chat A creates collaboration Chat B, B's security configuration follows A's
Chat configuration, irrespective of which Agent executes B, including when A
and B use the same Agent. Explicit updates to A's inherited configuration must
propagate to its collaboration Chats. In particular, B must not retain an old
independent security snapshot after A's security update takes effect for new
execution. Do not solve that divergence with another permission-difference
approval or by merging two Agent Security Profiles.

Materialized snapshots remain immutable; propagation produces updated Chat
configuration epochs rather than mutating existing snapshots or rereading Agent
defaults. Preserve the existing claimed-Turn epoch semantics. Coherent publication
and execution admission across affected Chats must be specified so stale child
configuration cannot authorize new execution after the source update. Behavior
of already-running work remains governed by explicit Stop and epoch contracts,
not a claim that configuration propagation rolls back side effects.

Propagation applies to source-inherited configuration, including security and
the inherited execution environment, not indiscriminate replacement of B's
executor-specific model, prompt, or available-Tool configuration with A's.
Separate histories, non-cascading Stop/deletion, and ordinary target-Chat storage
remain unchanged. If A is deleted, retained B keeps its last materialized
configuration; deletion neither broadens permissions nor requires live Agent
default lookup.

### D29 - Retain the bounded Task dependency graph

Task simplification does not remove dependency scheduling. Keep immutable
`depends_on`, derived `waiting_on` and `blocked_by`, dependency validation,
failure blocking, and eligibility re-evaluation after explicit predecessor
retry. Users and Agents may pre-submit dependent background Tool work for Task
Runtime to schedule without another model decision between each step.

Keep the existing same-owning-Chat and bounded dependency contracts, concurrency
controls, cancellation, and explicit retry behavior. Do not confuse dependency
waiting with caller yield timeout. Restart interruption still follows D22 and
ADR-0031; retaining the graph does not authorize automatic recovery execution.

The graph relates Tool Tasks, not collaboration Chats or Agent Turns. This
retains useful background workflow capability without restoring Agent execution
as a Task Runner. Public Tool consolidation may simplify how graph state is
submitted and read, but must not hide removal of the scheduling capability.

### D30 - Collaboration executor identity and driver kind are fixed

Initial `chat_send` fixes the collaboration's selected Rua Agent identity or ACP
runner identity and its driver kind. Neither later messages, direct human
interaction, nor configuration updates may replace that executor with another
Agent/runner or switch between Rua and ACP in the same collaboration Chat.
Changing executor requires an explicit new collaboration Chat with the needed
input, not implicit transfer of the old Chat's history.

Model, prompt, and other supported execution configuration may still be updated
through the existing configuration-epoch mechanism, without changing the fixed
executor identity or bypassing inherited security and Tool availability rules.
Source-inherited configuration continues to propagate under D28. An ACP session
identifier is not executor identity; this decision does not require one process
or live session to survive forever, nor authorize automatic recovery execution.

This is a collaboration-specific constraint, enforced by Chat controls as well
as UI affordances. Ordinary user-originated Project Chats retain ADR-0019's
ability to select a different Agent for later Turns. Configuration refresh and
Agent deletion must not silently substitute a different collaboration executor.

### D31 - Clarification is an invocation result, not a reverse Chat call

When a collaboration Agent needs more information, it returns its clarification
question as the current invocation's reply and ends that Turn. The caller
receives it as a Tool result under the usual yield, delivery, and wake rules,
then may explicitly send the answer to the same collaboration Chat as a new
request. The target retains its own conversation history for that next Turn.

Completion of an invocation is not proof that the user's larger task is done;
a clarification question is a valid reply, not necessarily an execution failure.
Do not introduce a waiting-for-answer execution state, a suspended cross-Turn
invocation, or a reverse `chat_send` to the source Chat. This prevents circular
waits and avoids interjecting the user's main conversation with child input.
The earlier provision for child-initiated messages to the source is replaced
by this request/reply rule, not retained as a second communication path.

### D32 - Bound in-flight delegation and reject excess submissions

Apply a finite in-flight collaboration-request limit per full source Chat
Address. Accepted requests count while queued at the target, running, or waiting
for approval; terminal requests release capacity. Idle collaboration Chats and
retained history do not consume request capacity. Rua Agent and ACP delegation
share this admission bound rather than obtaining separate unlimited allowances.

At capacity, reject a new delegation explicitly before it is accepted or
started. Do not return an acceptance receipt, add a hidden overflow queue, or
automatically retry the rejected request. The caller may wait for existing work
and choose to submit again. Pure Chat creation without an initial request does
not reserve execution capacity; actual submission must pass admission.

This gate does not replace the target's existing Conversation Input Queue.
Within the admitted set, target Chat queueing and interjection remain unchanged.
Read, Stop, and result delivery must remain available when capacity is full;
they do not submit new delegated work. Result-delivery retries and duplicate
submissions with the same accepted identity must not consume extra slots.

Admission and release must be race-safe and reconcile with interrupted outcomes
after restart. Resource limits remain distinct from a shared token budget or a
Task wrapper. The concrete default and cap belong to configuration design.

### D33 - Consolidate the Task Tool surface without removing scheduling

Expose six Task Tools: `task_create`, `task_read`, `task_list`, `task_cancel`,
`task_send_input`, and `task_retry`. `task_read` replaces the separate Get and
Await tools with bounded status/output reads and an optional finite wait.
Keep List separate for discovery and keep Cancel, Retry, and SendInput separate
because their arguments, effects, and authorization differ. Do not hide them
inside a generic action-dispatch Tool merely to reduce the Tool count.

`task_create` submits permitted background Tool work and retains D29's dependency
graph. Agent and ACP collaboration use Chat Tools instead of Task Runner
selection. `task_send_input` is restricted to actual input-capable Tool/Process
execution such as stdin/EOF; it is not an Agent-message transport. `task_retry`
retains explicit, side-effect-aware Attempt semantics and never happens merely
because a read or yield wait expired.

`task_read` returns current state, bounded incremental output, cursor/continuation
metadata through one cursor input, not a separate artifact selector. Initial reads
need only `task_id`; the optional cursor may be omitted or null. Attachment entries
provide `artifacts[].cursor`; reading one returns bounded base64 bytes and the same
`cursor`/`has_more` continuation shape. Stored attachment reads return immediately
even when a wait is supplied, preserving Task ownership and resource checks.

Status/output reads include any terminal result. A zero wait reads immediately. A finite wait
returns on available new output, terminal outcome, or its deadline, without
cancelling the work. Waiting/blocked dependency state remains visible rather
than being misreported as running execution. Raw Tool output uses the established
deterministic bounds and result references, not the Agent progress summarizer.

Remove `Task_Watch` and `Task_Unwatch` from the model-facing Tool surface. UI
activity subscription and replay do not require a model to opt in. Models inspect
output through `task_read`; intermediate stdout is not automatically streamed
into model context simply because the UI is observing it. Preserve canonical
terminal result notifications and coordinate them with explicit reads to avoid
duplicate automatic result projection/wake. Internal event subscriptions may
remain as infrastructure, not as another model-managed workflow.

### D34 - Chat Tools expose compact observations with complete control evidence

Apply ADR-0010's semantic projection boundary to Create, Send, Read, StopTurn,
and automatic collaboration results. Model-visible output is not a dump of the
service response or persisted record. Preserve canonical Chat/Turn authority,
HTTP contracts, and internal delivery evidence; do not introduce a second
lifecycle or a generic action-dispatch Tool to obtain a uniform envelope.

Create and Send return the target reference and admission state, adding result
content only when available. Read returns current request state, bounded new
result/progress content, and the accurate references needed for subsequent
controls. Omit redundant input echoes, meaningless nulls, repeated status, and
internal cache bookkeeping that does not help the caller act. Retain complete
Chat routing identity, necessary request/result correlation, and partial,
unavailable, or stale evidence where applicable.

A stoppable Active Turn observation must provide its matching Turn and Authority
epoch evidence, usable unchanged by StopTurn. Never require the model to guess
an epoch from a Turn ID. A stale reference cannot stop a newer Turn. StopTurn
reports what actually happened, distinguishing a successful stop from an already
terminal or changed target; a boolean failure without actionable context is not
a sufficient model-facing contract. A natural-language Send is not cancellation.

Send retains D2 and ADR-0024's normal queue and passive-interjection behavior.
It must not silently request active Send Now. Explicit Send Now controls an
already accepted input; it does not resubmit its content. Failure to promote an
accepted input must not claim that admission was rejected while leaving hidden
executable work queued. A rejected admission has no hidden pending execution.

State/outcome reads remain useful when the progress summary is unavailable.
Summary failure is not execution failure; stale summaries are identified rather
than presented as fresh. D21's finite wait must bound the full read, including any
summary work, not precede an additional unbounded model call. Preserve D4/D5 and
D20's result delivery and bounded inspection rules across compact responses and
automatic observations; formatting changes do not justify duplicate wakes.

Exact JSON schemas, preset names, prompt text, and UI layout belong to protocol
specification and implementation tests. D24's two presets and D34's projections
are accepted target requirements, not a claim that the current implementation
already provides them.

### D35 - Release the source Turn and resume on background results

This decision applies equally to long-running Tool Tasks (including adopted
foreground Tool calls) and collaboration requests. Their execution ownership
remains distinct; sharing continuation semantics does not wrap Chats in Tasks or
create a separate workflow executor.

After submission, the source Agent continues independent work outside the
delegate's write ownership. When no independent work remains, it reports that
background work is pending and normally ends its current Turn. It does not need
to predict completion time, hold a tool call open, or register a model-managed
waiter. Do not add `chat_wait`, revive Task Await/Watch tools, or use repeated
`chat_read`, `task_read`, sleep commands, or empty tool calls to keep a Turn alive.
Existing finite submission yields and bounded reads remain inspection conveniences,
not the normal scheduling mechanism. A read needs a concrete information need.

Runtime owns the continuation relationship from accepted work to its source Chat.
Result identity, ownership, durable acceptance, delivery evidence, authority
validation, and source availability determine continuation eligibility;
neither a model's closing sentence nor a browser subscription establishes it.
Task terminal outcomes use the same source-Chat continuation policy as D5-D8,
including non-success outcomes. Normal source Turn completion does not cancel
either kind of background work. Explicit source Stop preserves automatic
result continuation and the paused user queue; targeted background cancellation
remains a separate control. Elapsed time, read timeout, or unavailable progress
alone is not evidence authorizing cancellation or takeover.

If the source has an Active Turn, accepted results enter its existing safe
observation boundary. Otherwise an eligible result starts a follow-up Turn through
the existing Conversation Authority/input intake, not a fake user message or a
second queue. Result arrival racing with Turn completion must not lose a wake;
concurrent arrivals may be coalesced but must not create concurrent Active Turns.
Delivery is complete when the existing source Chat input queue accepts the result
message. The sender does not wait for a model call, an assistant reply, or Turn
completion. Queue acceptance and idle-source wake use the existing Authority
transition boundary and submission identity. An accepted message belongs to the
source queue; subsequent source failure or Stop must not replay it from the sender.
Do not scan conversation history, infer consumption from assistant messages, or
build a separate consumption-acknowledgement or reply-dependent retry protocol.
Delivery recovery must avoid duplicate observation/continuation for already-queued
results, retain unqueued results across reconnect/restart, preserve the user queue
pause at the authority boundary, and never recreate deleted source Chats or replay background execution.

The source Chat must visibly distinguish these facts:

- Active execution, possibly with background work still in flight.
- No Active Turn, with background work whose result is still awaited.
- A result ready for continuation or currently being handled.
- Approval/input required or failed delivery, rather
  than an indefinite generic waiting indicator.
- No pending background continuation; this is not by itself proof that the user's
  requirements were satisfied.

Collaboration Chat activity has exactly two states: the current Turn is active,
or it is not active. Result readiness, result delivery failure, and request
history are not collaboration activity states. Count each active target Chat
once and remove it from the running projection when its Turn becomes inactive,
even when an outcome is retained for later delivery. Publish target Turn activity
changes to the source Chat so an open viewer does not retain a stale count.

The waiting projection shows counts and navigable identities for Tasks and
active collaboration Chats and survives refresh/reconnect from canonical runtime
facts. It is not a new Turn state, Task state, lease, or executing placeholder.
An already-queued result, an empty collaboration Chat, or an unrelated background
activity must not keep the source in waiting. The UI must not label a normally
ended Turn as the whole Chat's completed work while pending continuation exists.
Closing the viewer neither stops work nor disables continuation. Where execution
authority is unreachable, show that limitation instead of promising immediate wake.

### D35 implementation acceptance

- Submit both a collaboration request and a long Tool Task, end the source Turn,
  and observe no model calls or model-issued polling/sleep during the idle period.
- Show explicit waiting with links/counts after Turn end, page refresh, and viewer
  reconnect. Intermediate logs alone do not trigger repeated model execution.
- Complete background work with the viewer closed: the source resumes via a new
  Turn, consumes results, and continues the user task without new user input.
- Deliver results while the source does independent work, at its Turn-end boundary,
  and simultaneously: preserve all outcomes and single-Active-Turn ownership.
- Exercise failure, interruption, clarification, duplicate delivery, consumed
  explicit reads, restart recovery, user Stop/resume, and deleted source Chats.
  Persisted results alone do not satisfy this end-to-end acceptance.
- Do not report the user's work complete merely because the source Turn ended or
  the background execution returned; required result handling and verification
  still belong to the resumed source Agent.

## Contract verification required for implementation acceptance

- Chat submission finishing before, at, or after yield produces one outcome
  delivery; polling, duplicate transport delivery, and reconnect do not create
  duplicate execution or automatic wakes.
- Interjection, direct human input, explicit Stop, and normal completion return
  the correct request's result and preserve single-Active-Turn ownership.
- Restart reports interrupted accepted work without replay; committed results
  survive delivery recovery and deleted sources are never resurrected.
- Configuration snapshots, source update propagation, and new execution
  admission preserve authorization without consulting mutable Agent defaults.
- Retained collaborations remain inspectable after source deletion without
  changing their origin, executor, or delegation privileges.
- Progress inspection is bounded, cached, read-only, and never exposes raw
  child transcripts on summarization failure; Task output follows its separate
  deterministic read contract.
- Collaboration admission is atomic under concurrent submissions and releases
  capacity on terminal outcomes, not on yield; full capacity does not block
  read, stop, or result delivery.
- Tool availability and inherited authorization apply through generic dispatch,
  background Tasks, and ACP host controls, without claiming control over all
  external-runner internals.
- Task dependencies, failure blocking, explicit Retry, stdin/EOF, cancellation,
  bounded reads, and UI activity remain functional after Tool consolidation.

Additional acceptance coverage for D24 and D34:

- All four Chat Tool responses remain usable after compaction, including queued,
  rejected, terminal, partial, unavailable-summary, and stale-summary cases.
- Observed stop references work unchanged; replay or authority/Turn changes do
  not stop later execution. Normal Send does not implicitly issue Send Now.
- Both managed presets are provisioned without replacing the primary Agent or
  overwriting user edits/deletions. Read-only capabilities remain bounded across
  reload/backfill and indirect dispatch; writable execution still honors approval
  and inherited authorization. Capability discovery reflects effective settings.
- Model-visible active reads and automatic observations preserve result identity
  and bounded delivery. Task Tool output acceptance remains owned by ADR-0032.

## Remaining protocol and implementation design

Before implementation acceptance, verify the proposed mechanics against
concrete Rust DTO/storage mappings and failure boundaries:

Its Rust mapping identifies concrete current gaps: `ChatRunningConfig::resolve`
still overlays partial Chat fields on Agent defaults; persistent Turn records
still require a user-message identity/sequence; runtime-note append requires an
existing Turn. Complete snapshots and result-triggered Turns therefore require
explicit type/storage changes, not just additional Tool adapters.

- Durable request/acceptance/result correlation and direct/deferred delivery
  evidence, without a persistent execution queue or Agent Task wrapper.
- Dedicated result-observation intake: current Task Watch's `accept_submission`
  path cannot simply be reused as though a result were ordinary Chat input.
- Complete security snapshots, inherited revision propagation and new-execution
  admission across updates, deletion, and Authority changes.
- Exact Tool schemas, summary model selection/budgets, output cursors, and
  device-aware resource bindings. These implement confirmed behavior rather than
  reopen it as a new default policy.

## Relationship to current contracts

D1 through D33 are accepted product decisions. Acceptance does not mean code
conformance or production readiness. The protocol draft's proposed Rust types,
storage layout and publication mechanics still require implementation validation.

Coordinated normative amendments are now recorded in ADR-0005/0009/0011/0012,
0019, 0023-0026, 0028, and 0031-0033. ADR-0046 remains Proposed independently,
with retained collaboration deletion semantics aligned here. Request input and
interjection retain ADR-0023/0024 ownership; result-triggered execution does not
create another input queue. Implementation sections distinguish old code and
retirement tests from the amended requirements. PRD/Issues must include those
conformance gaps, not treat the documentation amendment as implementation.

## Implementation status

Accepted architecture, not a blanket implementation-completion claim. D35's
unified Task/collaboration continuation and visible waiting projection require
the end-to-end acceptance above. Existing collaboration delivery and wake paths,
Task terminal Runtime Notes, and bounded read tools are implementation anchors;
their presence alone does not establish idle-source Task wake or UI conformance.
The earlier blanket statement that all adapters remain unchanged is historical,
not a current implementation inventory. Verify each contract against current code
and tests before marking it delivered.
