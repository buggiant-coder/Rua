# ADR 0019: Project, Project-Scoped Chat, resource bindings, and sidebar organization

**Status:** Accepted.

## Context

Repository work needs a stable directory on a specific Device. Treating that
directory as an optional `cwd` on an Agent-Scoped Chat is insufficient: it does
not provide a durable Project identity, cannot express remote ownership, and
allows later execution to drift to a different Device or path.

Rua therefore models Project separately from Agent. A Project names a logical
work context and binds it to one or more external user-owned directories on one
resource Device. Project-Scoped Chats reuse the common Chat, Conversation Authority,
history, Task, Media, and synchronization systems; they do not create a second
conversation runtime.

Current implementation separates portable Project descriptors from private
resource bindings in the Project services and resolves Project Chat execution
through `crates/rua-server/src/services/project_chat.rs` and the shared
`ChatAddress` model.

## Resource directories and sidebar organization

### Motivation and scope

A logical Project may span an application, documentation, and shared tools in
separate directories. Equating Project identity with one directory prevents that
workflow. Sidebar organization is also incomplete: the Project row exposes only
Chat management, while rename and lifecycle operations live in the Projects
Overview, and ordering offers only three automatic modes.

The revised model separates Project identity, resource directory membership,
Chat working directory, and user sidebar preferences. It retains one immutable
resource Device per Project, Project-owned Conversation State, the shared Chat
runtime, and no execution fallback to a controller Device. Multiple resource
Devices within one Project are outside this revision.

OpenAI's public [Projects and chats documentation](https://learn.chatgpt.com/docs/projects?surface=app)
describes local Projects with multiple folders and a primary folder for new
Chats. This informs the interaction model, not an assertion about OpenAI's
internal implementation. Rua's Device placement, permissions, and lifecycle
remain governed by its own contracts.

### R1 - One Project has a versioned directory collection

The private Project Resource Binding contains one resource Device,
a non-empty collection of resource directories with stable directory IDs, one
primary directory ID, and a monotonically increasing revision. Each directory
entry stores its private `resource_root` on the resource Device. Directory order
and path strings are not Project identity. Explicitly changing a directory's path
retains that entry's ID and its durable Chat references. Remove/add remains a
distinct operation that creates a new directory identity and cannot remove a
directory referenced by existing Chats.

Project Descriptor remains portable display metadata. It may describe directory
IDs, labels, primary selection, and binding revision, but must not turn a display
path into execution authority or replicate private binding records. Learning a
Descriptor does not authorize another Device to resolve its paths.

Creation and edits are validated by the resource Device. Path expansion, lexical
normalization without symlink canonicalization, and directory existence checks
remain in force. Exact duplicate normalized paths are rejected within a Project;
ancestor/descendant overlap is reported without granting their common parent as
a writable root. The replacement for global binding uniqueness permits
separate Projects to reference the same directory, with a shared-write warning.
No directory attachment copies files or writes a Project marker into the root.

### R2 - Primary directory and Chat working directory are distinct

The primary directory supplies the default for a new Lazy Project-Scoped Chat.
The Chat header has no folder selector; users change the default directory in
Project settings. New Chat submission does not reuse an old browser folder
selection. First materialization persists the primary directory ID as the Chat's
working directory selection. An existing Chat retains its selection when the Project primary
changes; Agent switching does not change it. Project-origin collaborations
inherit the source Chat's working directory rather than rereading the current
Project primary.

Each admitted execution resolves a coherent binding revision, working directory,
and resource policy on the resource Device. Queued input must validate the
current binding when admitted; it must not revive a removed directory through a
stale cached path. Missing directories produce an explicit unavailable result,
never a replacement cwd or a controller-side fallback.

Removing a directory never deletes its files. The initial rule rejects
removal of any directory referenced as a durable Chat working directory and
identifies the affected Chats, including archived or otherwise hidden records.
Removing the primary requires selecting another attached directory in the same
edit; removing the last directory is rejected. Moving an existing Chat to a new
working directory is not implicitly introduced by primary selection or removal.
An explicit directory rebind changes that directory's execution path for every
Chat referencing its stable ID; the UI explains this before saving. The old path
need not exist, while the replacement must open as a directory on the same Device.

### R3 - Resource configuration is enforced by the execution module

One resource-resolution module owns validation and returns the resource Device,
Chat working directory, attached resource roots, and binding revision to file,
Shell, Git, MCP, ACP, Skill, and Hook execution integrations. Callers must not
independently select a primary root or reconstruct authorization from display
metadata. Integrations unable to honor multiple roots must report that limitation
rather than silently run with a narrower or wider configuration.

The default writable roots become the attached directory set; reads outside it
retain the existing Project Resource Policy behavior unless another rule denies
them. Agent security, Tool Guard, reusable approvals, and explicitly selected
more permissive profiles still apply. Never substitute the common ancestor of
attached directories for the directory set. Path identity remains lexical while
operation-time security checks retain their symlink and sensitive-path checks.

The built-in file editor checks the canonical destination against the individual
roots. Shell/Python and opaque MCP operations use the existing explicit approval
path because their writes cannot be confined by cwd alone; this is a Tool Guard
policy, not an OS sandbox. Simple read-only shell commands retain their existing
read behavior. Explicit full-access settings and existing approval reuse remain
applicable. ACP currently exposes a single cwd, so multi-directory Projects reject
ACP activation with an actionable unsupported-resource message until the adapter
can carry and enforce the directory set.

Default Git operations and project-level instruction/configuration discovery use
the Chat working directory. Other repositories require an explicit directory
selection. Attachment alone must not automatically activate or merge every
attached repository's skills, Hooks, or configuration. File-scoped instructions
continue to apply where relevant; Hook trust remains explicit.

Binding edits use expected-revision checks and atomic persistence. The
first implementation rejects resource edits while Project-owned execution is
active, including running ACP turns, queued inputs, pending approvals/questions,
and nonterminal Background Tasks. An idle ACP binding only records executor
identity and does not block resource edits or Archive. Archive stops running ACP
Turns while preserving the binding; an idle binding is not work requiring a stop.
Validation and
commit must be serialized with execution admission so a check-then-start race
cannot change resources under running work. This narrow mutation gate does not
serialize ordinary concurrent Chats. No durable offline edit queue is added.

Resource-device mutation is owned by the server's Project lifecycle module.
Local HTTP and Device command adapters call its resource editor; they do not
assemble admission, work checks, Chat-reference validation, or publication.
The editor holds the existing Project admission exclusion through canonical
commit and releases it on failure or cancellation. The store accepts resource
commits through the edit guard and persists the exact checked candidate with a
revision check; primary selection remains a distinct operation that validates
only the selected directory. Committed descriptor publication follows the write
inside the editor and cannot roll back local success. Inbound descriptor merge
continues through its existing separate path.

### R4 - Project actions separate navigation from lifecycle

A Project row should expose these actions through the same operations used by
Projects Overview, rather than duplicating mutation logic:

- Pin/unpin and edit Project (name, directories, and primary selection).
- Move to a sidebar section and manage Project Chats.
- Open a selected resource directory when the current client and resource Device
  support it; do not open a controller-local lookalike of a remote path.
- Hide from sidebar, with restoration through Projects Overview.
- Archive Project through the existing lifecycle preview and confirmation flow.

Hide and pin affect navigation only: neither stops execution, archives Chats,
changes access, nor deletes Project data. Project Archive retains its hard-stop
semantics. Permanent deletion remains available only for an Archived Project and
never modifies any attached external directory. Reopen validates retained
bindings on the same resource Device and never resumes discarded work.

Individual Project Chat archive and restore already exist: the sidebar and
Project Chat management update the Chat's `archived` listing metadata through
the shared Chat update path. An Archived Project still rejects Chat updates until
reopened.

A Project-menu bulk “Archive chats” action is an extension of this existing
capability, not a new archive concept. It snapshots the Project's unarchived
Chats, including collaboration Chats, skips Chats with active or pending work,
and reports archived, skipped, and failed outcomes per Chat. Eligibility is
rechecked at mutation time; an offline authoritative Device causes a reported
failure rather than queued intent. It must never call Project Archive. Clearing
archived Chats is permanent deletion and remains a separate action.

### R5 - Manual ordering is independent of Project activity

Add `manual` alongside name ascending, creation time descending, and last Chat
activity descending. Starting a drag in an automatic mode snapshots the visible
Project order, applies the move, switches to manual, and announces the change.
Switching back to an automatic mode preserves the saved manual order. New
Projects append to their section in manual mode; Chat activity, rename, route
selection, and background loading do not reorder existing manual entries.

The sidebar has one level of named sections and a pinned area. A pinned
Project appears once and remembers its original section for unpinning. Sections
and Projects can be reordered; removing a section moves its Projects to the
default section rather than deleting them. Cross-section movement is explicit.
Keyboard move-up/down and move-to-section actions provide equivalents to drag;
mobile interaction must avoid confusing a drag with scrolling or opening a Chat.
Navigation mutations preserve the selected Chat, composer draft, and runtime.

Project Sidebar Preferences own pinning, hidden state, section membership,
section order, manual Project order, and sort mode. They are not Project
Descriptor fields or Chat activity. Selection, expansion, and scroll position
remain disposable browser navigation hints. Persist sidebar preferences as
separate user-owned state and synchronize them across paired Devices. Use the
existing Sync ordering/conflict mechanisms: independently changed entries must
merge without replacing the entire sidebar, and concurrent moves of the same
entry resolve deterministically. Deleting a section retains a tombstone so a
stale move cannot resurrect it; its remaining entries appear in the default
section. Local stale edits use revision conflicts rather than overwriting newer
state silently.

### R6 - Implementation boundaries and required verification

Changes span `rua-project` model/store/path validation; Project endpoints and
resource-device commands; Chat materialization, execution admission and resource
policy; Git and runner integrations; portable Project synchronization; and Console
Project editing, menus, sorting and sidebar preferences. Preserve Project scope
storage, fixed authority placement, and external-file ownership throughout.

Required acceptance covers:

- Multi-directory creation/edit is validated on the resource Device, rejects
  duplicate entries and stale revisions, and cannot race execution admission.
- Changing primary leaves existing Chats and source collaborations on their
  selected directories; removal cannot orphan those references.
- Tools honor individual roots and existing security policies, including symlinks;
  unattached sibling paths are not granted access through a common ancestor.
- Missing directories, offline Devices, and unsupported runners never trigger
  execution elsewhere; portable metadata never becomes path authority.
- Archive stops Project work; hide and pin do not; deletion leaves every external
  directory untouched. Individual Chat archive/restore remain supported and
  independent of Project lifecycle; bulk archive reuses that metadata operation.
- Drag, keyboard moves, automatic/manual switching, reload, section removal and
  restored hidden Projects preserve consistent ordering and active Chat state.
- Cross-Device preference convergence, conflicting moves, and stale section
  references are tested.

The default policies allow cross-Project directory reuse, reject removal of
Chat-referenced directories, require idle-only resource edits, skip busy Chats
during bulk archive, and synchronize sidebar preferences. Implementation and
release verification are separate from acceptance of these decisions.

## Shared Chat, storage, and lifecycle rules

### D1 - Project is a logical entity, not an Agent

A Project has a stable Project ID and portable Project Descriptor. The
descriptor carries display metadata, lifecycle, the resource Device identity,
and a non-authoritative display path. It does not carry the private
`resource_root`, Agent configuration, Conversation State, or a copy of the
repository contents.

Agents may be selected to execute individual Project Chat Turns. That selection
does not change Project identity and does not make the Project a child of the
Agent.

### D3 - Project-Scoped Chat has Project ownership and mutable Agent selection

A Project-Scoped Chat has address `{project_id, chat_id}`. Its durable history,
configuration, Tasks, approvals, and Media remain attached to that address.

The Chat's selected Agent may change for later Turns. Every claimed Turn freezes
its own Agent assignment. Agent changes do not create another Chat, rewrite
historical Turn assignments, move storage, or change the resource Device or
working-directory selection.
The explicit switch publishes a complete configuration epoch under ADR-0028.
Collaboration Chats are the exception: executor identity and driver kind are
fixed at creation; replacing them requires a new Chat (ADR-0053).

### D4 - Project Workspace and Project Resource Root are different

A Project-origin collaboration remains in the same Project and inherits the
source Chat's working-directory selection even when another Agent or ACP runner
executes it. More generally, Chat default execution environment is separate from Agent identity
and managed Chat storage; ordinary Agent Chat collaborations also retain their
source environment rather than switching to the target Agent Workspace.

Rua-managed Project data lives below:

```text
<rua-root>/projects/<project-id>/
```

The external repository or working directory remains at `resource_root` on the
resource Device. Session files, Message Logs, Chat Index data, Media, Task
metadata, and synchronization state must not be written into the external root.

Workspace directories are created lazily and idempotently when durable Project
data is first needed on a Device. Learning a synchronized descriptor alone does
not require an empty local Workspace.

### D5 - Conversation Authority is pinned to the resource Device

Project Chat selection is read from the Session header's `selected_agent_id`.
Its selection timestamp and revision come from the latest Project Agent switch
audit event; an unswitched Chat uses its creation time and revision 1. Reading
history or addressing an existing Chat does not materialize missing scope fields
or capture new Agent defaults. Draft creation writes a complete current Chat and
rejects an existing index or storage directory rather than repairing it.

Project-Scoped Chat uses the shared Conversation Authority state machine, but
its placement is fixed to `resource_device_id`. The user cannot switch authority
to another Device while retaining the same Project binding.

All Project file, Shell, MCP, ACP, Skill, and related execution starts from the
resource Device and uses the selected working directory and attached resource
roots according to Runtime Authority and security policy. A controller Device routes commands; it never interprets the
remote path or executes the work locally.

### D6 - Project work remains independently concurrent by Chat

Distinct Project-Scoped Chats may run concurrently. Project does not impose a
global serialized execution lock merely because Chats share resource directories.

Filesystem conflicts are handled by tools, repository practices, and security
policy. Conversation Authority still guarantees at most one Active Turn per
Chat Address, not one Active Turn per Project.

### D7 - Lifecycle preserves the external resource

Archive performs the Project-wide hard-stop transition, stops new Project work,
and retains Rua-managed state and the Project Resource Binding. It also retains
each Chat's ACP executor selection: stopping a Turn must not invoke Exit ACP or
remove the Chat's binding. In particular, archiving a collaboration Chat does not
attempt to change its fixed executor. Permanent Project deletion is
available only after Archive; it removes Project-owned Rua data and releases
the private resource bindings, but never deletes or mutates external Project
resource directories.

Removing a Device from the network does not rewrite bindings. If the same
persistent Device identity returns and the path is available, the Project may
become available again.

Device removal does not implicitly Archive or delete the Project. Reopen
revalidates the stored directories on the same resource Device and never resumes
Turns, queued input, approvals, or Tasks that Archive stopped. Retained ACP
executor selections remain available without automatically starting work.
Archive and permanent deletion require explicit confirmation; deletion is
permitted only after Archive and does not require the resource Device to be
online once the Project-wide stop boundary has been established.

## Hard invariants

- A Project is not an Agent and does not own Agent defaults.
- One Project has one immutable resource Device and a versioned collection of
  resource directories with stable IDs and one primary directory.
- The portable Project Descriptor names the resource Device but never carries
  the private `resource_root`.
- Duplicate normalized paths within one Project are rejected; separate Projects
  may share a directory with an explicit shared-write warning.
- Path identity is lexical after expansion and normalization; symlink targets
  and filesystem inodes are not Project identity.
- Project-Scoped Chat identity is independent of selected Agent.
- Project data is stored in Project Workspace, never in the resource root.
- Project Chat authority is pinned to the resource Device.
- Controller Devices never execute Project work as a fallback.
- Different Project Chats are not globally serialized.
- Project deletion never deletes the user-owned directory.
- Project creation is acknowledged only by the resource Device after validating
  the directory; there is no offline pending Project record.
- Device removal does not cascade into Project Archive or Project deletion.
- Reopen validates the retained directory bindings and never resumes discarded
  live work.

## Authority, persistence, concurrency, and failure boundaries

**Authority.** The resource Device is authoritative for the private binding,
path validation, Project availability, and Project Chat execution. Portable
descriptors and replicated Chat state do not authorize another Device to use a
lookalike path.

**Persistence.** Project descriptors are portable; resource bindings are
device-private; Project Chat state is portable Conversation State under the
Project Workspace. These stores have different ownership and must not be
collapsed into one record.

**Concurrency.** Conversation Authority serializes each Project Chat. Multiple
Chats may execute concurrently against the same resource root. Project
lifecycle operations coordinate all Project-owned active work before committing
an archive or destructive state transition.

**Failure.** An offline Device, absent directory, failed path validation, or
failed routed command makes the operation unavailable. No durable controller
outbox replays stale intent when the Device returns, and no local fallback
changes the execution location.

## Consequences

- Project and Agent scopes share Chat infrastructure while preserving distinct
  Workspace ownership and placement policy.
- A Project Chat can change Agent without losing history or changing cwd.
- Remote controllers can view replicated stable history while execution remains
  unavailable when the resource Device cannot be reached.
- APIs must carry a complete Project Chat Address and cannot substitute an Agent
  ID for Project identity.
- Security and approval decisions evaluate the bound resource root on the
  execution Device.

## Rejected alternatives

- Add an arbitrary cwd field to Agent-Scoped Chat.
- Bind a Project permanently to one Agent.
- Store Project Chat history under the current Agent Workspace.
- Synchronize or copy the external resource directory as Project metadata.
- Rebind automatically when the Device or path is missing.
- Switch Project Chat authority away from the resource Device.
- Serialize all Project Chats behind one Project-wide lock.
- Delete the external directory when deleting the Project.

## Implementation and verification

Project Chat archive is implemented by `ProjectChatUpdate.archived` and the
shared Chat listing update, independently of Project Archive. Existing HTTP
coverage includes `project_chat_archive_filter_and_bulk_delete_archived_contract`
in `crates/rua-server/tests/contract_projects_http/projects_routes.rs`, plus
archive/restore assertions in that same contract binary. The Console sidebar
and `ProjectChatsDrawer` expose archive and management/restore respectively.

Current implementation anchors include:

- `crates/rua-project/src/model.rs`, `crates/rua-project/src/path.rs`, and
  `crates/rua-project/src/store.rs`;
- `libs/rua-core/src/chat.rs` and `libs/rua-core/src/chat_store.rs`;
- `libs/rua-core/src/workspace.rs` for Workspace ownership;
- `crates/rua-server/src/services/project_chat.rs`;
- `crates/rua-server/src/endpoints/projects.rs`;
- `crates/rua-server/src/services/conversation_authority_target.rs`;
- `crates/rua-server/src/services/chat_storage.rs`.

Verification must prove fixed resource-Device ownership, revision-checked
directory edits and stable Chat working-directory references, lexical path
normalization without symlink canonicalization, resource-Device validation,
exact scope-root selection, fixed authority routing, no controller fallback,
Agent changes preserving one Chat Address, concurrent Chats remaining
independent, Archive retaining the binding, and deletion leaving the resource
root untouched. Relevant coverage includes
`crates/rua-server/tests/infra/project_chat_materialization.rs`,
`crates/rua-server/tests/infra/project_device_command.rs`, and
`crates/rua-server/tests/contract_projects_http/projects_routes.rs`.

## Related decisions

- ADR 0018: Chat Address, scope, and lazy materialization.
- ADR 0022: Conversation Authority, placement, and epoch fencing.
- ADR 0028: Chat Running Config and Turn-stable snapshot.
- ADR 0030: Chat Media ownership and transfer.
- ADR 0031: Durable Task Runtime and Attempt state machine.
- ADR 0035: Chat CLI adapter and resident server boundary.
