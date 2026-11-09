# ADR 0018: Chat Address, scope, and lazy materialization

**Status:** Accepted

## Context

Rua has one Chat domain model with two ownership scopes: Agent and Project.
A bare Chat ID is unique enough for a directory name inside one Workspace, but
it is not a complete product, routing, synchronization, or authorization
identity. The same runtime services must work for both scopes without deriving
missing identity from browser selection or the currently executing Agent.

The current domain model represents this explicitly in
`libs/rua-core/src/chat.rs`: `ChatScope` is either Agent or Project, and
`ChatAddress` combines that scope with `chat_id`. Console paths, notification
URIs, Conversation Authority, Task ownership, Media operations, deletion, and
session replication all depend on the complete address.

Rua also permits navigation to a valid Chat address before durable storage
exists. This Lazy Chat state avoids persisting empty Chats merely because a
route was opened, while still allowing deep links and cross-device navigation
to identify the intended future Chat.

## Decision

### D1 - Chat Address is the complete Chat identity

A Chat Address is exactly one of:

```text
Agent scope:   { agent_id, chat_id }
Project scope: { project_id, chat_id }
```

The scope discriminator and owner ID are part of equality, hashing, routing,
storage resolution, synchronization identity, authorization, and cache keys.
No API crossing a scope boundary may identify a Chat by `chat_id` alone.

The canonical Console routes are:

```text
/agents/{agentId}/chats/{chatId}
/projects/{projectId}/chats/{chatId}
```

Rua-generated Chat IDs use UUID v4. The accepted external identifier contract
is broader: a supplied Chat ID must parse as a UUID, but its UUID version is not
part of Chat Address validity. Scope, rather than UUID version, provides the
ownership namespace.

An open Chat derives its identity from the complete route. Global Agent or
Project selections are navigation preferences and cannot complete, relabel, or
override an already open Chat Address.

### D2 - Scope is immutable

An Agent-Scoped Chat never becomes Project-scoped, and a Project-Scoped Chat
never becomes Agent-scoped. Moving content between scopes requires creating a
distinct Chat with a distinct Chat Address through an explicit product action.

Agent assignment inside a Project-Scoped Chat is execution configuration, not
Chat ownership. Changing that assignment preserves the Project Chat Address.
Likewise, an Agent-Scoped Chat remains in its Agent namespace even when work is
routed to another authority Device.

### D3 - Storage is resolved from scope

The complete address resolves the owning Workspace before Chat storage is
opened:

```text
Agent:   <rua-root>/workspaces/<agent-id>/
Project: <rua-root>/projects/<project-id>/
```

Chat storage modules consume this Workspace through `ChatScopeStore`. They do
not infer scope from a path supplied independently by a caller. Passing both a
reconstructed address and an unrelated Workspace path is invalid because the
two values can disagree.

### D4 - A valid address may be unmaterialized

A syntactically valid complete Chat Address is meaningful even when no Session
File, Message Log, or Chat Index row exists. This is a Lazy Chat.

A history read returning not found identifies the unmaterialized state; it is
not by itself an invalid route. Read-only navigation, draft editing, ACP runner
discovery, and resumable-session discovery do not materialize the Chat.

### D5 - First stateful acceptance materializes one address idempotently

The first accepted stateful operation creates durable state for the same Chat
Address. A first Submit materializes Chat metadata, running configuration, and
the first Turn as one logical ownership transition. ACP Enter may materialize metadata
and an ACP binding without inventing a Turn. A first Chat Media upload may also
materialize the Chat and its initial authority/configuration without inventing
a Turn; a media read or preview remains read-only and does not materialize it.

Materialization must not create a different Chat ID, change scope, or depend on
a browser-local draft marker. Repeated or concurrent first operations must
converge on the same durable Chat and must not duplicate the first accepted
Turn.

### D6 - Replication preserves the address unchanged

Every Stable Conversation State publication carries the canonical Chat
Address. Session replication, History Replacement, Full Export, repair, and
inbound apply must not reconstruct scope from the executing Agent.

If a payload's embedded Chat Address conflicts with its synchronization target,
inbound apply rejects the payload as invalid. It must not relocate the data or
silently reinterpret a Project Chat as an Agent-Scoped Chat.

## Hard invariants

- `chat_id` is never globally authoritative without its scope.
- Server-minted Chat IDs use UUID v4; accepted caller-supplied Chat IDs may use
  any UUID version.
- Route identity and persisted Chat Address must agree.
- Scope cannot change after the address is minted.
- Browser state cannot establish durable Chat existence.
- A Lazy Chat is absent from durable list/search results.
- First materialization uses the originally addressed Chat ID and scope.
- All durable publication preserves the exact canonical Chat Address.
- Project Agent selection never changes Project Chat ownership.

## Authority, persistence, concurrency, and failure boundaries

**Authority.** The route or command supplies identity; Conversation Authority
owns stateful acceptance and first materialization. Frontends and transport
adapters may carry an address but cannot reinterpret it.

**Persistence.** Before materialization there is no completed durable Chat
record, Chat Index row, or synchronized Chat fact. Current materialization writes
the Session Header, running configuration, Chat Index projection and optional
first Turn through separate file/database operations; it is not a cross-file
transaction. Operations that do not create a Turn, such as ACP Enter or first
Media upload, need not create Message Log content. Every durable artifact remains
rooted in the Workspace selected by the immutable scope.

**Concurrency.** Concurrent first submits, ACP Enter, or remote delivery target
one address. The authority intake and idempotency seams must serialize acceptance
or safely resume the same materialization; they must not mint another address or
duplicate an accepted first Turn.

**Failure.** Because the physical writes are not one transaction, a failed first
operation may leave partial state under the correct scope root. Retrying must
complete or repair that same Chat idempotently. It must not leave mixed-scope
files, publish a different identity, or create a duplicate first Turn. Missing
history for a Lazy Chat is normal; malformed scope, owner mismatch, or payload
address mismatch fails closed.

## Consequences

- Agent and Project Chats share one set of Chat services and differ through
  typed scope policy rather than parallel implementations.
- Deep links and refreshes do not depend on a selected-Agent browser store.
- Runtime registries, caches, Tasks, Media, approvals, and synchronization must
  use `ChatAddress` keys.
- Lazy navigation avoids empty durable Chats while preserving a stable future
  identity.
- Callers that currently pass a bare Chat ID need a scope-bound adapter before
  entering domain services.

## Rejected alternatives

- Treat Chat ID as globally sufficient and infer scope from the active page.
- Store Project Chats under the selected Agent's Workspace.
- Materialize every Chat when its route is opened.
- Require a browser-local draft marker before accepting a deep link.
- Reconstruct synchronization identity from the executing Agent.
- Repair address conflicts by moving data between scope roots.
- Retain unscoped `/chat/{chatId}` as an identity-bearing route.

## Implementation and verification

Current implementation anchors include:

- `libs/rua-core/src/chat.rs` for `ChatScope` and `ChatAddress`;
- `libs/rua-core/src/chat_store.rs` for scope-bound Chat storage;
- `crates/rua-server/src/services/project_chat.rs` for Project materialization;
- `crates/rua-server/src/services/conversation_input_intake.rs` for accepted
  input and first-turn ownership;
- `crates/rua-server/src/services/console_chat.rs::resolve_chat_id` for the
  current UUID-syntax validation boundary;
- `crates/rua-sync/src/chat_adapter.rs` and
  `crates/rua-sync/src/evergreen_chat_adapter.rs` for scoped replication;
- `console/src/userNotifications/chatViewer.ts` for canonical scoped addresses.

`crates/rua-server/src/services/console_chat.rs::resolve_chat_id` currently
uses `Uuid::parse_str` and preserves the supplied value; it does not inspect the
UUID version. Existing endpoint tests predominantly use v4 fixtures and do not
establish a v4-only acceptance rule.

### Current implementation gap

The materialization helpers do not provide one crash-atomic transaction across
Session Header, `running_config.json`, Chat Index, authority placement and first
Turn. For example, targeted Agent materialization writes metadata, then running
config, then scope defaults, while the submitted Turn is appended afterwards.
Tests cover the expected completed state and several idempotency cases, but do
not inject a failure at every write boundary and prove repair convergence. The
accepted contract is therefore idempotent completion of one address, not
all-or-nothing multi-file atomicity; recovery tests remain required.

Verification must cover both scope variants, direct deep links, history
not-found as Lazy state, concurrent first submissions, ACP-only and Media-only
materialization, read-only Media access without materialization,
address-preserving publication, rejection of mismatched sync targets,
server-generated UUID v4 IDs, and acceptance of valid caller-supplied non-v4
UUIDs.
Relevant suites include
`crates/rua-server/tests/infra/project_chat_materialization.rs`,
`crates/rua-server/tests/infra/targeted_chat_materialization.rs`, and
`libs/rua-core/tests/unit_identity/chat_scope_store.rs`, plus the Lazy Chat
cases in `crates/rua-server/tests/contract_console_http/chat_media.rs` and
`crates/rua-server/tests/contract_acp_http/acp_routes.rs`.

## Related decisions

- ADR 0019: Project and Project-Scoped Chat resource binding.
- ADR 0020: Session File and append-only Message Log.
- ADR 0021: Chat Index as a complete non-authoritative projection.
- ADR 0022: Conversation Authority placement and epoch fencing.
- ADR 0030: Chat Media ownership and authority-routed transfer.
- ADR 0035: Chat CLI single-request adapter and resident server boundary.
