# ADR 0059: Unified Chat, Project organization, and cross-device resources

**Status:** Proposed — product-design interview concluded on 2026-11-20;
technical design and implementation remain pending.

This proposal records the product decisions confirmed during the 2026-11-19–20
interview. It does not yet supersede the accepted scoped-Chat, resource-binding,
configuration, or authority contracts. The interview changed documentation only.
D33 selects a temporary one-time migration script to accompany implementation
once the target schema is settled.

## Confirmed direction

- Use one Chat model with an independent Chat ID (D30), rather than separate
  ordinary and Project Chat categories; provide unified listing/search (D31).
- Retain Project to organize related Chats and share resource associations.
  Directory guidance comes from applicable AGENTS.md files, not a separate
  Project-wide instruction document (D24).
- Allow user-facing Chats to change their selected Agent and execution directory;
  decouple execution-device selection from the devices holding their resources.
- Manage resource associations at Project level, including directories on
  different Devices such as computer and phone photo directories. Chats without
  a Project have no bound resource collection (D5).

The decisions below define the target ownership, configuration, transition and
failure behavior. The remaining work is to translate them into a technical design
and implementation without treating unchosen implementation proposals as user
requirements.

## Current contract being reconsidered

ADR 0018 includes Agent or Project Scope in Chat identity and storage resolution.
ADR 0019 binds all Project resource directories and Chat execution to one Device.
ADR 0028 gives each Chat complete configuration snapshots and records Agent
assignment for claimed Turns. Those existing mechanisms are the starting point;
unifying identity does not imply replacing Message Log or Turn semantics.

## Remaining technical design and implementation

The product interview concludes with D49. The items below are engineering work
constrained by the confirmed decisions, not another product questionnaire. If
technical exploration reveals a new trade-off that changes the agreed behavior,
raise that concrete conflict rather than silently expanding the product scope.

1. Draft/creation integration: D32 fixes Project membership when a new Chat
   draft is established, preserves the existing lazy materialization boundaries,
   and isolates input caches by Project (including no Project). D30 fixes the
   independent identity; backend admission and migration details remain open.
2. AGENTS.md loading: source provenance, exact discovery-before-execution
   mechanics, and the Turn refresh boundary. D24 records the Codex comparison;
   Rua's confirmed refresh policy is its own target behavior.
   File-based guidance and the discovery baseline follow D24; first-use discovery
   follows D25, best-effort reading follows D26, and the previously selected
   Turn refresh follows D2. D29 selects direct execution-position and resource
   Device/root-path context; exact rendering remains an implementation detail.
   Project-owned resource associations follow D5 and D21-D23. Permission
   revocation remains distinct from removing a resource association (D6/D21).
3. Agent integration: preserve existing knowledge/Memory/credential boundaries
   while adapting independent Chat references. Configuration replacement follows
   D7, admission/background Tasks follow D8/D18, New Chat selection follows D35,
   unavailable optional capabilities follow D36, Agent deletion admission follows
   D40, and collaboration executor identity remains fixed under D41.
4. Execution position: capability/readiness checks and handoff protocol/failure
   recovery. Creation defaults follow D9, directory selection/change follows
   D10/D19, and Device switching with active background Tasks follows D11/D20.
   D37 excludes switch-triggered file transfers; D38 fixes per-Chat/per-Device
   managed directory reuse, and D39 allows offline post-deletion cleanup.
   Task Retry retains original placement under D47; ACP binding must be exited
   before Device switching under D48. D49 makes collaboration Device/directory
   inheritance creation-only while retaining security inheritance; integrate this
   distinction into configuration publication and independent Chat handoff.
5. Resource operations: concrete Tool schemas, cross-device
   enforcement of existing authorization (D6), remote Task admission/control and
   result routing, file-transfer semantics, and uncertain-result reconciliation.
   Per-operation placement follows D12, explicit resource access follows D13,
   partial availability follows D14, long-operation lifetime follows D15-D17,
   and disconnection of remote foreground execution follows D46.
   Device-and-path addressing follows D27; omitted remote command working
   directories use the target default under D28.
6. Project lifecycle integration: D42 permits zero resources, D43 permits metadata
   changes from any paired Device, and D44 commits archive/delete state locally
   without waiting for offline execution Devices or stop acknowledgements. D45
   retains execution-disabling Archive semantics when observed, with normal UI
   entry points removed and the existing backend state check as a backstop.
   Apply/cleanup and late-result handling are implementation work; Agent deletion
   still follows D40.
7. Durable storage: adapt Chat/Media ownership, list projection, Sync keys,
   authority routing, and existing references to D30's independent Chat ID and
   D31's unified list/search. Physical storage and index layout remain open.
8. Cutover execution: D33 selects one-time migration of existing data with a
   temporary script and no ongoing backward compatibility. Final source/target
   mapping, coordinated stop/upgrade/migration, and acceptance checks remain
   implementation work after the target schema is settled.

## Interview decisions

### D1 - A Chat belongs to zero or one Project

Confirmed by the user on 2026-11-19. Project membership is optional and singular;
a Chat cannot simultaneously belong to multiple Projects. This gives its shared
Project resource collection one unambiguous owner. Applicable directory
instructions are discovered from file locations under D24, not from a separate
Project-wide instruction object.

Membership transitions are settled by D3 below. Existing reference resolution
must adapt to D30's independent Chat identity; no additional cross-Project sharing
feature is specified by this decision.

### D2 - Shared resources and applicable directory guidance refresh at Turn boundaries

Confirmed by the user on 2026-11-19, with the instruction-source interpretation
clarified by D24. Project resource membership is a shared reference rather than
a creation-time copy. An existing Chat adopts the latest membership revision
when its next Turn is claimed, without a manual configuration refresh, and
captures the revision used for that Turn.

Applicable directory instructions come from AGENTS.md discovery (D24), not a
fixed Project-wide instruction document. Re-read the applicable directory
instruction chain for a new Turn and record the content/provenance actually used.
Do not replace an already-captured Turn's instructions merely because those files
change on disk. Loading another directory scope later within that Turn still
requires its own defined discovery and recording boundary.

This is not a frozen copy of ordinary resource file contents or a grant of
read/write permission. Association removal during active work follows D21-D23;
actual permission revocation and cross-device enforcement remain distinct
concerns under the existing permission system (D6). It does not introduce live
inheritance of Agent defaults or other Chat configuration.

### D3 - Project membership is fixed from the new Chat draft onward

Confirmed by the user on 2026-11-19 and clarified by D32. Establishing the new
Chat draft fixes either one Project membership or the absence of membership for
the lifetime of that Chat, including before durable materialization. The Chat
cannot join, leave, or move between Projects. This removes membership-transition
rules and their resource/context-retention complexity. The choice supersedes both
proposed answers about retaining or dropping context when leaving a Project.

This does not freeze the Project's shared context (D2), the selected Agent, or
the execution position. D32 preserves the existing materialization timing while
fixing membership already in the draft stage. Project archive/deletion retention
follows D4; deleting a Project does not move its Chats to no Project.

Unified Chat behavior no longer requires a decision to remove Project from every
storage partition. Agent-independent Chat storage may use a unified container
or fixed Project/unassigned physical partitions. D30 selects an independent
Chat ID as the canonical logical identity; physical layout remains open.

### D4 - Archive preserves Chats; permanent Project deletion deletes its Chats

Confirmed by the user on 2026-11-19. Archiving preserves the Project and its
Chats. Permanent Project deletion deletes the Project, its member Chats, and
the associated Rua-managed data. It preserves original files in bound resource
directories. Chats are not detached into an unassigned group, and users are not
required to delete member Chats individually before deleting the Project.

D44 commits the archive/delete metadata change without waiting for execution
Devices to be online or stop acknowledgements. D39 covers deferred physical
cleanup. D45 retains Archive's execution-disabling meaning when observed; deletion
application and reference-safe reclamation integrate with existing lifecycle
services. External resources and unrelated managed data are preserved.

### D5 - Projects exclusively manage bound resource collections

Confirmed by the user on 2026-11-19. A Chat uses its Project's resource collection;
there are no persistent Chat-specific resource additions or exclusions. A Chat
without a Project has no bound resource collection. A Project may itself have
zero associated resources under D42. Changes to the collection follow D2 rather
than requiring per-Chat resource maintenance.

Agent selection, execution Device, and local working directory remain separate
Chat choices. An empty bound resource collection does not mean that the Chat has
no execution working directory, nor does it define a filesystem permission
policy. Resource authorization follows D6.

### D6 - Resource access reuses the existing permission system

Confirmed by the user on 2026-11-19. Reuse Rua's existing permission policies and
Tool Guard for resource access. Do not introduce per-resource read-only/read-write
settings or a second resource-specific authorization configuration system.
Resource associations identify available resources; effective permissions remain
governed by the existing authorization system and applicable device restrictions.

Cross-device enforcement, capability availability, and resource revocation during
active operations still require explicit design. This decision does not equate
Tool Guard with an OS sandbox or make directory association unrestricted access.

When existing policy requests a user's approval for a file on another Device,
the user decides in the Chat where they requested the work. For example, the
computer shows the phone, the file path, and the requested operation. A single
approval there applies to that call. The phone verifies the decision, its local
tool policy, and filesystem access before acting. The one decision covers the
user interaction on both devices. A target denial or an unavailable device is
reported in the originating Chat.

### D7 - Agent switches replace the Agent contribution and retain Chat choices

Confirmed by the user on 2026-11-19. Apply the existing Project Chat switching
approach to unified user-facing Chats. Switching adopts the new Agent's prompt,
Skill selections, Tool selections, authorization policy, and running defaults.
Chat-specific edits within that replaced contribution are replaced as well;
the switch does not preserve an implicit layer of overrides from the old Agent.

Retain the Chat's model and thinking selection, execution Device and working
directory, fixed Project membership, history, and attachments. Project resources
remain Project-owned (D5). Capture the resulting complete configuration as a new
Chat snapshot rather than mixing mutable Agent defaults during execution.
Historical Turn Agent assignments remain unchanged.

Foreground switch admission follows D8 and background Task behavior follows
D18. D35 defines Agent selection on New Chat, including resumed drafts; D36
allows switching when optional capabilities are temporarily unavailable. Existing
knowledge, Memory and credential policies remain applicable; their integration
with independent Chat identity still needs review. D41 retains the fixed
collaboration executor rule.

### D8 - Agent switching requires an idle Chat

Confirmed by the user on 2026-11-19. An Agent switch is allowed only when the Chat
has no active foreground Turn, pending approval, or queued input. The current Turn
must finish or be confirmed stopped, and approvals/queued inputs must be resolved
before switching. Do not add a next-Agent preselection while the current Agent is
still executing. This keeps foreground execution and the selected Agent aligned.

Already-started background Tasks follow D18. D18-D20 extend this admission
rule to working-directory changes and Chat runtime Device switching, including
pending approvals originating from background Tasks. Collaboration executor/driver
identity remains fixed under D41. ACP binding is a separate blocker for Agent
and runtime Device switching under D48; idle state alone does not remove it.

### D9 - Execution defaults apply only when a Chat is created

Confirmed by the user on 2026-11-19. A Project can provide a default execution
Device and working directory for new member Chats. These are creation-time
defaults; changing them does not redirect existing Chats. The user can choose a
different execution position before creating a Chat.

A new Chat without a Project defaults to the Device running the Rua service to
which the client is currently connected, not necessarily the Device running the
browser. Execution defaults are distinct from continuously shared Project
instructions and resource references (D2).

### D10 - Execution position is a Device and an optional explicit local directory

Confirmed by the user on 2026-11-19. Each Chat selects an execution Device and may
select a local working directory on it. Without an explicit directory, Rua
provides an independent, managed working directory for that Chat on the execution
Device. A Project's directory default is adopted at creation (D9), and the Chat
can subsequently change its directory.

Switching Devices requires selecting a directory on the target Device or using
the managed default there. Never interpret a source Device's absolute path as a
portable target directory. An explicitly configured directory that is missing
causes an error rather than silently falling back to the managed default.

D37 clarifies that switching does not copy working files, resource directories
or attachment bytes as a side effect. Retain the necessary conversation-state
and authority handoff, without treating file pre-copying as a switch prerequisite.
Files are accessed when needed under their normal access/transfer rules. Managed
directory reuse follows D38 and post-deletion offline cleanup follows D39.
Selecting a working directory does not itself change the Project resource
collection or grant additional permissions.

### D11 - Execution-Device switching requires cooperative handoff

Confirmed by the user on 2026-11-19. Switch execution only after the old execution
Device confirms that foreground execution is stopped or quiescent and the new
Device can adopt the Chat. If the old Device is unavailable, do not offer forced
takeover. An authority epoch by itself cannot fence filesystem effects that an
unreachable old Device might still produce.

This settles the cooperative-only policy, not the complete transfer protocol.
D20 specifies idle admission and background Task behavior. Capability/readiness
checks, authority/result-routing races, and handoff failure recovery still need
explicit treatment.

### D12 - Tool execution placement is independent of the Chat runtime Device

Confirmed by the user on 2026-11-19. A Chat can run its Agent loop on one Device
and explicitly invoke Tool operations, including commands, on another Device.
The target executes the operation and returns results to the originating Chat;
this neither migrates the Chat nor creates another Agent conversation.
Conversation Authority remains the single owner of the Chat's Active Turn and
canonical conversation writes.

D9-D11 describe the Chat's runtime Device and its default local working directory.
Their cooperative-handoff requirement applies to changing that Chat runtime
Device, not to invoking an individual remote Tool. A resource's owning Device,
a Tool's execution Device, and the Chat runtime Device are distinct roles even
when one physical Device serves several of them.

Existing permission rules still apply (D6). Long-operation placement and
lifetime follow D15-D17; target admission and result-routing details remain open.
ADR 0032 D7 currently fixes Task acceptance/execution to the Chat Authority
Device; supporting the independent remote Task placement selected here requires
an explicit extension rather than assuming that the current contract already does it.

### D13 - Remote resources use explicit access and transfer, without transparent mounts

Confirmed by the user on 2026-11-19. Use unified file access tools with explicit
Device-and-path locations (D27), and explicit transfer when files are needed on
another Device.
Ordinary programs operate on files local to their execution Device. To process
phone photos, execute a suitable program on the phone or transfer the required
files to the computer. Do not require a transparent distributed mount that makes
remote directories appear to be local directories.

Remote execution (D12) and transparent mounting are separate choices. This
explicit-access design does not restrict resource access to reads; existing
permissions govern authorized operations (D6). Exact Tool schemas, copy conflict
handling, and cross-device move semantics remain to be specified.

### D14 - Resource availability gates dependent operations, not the entire Project

Confirmed by the user on 2026-11-19. When a resource required by the current task
is unavailable, continue the portions that can be completed independently and
clearly report the missing coverage and remaining work. Defer operations whose
required inputs are unavailable; do not treat partial evidence as a complete
result or use it to perform mutations that require a complete comparison.

For example, if phone photos are unavailable, inventorying computer photos may
continue while comparison across both collections remains incomplete. Evaluate
the resources needed by the current operation, not whether every Project resource
is online. This decision does not authorize offline command queues, automatic
replay, or claims that an operation with an unknown outcome has failed.

### D15 - Remote foreground work can transfer into a target-owned Task

Confirmed by the user on 2026-11-19. Remote commands can be submitted explicitly
as background Tasks. Eligible foreground commands may also use the existing
Tool Transfer Policy: on expiry of the synchronous wait window, the actual
execution Device adopts the same running execution into its Task Runtime and
returns a Task reference so the Chat can continue other work.

The execution Device manages authoritative Task execution state and results;
the owning Chat remains the Task's logical owner. A synchronous wait limit is
not an execution deadline. Adoption must not cancel or relaunch the command,
and the caller must not report successful Task creation before target-side
acceptance. Existing completion/adoption race rules still apply. Short ordinary
Tool calls need not become Tasks merely because they execute remotely.

The admission, foreground-adoption protocol, control routing, and durable Task
references across Devices remain implementation-design work. This extends the
placement constraint identified in D12 while retaining the existing Task model.

### D16 - Accepted background Tasks survive network disconnection, not runtime restart

Confirmed by the user on 2026-11-19. An accepted background Task continues on its
execution Device while that runtime remains healthy, even if contact with the
Chat runtime Device is lost. The execution Device preserves results for later
delivery or inspection after connectivity returns. Reconnection does not relaunch
the operation. An unreachable execution Device yields unknown current status,
not evidence that the Task failed, completed, or stopped.

A restart of the Task Runtime itself follows the existing recovery contract:
previously non-terminal Attempts become interrupted and are not automatically
replayed. Already completed results remain available. Loss of a viewer, network
connection, or the originating Chat process is not by itself a restart of the
remote Task Runtime.

This decision covers accepted background Tasks. D46 defines cancellation for
remote work still owned as foreground execution when its execution connection
is lost. Lost admission acknowledgements and precise reconciliation protocols
remain implementation work: target-side acceptance determines ownership even
when its acknowledgement is lost. Unknown-outcome mutations must not be
automatically replayed.

### D17 - Chat Stop leaves background Tasks running and preserves result continuation

Confirmed by the user on 2026-11-19. Chat Stop terminates its current Turn and
foreground Tool work under the existing Stop contract. It does not cascade to
already-backgrounded Tasks, whether local or remote. Cancelling those Tasks
requires a separate Task control.

Background Task results can still wake the Chat to process them after Stop.
Result processing must not resume or claim the user inputs paused by that Stop.
This follows ADR 0053 D6/D35; its newer continuation contract takes precedence
over ADR 0032 D6's older statement that Stop suppresses wake, which must be
reconciled when updating the accepted architecture.

Cancellation without confirmation from the execution Device must remain visibly
unconfirmed. Only target-side terminal evidence establishes that execution has
stopped; natural completion may win a cancellation race. Contact loss alone
cannot establish a successful stop, and an uncertain mutation must not be rerun
as an implicit recovery action.

### D18 - Background Tasks do not block Agent switching once foreground admission is clear

Confirmed by the user on 2026-11-19. Agent selection, default working-directory
changes (D19), and Chat runtime Device switching (D20) share one admission gate:
no active foreground Turn, queued input, or unresolved approval. Approvals
originating from background Tasks count toward this gate. A running background
Task by itself does not block these changes.

An accepted Task keeps its original Agent/configuration provenance and captured
execution configuration; switching the Chat's Agent neither reconfigures nor
restarts it. Subsequent result-processing Turns use the Chat's then-selected
Agent while preserving the Task's original provenance. For example, Agent A can
start a phone scan and Agent B can later process that scan's result in the same
Chat after an admitted Agent switch.

This concerns Tool Tasks owned by the Chat. It does not change separate
collaboration-Chat executor or driver constraints, nor define new rules for
permission revocation or explicit Task Retry.

### D19 - Default-directory changes leave existing Task execution locations intact

Confirmed by the user on 2026-11-19. Once the shared admission gate in D18 is
satisfied, a Chat may change its default working directory even while background
Tasks remain active. The change affects the default used for future Tool calls;
existing Tasks keep their original execution Device and working directory.

Task output files remain at their original locations. Result references retain
their Device/location provenance and must not be reinterpreted relative to the
Chat's new working directory. Changing the default does not move files, restart
Tasks, or change an explicit resource/target choice. Explicit Retry retains the
Task's original resolved execution Device and working directory under D47.

### D20 - Cooperative Chat handoff leaves background Tasks on their execution Devices

Confirmed by the user on 2026-11-19. Once D18's shared admission gate is satisfied,
a Chat can move its runtime Device while background Tasks remain active. D11's
cooperative handoff still applies: the old Chat runtime Device must confirm
foreground execution is stopped/quiescent and the new Device must satisfy the
conditions for continuation.

Existing Tasks remain on their own execution Devices without migration or
restart. The new Chat Authority receives subsequent results and routes Task
controls to the actual execution Devices. Its subsequent result-processing Turns
follow the current Chat Agent selection (D18).

A background Task's Device being unreachable does not by itself block Chat
handoff. For a Chat on computer A and a Task on phone B, transfer to computer C
may proceed if A can cooperate and C can continue, even while B is unreachable.
B's Task state remains unknown, not stopped. If A itself is unavailable, D11
still prevents handoff. Pending approvals and other admission/readiness failures
are not waived merely because the relevant Task runs remotely.

The handoff protocol must reconcile result delivery and Task-control routing;
this policy does not claim that transport connectivity alone proves handoff
readiness or that the existing implementation already supports this flow.

### D21 - Resource association edits do not require existing work to finish

Confirmed by the user on 2026-11-19. A Project may remove a resource association
while existing Turns or Tasks still use it. Already-started work retains its
captured resource references and may continue accessing the original location;
new Turns no longer receive the removed association. Adding a resource makes it
available at the next Turn under D2 without requiring the whole Project to be
idle. This replaces ADR 0019's Project-wide idle gate for resource edits in the
proposed architecture.

Removing an association does not cancel work, delete original files, or revoke
global filesystem permissions. Access remains governed by the existing permission
system. A Chat whose independently selected working directory happens to be the
same directory keeps that selection; association removal does not change its cwd.

Captured references are not frozen file contents. Original locations becoming
unavailable must produce truthful failures, not implicit redirection to another
resource. Retention of reference records and concurrent edit/admission handling
remain protocol/storage design work.

### D22 - Changing a resource Device or root creates a new resource identity

Confirmed by the user on 2026-11-19. Replacing the Device or root directory of a
resource is expressed as removing the old resource association and adding a new
resource with a new identity. Existing Task and historical references do not
silently change destinations. Renaming a display label preserves identity.

This deliberately changes ADR 0019's existing same-ID directory-path rebind rule.
If the original directory has been moved externally, old operations fail against
the original location rather than automatically following the new association.
This is identity management, not a file-move operation or an instruction to copy
the original data. Removal follows D21 and new target validation follows D23.

### D23 - An offline resource Device does not prevent association removal

Confirmed by the user on 2026-11-19. A Project can remove its association with a
resource whose Device is unavailable, provided the Project configuration update
itself can be validly saved. Source-file access and acknowledgement from that
resource Device are not prerequisites for this metadata change.

Existing work on the unavailable Device remains of unknown current status;
removing the association must not report it as stopped or revoke its captured
references. D21's permission and cwd boundaries still apply. Adding a resource
or validating a new path still requires validation by its target Device; offline
removal does not authorize creating an unvalidated binding.

### D24 - Use directory AGENTS guidance with Codex-style discovery; no Project-wide instruction field

Confirmed by the user on 2026-11-19. Do not introduce a fixed Project-wide
instruction document, independent Project instruction text field/editor, or a
new Project-selected cross-resource instruction source. Reuse directory-based
AGENTS.md guidance and use Codex's published discovery behavior as the design
reference. This supersedes the earlier proposed instruction-source question and
the premise of a separate Project text overriding Agent prompts; those proposed
options were never accepted.

Keep the existing Agent-owned prompt source independent: selected prompt files
come from the Agent's workspace/prompts directory and participate in captured
Chat configuration. Directory guidance is an additional contextual instruction
source. Maintain instruction provenance and existing mandatory protocol and
permission boundaries; this decision does not require flattening all layers
into one model API system message or assigning them equal priority.

The directory-discovery baseline to apply is:

- Walk from the detected repository/project root (normally Git root) to the
  working directory; when no root is identified, check the working directory.
  A Rua Project grouping or attached resource root is not automatically a Git root.
- At each directory, select at most one instruction file, preferring
  AGENTS.override.md to AGENTS.md. Explicitly configured fallback names, if
  supported, come after these; this does not require a new fallback-setting UI.
- Combine from root toward the working directory so more specific directory
  instructions override ancestor-directory instructions within their scope.
- Skip empty sources, retain source/location information, and bound the combined
  guidance size. The existing 32 KiB default is consistent with Codex's default.
- Attaching a resource does not automatically load every attached directory's
  instruction/configuration files or activate Skills and Hooks. Directory rules
  must not silently become global rules for unrelated resources.

Codex's documented startup discovery is a reference, not a reason to undo Rua's
already-selected next-Turn refresh policy (D2). For remote work, discovery must
use paths on the Device that holds them; it must not interpret a phone path on
the Chat's computer. D25 settles first-use discovery for explicit target paths;
its precise implementation remains open. Unreadable AGENTS.md files follow
D26's best-effort reading policy.

Codex also documents global user guidance in its own home directory. Rua already
has Agent-owned prompts; this decision does not add another global AGENTS source
or merge every remote Device's personal global instructions into the Chat.

Evidence: current Rua reads only working_dir/AGENTS.md through
`crates/rua-server/src/services/project_instructions.rs` and the Project branch
of `console_chat.rs`; the Agent prompt capture separately uses
`chat_configuration_capture.rs` and `rua-agent/src/prompt.rs`.
The target discovery contract is documented in
[Codex AGENTS.md guidance](https://learn.chatgpt.com/docs/agent-configuration/agents-md)
and [local project folder behavior](https://learn.chatgpt.com/docs/projects#use-local-projects-for-folders-and-codebases).
These public documents establish discovery behavior, not the full private
prompt contents or exact message assembly of every ChatGPT surface.

Refresh-behavior verification on 2026-11-19: the local Codex CLI reports 0.155.1.
That release and the inspected public main keep repository AGENTS.md contents
cached when the selected execution environment and trust settings are unchanged;
editing the file alone does not force a read on the next Turn. A newly created
runtime Session reloads them, including a cold restart/resume of an existing Chat.
The public main test
[multi_environment_thread_refreshes_global_and_keeps_repository_snapshot](https://github.com/openai/codex/blob/39c29c3bb620f91e84a3bb266bb496dfb697c101/codex-rs/core/tests/suite/agents_md.rs#L1935)
explicitly changes global and local/remote repository files between Turns and
retains both repository snapshots while updating global instructions.
The separate global-home refresh added in
[PR 44675](https://github.com/openai/codex/pull/44675) is therefore not evidence of
per-Turn repository AGENTS.md reload. This evidence concerns public Codex versions,
not a guarantee about every ChatGPT desktop build. Rua's D2 next-Turn refresh
remains its own selected behavior; the user's question about Codex did not
itself change that decision.

### D25 - Discover applicable AGENTS.md before the Agent acts on a new explicit target

Confirmed by the user on 2026-11-19 (question 1A). When a Tool explicitly targets
a resource, directory, or file whose applicable AGENTS.md chain has not yet been
loaded, Rua discovers that chain on the Device holding the path. It presents the
applicable instructions to the Agent before the affected operation executes.
The Agent must have an opportunity to reconsider or change the operation; merely
adding instructions and immediately executing the old command is insufficient.

This applies to paths explicitly identifiable from Tool requests. It does not
promise static analysis of every path that arbitrary shell/Python code might
access. Existing instruction-source contents stay fixed within the Turn under
D2; newly encountered directory scopes are discovered and recorded when needed.
Specific request/response mechanics remain open. D26 clarifies that this
loading step is best effort: unreadable AGENTS.md does not block the operation.

The user asked to unify terminology: use AGENTS.md when discussing these files,
and name AGENTS.override.md explicitly when its selection matters. Do not present
"rule files", "shared descriptions", and "directory instructions" as different
new product concepts. No separate Project instruction source is reintroduced.

### D26 - Read AGENTS.md best effort and skip unavailable contents without blocking work

Confirmed by the user on 2026-11-19. At an AGENTS.md loading boundary, include
successfully read content in the model's instruction context. If a file cannot
be read, omit its unavailable content and continue; do not pause or reject an
otherwise permitted operation because AGENTS.md could not be loaded. File
absence is also non-blocking.

This replaces both previously offered unreadable-file alternatives: blocking the
affected operation and requiring a stale cached copy. Do not silently substitute
an earlier Turn's file contents for a failed new read. Content already captured
within the current Turn remains governed by D2 until the refresh policy is
explicitly revised. No separate permission is required merely to proceed without
an unreadable AGENTS.md; ordinary Tool authorization and actual resource-access
failures retain their existing behavior.

D25 still applies when new instructions are successfully discovered: provide
them to the Agent before it decides the affected operation. An unsuccessful
AGENTS.md read must not become an instruction-specific execution gate. Detailed
loading diagnostics and bounded remote-read mechanics are implementation details.

### D27 - Address files uniformly by Device and real path

Confirmed by the user on 2026-11-19 (question 2, choosing Device plus real path).
Use the owning Device and its real filesystem path as the Agent-facing location
for file access and as the basis for execution/transfer locations. Do not require
an additional resource-ID-plus-relative-path addressing form for Project files.
This same location model also works for Chats without a Project and other paths
permitted by the existing authorization system (D5/D6).

Stable resource identities still govern Project association storage and lifecycle
under D22; they do not need to become Tool-facing file addresses. Device display
names are not identity: the callable Device reference must unambiguously identify
the target even when names collide or change. A path is interpreted on its named
Device, never implicitly on the Chat runtime Device or through a virtual mount.
This does not ban ordinary relative paths inside commands; those resolve against
the command's actual working directory, whose omission follows D28.

### D28 - Omitted remote command directories use the target's fixed default

Confirmed by the user on 2026-11-19 (question 3B). A remote command does not have
to supply a working directory. When omitted, use the fixed default provided by
the target Device's execution environment: its user home directory, as accepted
by the user in the following round. Resolve that directory in the actual target
environment rather than assuming a host path is also valid inside a guest.
For execution on the Chat runtime Device, omission retains the Chat's working
directory default. An explicitly supplied directory takes precedence.

Never infer a remote directory from the last command's `cd`, the Chat's local
absolute path, or the first Project resource. Resolve the default on the actual
target execution environment, including the Linux Compatibility Runtime on
Android. Return the actual Device and working directory in execution evidence.
A missing or inaccessible explicitly requested directory remains an error rather
than falling back to the default (D10). An accepted Task captures the resolved
working directory, so later default changes do not redirect existing work (D19).

### D29 - Provide execution position and resource Device/path context directly

Confirmed by the user on 2026-11-19 after the comparison with this conversation
and public Codex context. Each new Turn directly receives the Chat execution
Device and working directory, plus the owning Device and root directory of each
resource in that Turn's Project collection. The Agent does not need a preliminary
resource-list Tool call just to learn these locations. Device references remain
unambiguous under D27; readable labels may accompany them without becoming identity.
Resource IDs remain internal association-management identities and are not required
in the Agent's resource inventory or file-operation arguments.

Directory structure, ordinary file contents, and Device capabilities are inspected
on demand. Do not automatically scan every resource, include file counts, construct
indexes, or require reachability probes of all Devices at each Turn. Resource
membership refresh follows D2, AGENTS.md follows D24-D26, and actual unavailable
resource operations follow D14. Exact prompt syntax is an implementation detail,
not another user-visible Project configuration surface.

Official local-project documentation establishes that attached directories are
available for search/read/edit and that the primary folder drives default
AGENTS.md discovery. It does not publish an exact model-context template or
prove that every ChatGPT surface supplies only path strings. The recommendation
for Rua should remain distinguishable from those verified product facts.

The unreadable-AGENTS.md question is resolved by D26's best-effort reading
policy, rather than either of the earlier alternatives.

Verification on 2026-11-19: this desktop conversation already receives the Rua
working directory, filesystem workspace roots, and separately supplied AGENTS.md
content. It does not need a resource-list Tool call to discover the Rua root. The
observed context has no dedicated Project/resource name, resource ID, or owning
Device field forming the four-field inventory previously proposed for Rua.
Filesystem workspace roots are not necessarily identical to Project attachments;
they may also include runtime output locations. This is an observation of this
conversation, not a guarantee about every desktop version.

The public Codex 0.155.1
[environment rendering tests](https://github.com/openai/codex/blob/rust-v0.155.1/codex-rs/core/src/context/world_state/environment_render_tests.rs#L175)
also show direct cwd/shell and filesystem workspace-root injection. Its multiple
execution-environment IDs are not equivalent to Rua Device or resource identities.
Additional desktop extensions can contribute context, so the public core alone
cannot prove the complete absence of other product metadata. D29 selects the
subsequently accepted simplified Rua inventory; it does not reinstate the earlier
proposed mandatory resource-name/resource-ID/Device/root four-field inventory.

### D30 - Chat ID independently identifies a Chat within one user identity

Confirmed by the user on 2026-11-19 (identity question 1A). Within one user's
identity/synchronization domain, a Chat ID alone identifies one Chat. Agent and
Project identifiers are not components of equality or the canonical locator.
Project membership remains a fixed attribute (D3/D32); selected Agent and execution
position remain configuration. Links, Task ownership, Media references, routing,
and Sync must consistently reference that independent Chat identity.

This revises the target identity contract of ADR 0018; current accepted scoped
contracts remain in force until this proposal is adopted and implemented. It does
not bypass ordinary authorization, change the single Conversation Authority rule,
or require every data file and index to be stored in one physical container.
Migration must explicitly handle legacy scoped identities and reference mapping;
existing UUID strings cannot be assumed globally unambiguous merely because new
Chat IDs will be independent.

Issue 01 uses a shared Chat workspace at `<rua-root>/chats/`. The session
header at `sessions/<chat-id>/session.json` records the selected Agent and the
optional Project fixed at creation. History, configuration, attachments and the
rebuildable Chat Index resolve from this workspace. Agent configuration continues
to resolve from the Agent workspace. Console opens `/chats/<chat-id>` and HTTP
addresses a saved Chat through `/api/chats/<chat-id>`. A draft supplies its
initial Agent and optional Project when the execution device accepts its first
submission. Staging an attachment preserves the existing materialization boundary.

### D31 - Provide unified Chat listing/search with Project and current-Agent filters

Confirmed by the user on 2026-11-19 (listing question 2A). User-facing Chats can
be found through one unified listing/search entry point across Projects and Chats
without a Project. Retain Project organization/navigation and support filtering
by Project and currently selected Agent. Switching Agent changes which current-
Agent filter matches the Chat; it does not remove history or rewrite historical
Turn assignments. This does not introduce a historical-participant filter.

Unified discovery does not change archive/visibility policies or flatten
collaboration Chats into ordinary user-visible results. Preserve the separation
between authoritative Chat state and rebuildable list projections. Browser-local
unmaterialized drafts first enter discovery when their first submission succeeds (D32).

The unified list reads the shared Chat Index. Agent, fixed Project membership,
archive and conversation-view filters apply before counting or paging. Omitted
Project membership selects all Projects and standalone Chats. An explicit
standalone filter selects only Chats without a Project. Pinned Chats participate
in the same pagination as other Chats, with Chat ID breaking ordering ties.
Transcript search uses the same candidate order and counts visible matching
messages. On startup, Rua rebuilds local discovery rows from saved Session
headers, including current Agent, membership and pin order, and honors deletion
intents and synchronized tombstones. Opening a Chat resolves its configuration
and history separately.

### D32 - Fix Project at draft creation and preserve lazy materialization and input caching

Confirmed and explicitly clarified by the user on 2026-11-19 (creation question
3B). Clicking New Chat establishes or resumes a draft with a fixed Project, or
fixed absence of Project. It does not by itself persist an empty backend Chat.
Keep the existing draft and materialization logic; clicking New Chat does not
advance its persistence boundary. Materialization retains the same Chat ID and
fixed membership. This corrects the assistant's interpretation of 3B; eager empty-Chat
persistence was not requested.

Adapt the existing local draft/input cache to Project organization:

- Keep a resumable new-Chat draft per Project and a separate no-Project group,
  replacing the old Agent-based grouping for ordinary Chat drafts. Repeated New
  Chat actions within the same group continue to resolve the existing draft
  until the existing lifecycle releases it. D35 separately applies the current
  Agent Selector choice on every New Chat action, including this reused draft.
- Retain text and restorable attachment references for the particular draft/Chat
  within that group. Different existing Chats in one Project must not share a
  single input buffer. The logical association is Project-or-none plus Chat ID;
  the exact cache-key encoding is an implementation detail, not Chat identity.
- Selecting another Project opens its own draft/Chat state; it does not move or
  relabel the previous draft. Returning restores that group's cached input.
- Changing Agent within the same draft/Chat does not select a different cache,
  clear its input, or change Project membership. Preserve existing submission,
  failure recovery, materialization and cleanup semantics while adapting their
  keys. Do not silently promote browser input caches to synchronized Chat facts.

Current evidence: `projectChatNavigation.ts::createProjectChat` reuses a draft
under `project:<projectId>`, and `projectChatDraftKey` isolates input by Project
and Chat ID. Its tests verify repeated New Chat reuse and cross-Project isolation.
`chatNavigation.ts` currently groups ordinary drafts by Agent; `drafts.ts` stores
input/attachment references and submitted-input recovery separately. These are
existing behaviors to adapt, not reasons to replace the materialization model.
D35 changes the prior reused-draft Agent retention behavior only: the New Chat
selection now follows the current Agent Selector without changing the draft ID,
Project or cached input.

Current materialization verification: `chat_media.rs::upload_media` stages media
when placement is absent and returns no Authority Epoch; it does not create the
Chat header. The cross-device transfer acceptance test materializes on the target
when initial input is accepted. This matches ADR 0022 D3. ADR 0018 D5 and ADR 0030
D6 still contain older first-upload-materialization wording; do not adopt that
stale wording as a new requirement. Preserve current behavior rather than
redesigning creation as part of this change.

### D33 - Migrate existing data once with a temporary script; keep only the new model

Confirmed by the user on 2026-11-19. Rua has not been formally released. Accompany
this architecture change with a temporary script that migrates existing data to
the selected new schema once. Existing data is to be migrated, not implicitly
reset or discarded. Implement the script after the target storage schema is
concrete enough to map and verify; this interview does not provide that schema
or mean a migration has already run.

Production code should use the new identity, storage, API and synchronization
contracts directly. Do not add permanent legacy readers, dual-write paths,
old-route compatibility, bidirectional old/new adapters, or a long-lived mixed-
version support mechanism for this change. Any old-format understanding needed
for the transition belongs in the one-time migration tooling. Old/new clients
are not required to concurrently operate on the same migrated data.

The migration must preserve the intended data and convert its internal ownership
and references consistently, including any scoped-ID collisions, Chat history,
configuration, attachments and related records affected by the final layout.
Project-bound external files remain in place; moving Rua-managed data must not
move or delete source resource directories. Coordinate a stopped/consistent
source state and upgrade of affected Devices as part of the one-time procedure,
then verify the migrated graph and rebuild derived indexes. Exact backup,
restart/retry and validation mechanics belong to the migration implementation,
not a new permanent product compatibility subsystem.

### D34 - Deleting an Agent preserves its selected Chats

Confirmed by the user on 2026-11-19 (Agent lifecycle question 1A). After an admitted
Agent deletion, retain Chats that currently selected it, including their history,
attachments and configuration/provenance records. User-originated Chats require
selection of another valid Agent before starting a new Turn; do not cascade Agent
deletion into Chat deletion or silently select a replacement. Historical Turn
Agent assignments remain historical facts even when the definition no longer exists.

D40 requires unfinished work to be resolved before deletion. D41 is the explicit
collaboration exception to changing Agent within the retained Chat: preserve the
old collaboration and its history, but replacing its deleted executor requires a
new collaboration Chat rather than reassigning the old one. Deletion does not
authorize dropping results or destroying files owned by retained Chats.

### D35 - Every New Chat action adopts the current Agent Selector choice

Confirmed by the user on 2026-11-19 (question 2B with an explicit clarification).
Every click of New Chat uses the Agent currently selected in the Agent Selector.
This applies both to a genuinely new draft and to restoring an existing draft
under D32. The existing draft's previous Agent does not override the selector.
Do not add a separate per-Project/no-Project recent-Agent default for this action;
use the actual selector state rather than an independently inferred recent Agent.

Restoring a draft changes its Agent selection as needed while preserving its
Chat ID, fixed Project-or-none membership, cached text/attachments and the
existing lazy materialization boundary. Agent-contributed settings follow D7's
replacement semantics, with draft preparation distinguished from committing a
materialized Chat configuration. Merely opening a materialized Chat is not a
New Chat action and does not reassign it from the global selector. Existing
accepted-submission and Agent-switch admission rules are not bypassed.

### D36 - Temporarily unavailable optional capabilities do not block Agent selection

Confirmed by the user on 2026-11-19 (question 3A). When the target Agent's required
configuration is valid and resolvable, allow selection even if some Tool/MCP
capabilities are temporarily unavailable. Keep available capabilities usable and
explain the unavailable capability when it affects an operation. Do not silently
fall back to the old Agent or mix its configuration/capabilities into the new one.

Invalid or unresolvable required Agent configuration still fails selection. This
policy does not automatically install dependencies, copy credentials, claim an
unavailable Tool is callable, or waive existing authorization and exposure rules.
D8/D18's idle/admission gate still applies to materialized Chat Agent changes.
It does not by itself settle the core runtime/provider readiness required to
move the Chat's execution Device.

### D37 - Switching the Chat execution Device does not initiate file copying

Confirmed by the user on 2026-11-19 (directory question 1A, with an explicit
clarification to avoid extra work or cross-device file copying during a switch).
Changing the Chat runtime Device/target directory must not automatically copy,
move, mirror or synchronize ordinary working files, attached resource trees, or
attachment bytes. Leave files on the Devices that hold them and use their normal
on-demand access or explicit transfer operations when actually needed. Do not
hide a directory-copy job inside handoff or require it before switching succeeds.

The selector change still needs D11/D20's cooperative authority transition and
valid conversation-state continuation; it is not permission to create concurrent
writers or lose history. Ordinary conversation-state synchronization remains
independent of switching. A file already replicated through its existing mechanism
may be used, but switching itself does not trigger a new copy or promise local
availability. An unavailable file affects operations requiring it under D14,
rather than being silently read from a target-local lookalike.

This clarifies D10's earlier transfer/adoption wording and revises the target
behavior of ADR 0030 D8: do not eagerly pre-transfer a Required Media Set as a
side effect or prerequisite of the execution-Device switch. ADR 0030 D4's
read-routing implementation must also stop assuming that the current Chat
Authority physically holds every media object: resolve an existing valid replica
or the Device holding the bytes when an actual read is requested. This is not a
ban on network reads or explicit transfers when files are used, and does not
require changing portable media references into raw paths. Exact integration
with existing references is implementation work. Existing Tasks and output
locations continue to follow D19/D20.

### D38 - Reuse one managed working directory per Chat and Device

Confirmed by the user on 2026-11-19 (directory question 2A). Each Chat has a
stable Rua-managed default working directory on each Device where it uses that
default. Returning to that Device and selecting the managed default reuses the
same directory and the files remaining there; do not create a new empty directory
on every switch. The Chat ID and Device identity determine this association,
independently of the selected Agent and fixed Project membership.

Directories on different Devices have independent contents (D37). This does not
automatically restore a previously selected external working directory, copy its
contents, or relocate/clean up an existing Task's directory. Creation and removal
of managed directories follow their normal lifecycle rather than switch-triggered
replacement. Explicit external directories remain user-owned.

### D39 - Offline physical cleanup does not block an admitted logical Chat deletion

Confirmed by the user on 2026-11-19 (directory question 3A). Once permanent Chat
deletion has been admitted, commit its durable logical deletion without waiting
for every Device holding Chat data to be online. A Device that is offline cleans
up its Chat-exclusive managed directories/data after it reconnects and learns
the deletion. Report logical deletion separately from physical cleanup completion;
do not claim offline bytes have already been erased or allow stale state to
resurrect the deleted Chat.

The question assumed deletion was already permitted and no unfinished execution
needed handling. It does not establish that an unreachable Device is idle or
settle every execution-admission rule. D44 subsequently settles Project deletion:
its logical commit does not wait for remote stop confirmation, and each Device
applies deletion when observed. Preserve external resource/user directories and
managed data retained by another owner/reference;
shared Media reclamation continues to use the existing reference-aware boundary.
The deletion identity follows D30 rather than the current Agent or Device path.

### D40 - Reject Agent deletion while its work remains unfinished

Confirmed by the user on 2026-11-19 (execution-lifecycle question 1A). If the Agent
still has executing Turns, nonterminal background Tasks it started, pending input,
or unresolved approvals, reject deletion and identify the unfinished work. The
user resolves/stops/cancels that work through the existing controls before retrying
deletion. Do not make Delete Agent implicitly stop or cancel the work for them.

A Chat having switched to another Agent does not erase the original Agent's
provenance on its still-running Task. Do not equate lost contact or an unconfirmed
stop with completed execution. Admission must coordinate with new work so that a
check followed by deletion cannot race another accepted start. Retention after an
admitted deletion follows D34; merely being referenced by completed history does
not block deletion.

### D41 - Collaboration Chats retain their fixed executor identity

Confirmed by the user on 2026-11-19 (execution-lifecycle question 2A). Retain ADR
0053 D30 for collaboration Chats: the configured Agent or ACP runner identity and
driver kind are fixed at creation. Neither idle state, human participation nor
unified Chat storage lifts this restriction. Using another executor requires a
new collaboration Chat with explicitly supplied context, not replacement inside
the existing collaboration.

Ordinary user-originated Chats retain the Agent-switching behavior in D7/D18.
The unified identity, history storage and shared Chat services do not imply
identical executor-change policy for every origin. Existing collaboration
configuration refresh, inherited authorization, result routing and source
provenance retain their established boundaries. D49 separately changes execution
Device/directory inheritance to creation-only; it does not loosen executor identity.

If the fixed executor is deleted after D40's admission check, retain collaboration
history without silently substituting another executor. The ordinary-Chat
replacement path in D34 does not apply to that fixed collaboration; subsequent
work with a different Agent starts a new collaboration.

### D42 - Projects may have no resource directories

Confirmed by the user on 2026-11-19 (Project-boundary question 1A). A Project may
be created with an empty resource collection and may lose its last resource
association while retaining the Project and its Chats. Project organization does
not depend on at least one directory. Remove ADR 0019's non-empty collection and
mandatory primary-resource requirement from the target model; optional execution
defaults remain separate under D9/D10.

Removing the final association follows the same D21-D23 rules as any other
removal. It does not move existing Chat working directories, erase source files,
change fixed membership, or imply the Chat has no filesystem access.

### D43 - Any paired Device may manage Project metadata

Confirmed by the user on 2026-11-19 (Project-boundary question 2A). Project
metadata can be created and changed through any paired Device under the existing
authorization system, then synchronized to the other Devices. Do not designate
one fixed Project-management Device or require a former resource Device to be
online for metadata changes such as rename and association removal.

Actual source-directory validation and operations remain on the Device owning
the directory: adding/replacing a resource requires target validation under D23,
and metadata access does not grant file permissions or make a remote path local.
A Project update does not copy resource files. Concurrent metadata edits,
revision/conflict handling and synchronization publication use the existing
architecture adapted to this ownership model; those mechanics are engineering
work, not a new user-configured management authority.

### D44 - Project archive/delete commits metadata now and propagates through Sync

Confirmed and clarified by the user on 2026-11-19 (question 3). Archive or
permanent deletion changes Rua-owned Project state on the initiating Device and
propagates through normal synchronization. The operation does not wait for every
resource/execution Device to be online, acknowledge the change, or confirm work
has stopped. After the necessary local durable commit, treat the metadata
operation as complete rather than retaining a pending request until remote work
finishes. This records the user's explanation, which differs from the offered
option B's wait-and-complete-later wording.

Archive preserves Project/Chat data; permanent deletion retains D4's cascading
logical deletion of member Chats and D39's deferred managed-data cleanup. Preserve
a durable deletion fact so late replica updates or results cannot recreate the
Project/Chats. Devices apply deletion when they learn it: disallow new work for
the deleted owner, stop its remaining local work through the existing lifecycle,
and safely reclaim eligible data. Until an offline Device receives the deletion,
it may still execute; metadata success must not claim immediate global stop or
physical erasure. Retain enough execution state to confirm cancellation and to
avoid treating a still-running Task as finished merely because its Chat is deleted.

This replaces ADR 0019 D7's requirement to establish Project-wide stopping before
completing the metadata operation. D45 separately retains execution-disabling
Archive behavior on Devices that have observed the state. Existing confirmation/
order affordances are not otherwise redesigned by this connectivity decision.
Agent deletion remains the distinct D40 operation and still rejects unfinished
work; do not silently apply this Project metadata rule to Agent deletion.

### D45 - Archive removes normal execution entry points and applies the existing stop semantics

Confirmed by the user on 2026-11-19 (Archive meaning B), with the clarification
that the normal UI already removes New Chat entry points for an archived Project.
Keep that ordinary interaction: Archive is execution-disabling rather than a
purely visual collection state, and users do not need a new workflow to prevent
creation inside the archived Project. D44 still commits metadata immediately and
propagates it without waiting for all Devices to acknowledge stopping.

On a Device that has observed Archive, apply the established Project stopping
behavior and check that state at existing execution-admission boundaries. This
is a backend consistency backstop, not a new permission system or a second
work-admission product concept. It covers a previously opened page submitting
stale input, another client with stale UI, queued execution and automatic
continuation from background results. Do not start another Project Turn merely
because such an asynchronous trigger bypasses the removed New Chat button.

A Device that has not yet received the state may still run; D44 does not promise
instantaneous global enforcement. Results arriving after observed Archive may
be retained by their normal history/result owners without waking new Project
execution. Reopening retains the existing rule that stopped work is not
implicitly resumed. Do not expand this decision into an offline command queue,
a mandatory all-Device check, or additional user-facing approval/configuration.

### D46 - Remote foreground execution is cancelled when its execution connection is lost

Confirmed by the user on 2026-11-20 (execution-boundary question 1A). When the
execution Device detects that the inter-Device connection owning a remote
foreground operation has closed, cancel that foreground execution through its
normal lifecycle. This is not triggered merely by a browser/viewer disconnect,
and ordinary transport recovery within a live connection is not itself proof
that the execution connection has closed.

Background Tasks already accepted by the target remain governed by D16 and
continue independently. Connection loss does not itself adopt foreground work
into a background Task. Serialize foreground cancellation against the existing
completion/adoption boundary: if target-side Task acceptance already occurred,
a lost acknowledgement does not turn that accepted Task back into foreground
work. Reconcile the authoritative target state instead of inferring ownership
from which response reached the caller.

Cancellation is a request to stop, not proof that side effects were rolled back
or that the operation has already stopped. Natural completion may win the race.
Until target-side terminal evidence is available, the originating Chat reports
unknown/unconfirmed status and must not automatically replay the operation.
Exact disconnect detection and reconciliation mechanics remain engineering work.

### D47 - Task Retry preserves the original resolved execution location

Confirmed by the user on 2026-11-20 (execution-boundary question 2A). An explicit
Retry uses the execution Device and working directory resolved for the Task's
first execution. This includes values originally omitted by the caller and
resolved from defaults; retry does not resolve them again from the Chat's new
Device or directory. A different execution location requires a new Task.

Retain ADR 0031's Retry contract: the Task ID and immutable Spec remain the same,
the next Attempt uses the retrying Turn's effective authorization snapshot, and
historical execution provenance is not rewritten. This does not reload mutable
Agent defaults or freeze old permissions. Preserve resolved placement alongside
the Task's existing execution provenance without mutating its immutable Spec.

If the original Device or directory is unavailable, report that condition
instead of redirecting execution, copying files, or queuing a future offline
replay. Retry remains explicit and subject to the existing terminal-state,
dependency and concurrency checks; it is not recovery from an unknown outcome.

### D48 - Exit ACP binding before switching the Chat runtime Device

Confirmed by the user on 2026-11-20 (execution-boundary question 3A). A Chat with
an active ACP binding must explicitly leave ACP mode through the existing exit
flow before its runtime Device can change, even when its foreground Turn is
idle. D18's idle admission and D11's cooperative handoff still apply. Do not
automatically exit ACP as a side effect of selecting another Device.

After an admitted Device switch, the user may enter ACP through the normal flow
on the new Device. The switch does not migrate, automatically restore, or
rebuild the external runner's session and does not transfer its files. Existing
Chat history and attachment retention follow the normal Chat contracts.

Agent switching already treats an active ACP binding as a blocker in the
existing Project Chat service; preserve that rule in the unified model. This
decision extends that explicit exit requirement to runtime Device switching,
without loosening collaboration executor identity under D41 or silently adding
ACP restrictions to unrelated configuration changes.

### D49 - Collaboration execution positions are inherited at creation and independent afterward

Confirmed by the user on 2026-11-20 (collaboration execution-position option A).
A collaboration Chat initially inherits the creating source execution's actual
Device and working directory. Later changes to the source Chat's Device or
directory do not move, reconfigure, or restart an existing collaboration Chat.
The collaboration's running work, queue, approvals, connectivity or ACP binding
therefore do not block a source change that only changes execution position.
The source must still meet its own normal change-admission and handoff rules.

Creating a collaboration at the same location does not imply continued position
binding or isolated directory contents. Inherit the resolved location rather
than copying a managed-default selector and accidentally resolving it to a new
empty directory. If that location is a source Chat's managed directory, sharing
its use does not transfer exclusive ownership. Each Chat retains its own managed
default under D38 when selected, and reference-safe cleanup under D39 must protect
data still retained by another Chat.

A later explicit position change belongs to the collaboration itself and follows
its own idle admission, cooperative handoff and ACP constraints (D18/D11/D48).
Do not clear its queue, resolve approvals, exit ACP or copy files as a side effect
of a source change. D41 still fixes the collaboration's Agent/ACP runner identity
and driver kind; neither independent placement nor ending an ACP session permits
substitution of its executor or driver.

Only execution Device and working-directory inheritance change. Security
configuration continues to follow the source Chat under ADR 0053 D28 and ADR
0028 D7, with immutable claimed-Turn snapshots and coherent admission of later
execution. A position-only source change must not put the collaboration into a
pending inherited-position update or prevent it from starting its own next Turn.
Source deletion retains the existing last-materialized-configuration rule.

This revises the execution-position portion of those ADRs' environment
propagation rule in the target model. It does not remove other established
configuration/security inheritance or broaden permissions. Configuration epoch,
managed-directory reference retention and independent authority routing are
technical integration work; no new cross-Chat position-change queue is required.

## Technical design candidates - Device-targeted resource operations

The user introduced an SSH-like model over iroh/QUIC on 2026-11-19. The reframed
interview confirmed execution placement, explicit file access, and partial
availability in D12-D14. Those answers replace the earlier unanswered resource
questions; the local-computation-only recommendation was not adopted.

The following schema and implementation details remain under discussion.
Previously confirmed decisions cited below constrain these recommendations; the
remaining details are not additional confirmed interview decisions:

- D27 selects Device plus real path for Agent-facing locations. Preserve stable
  Project association identities under D22 without requiring resource-ID-based
  Tool addresses. Exact Tool schemas and platform path handling remain open.
- Expose operation-specific file tools, explicit command execution, and explicit
  file transfer. Remote execution identifies its target; an omitted working
  directory follows D28. Never inherit an implicit last-connected remote shell
  or substitute another Device on failure. Exact tool names and schemas remain
  open; supplied paths are resolved in the target execution environment.
- D29 selects a compact execution-position and resource Device/path inventory;
  Device capabilities and detailed contents are discovered on demand. Return
  actual Device/path provenance, bounded output, exit/error evidence,
  and usable continuations. Large binary results use existing artifact/media
  mechanisms instead of inline text encoding.
- Keep authorization in the existing system (D6), enforce it on the target, and
  launch external processes through that Device's Runtime Authority. Device
  pairing alone does not authorize every command, and cwd is not a sandbox.
- Reuse shared Device Sessions, Command and Bulk Transfer streams (ADR 0037).
  Existing sync_ls/sync_send and remote Terminal controls are useful foundations,
  not a complete Agent-facing remote execution contract.
- Implement D15-D17 through the existing Task lifecycle/control concepts,
  extending placement while defining remote admission, foreground adoption,
  references, and result delivery. Configuration changes and Chat handoff while
  Tasks remain active follow D18-D20; cross-device races and recovery still need
  protocol specification. Foreground disconnection follows D46, and explicit
  Retry keeps original resolved placement under D47.
- Preserve operation identity across observation and reconnect. Distinguish
  unavailable-before-dispatch, accepted/running, terminal failure, and unknown
  outcome after dispatch. Query uncertain outcomes instead of blindly replaying
  mutations. Cancellation is confirmed only by target-side evidence. Do not
  queue stale foreground intent for execution when an offline Device returns.

Before selecting the final tool shape, evaluate representative Agent workflows:
same-named paths on different Devices, remote photo inventory with bounded output,
missing runtime capability, copying an artifact for local processing, and a lost
response after a mutation. Compare wrong-target actions, unnecessary transfers,
repeated mutations, and successful recovery as well as completion rate.
