# ADR 0030: Chat Media ownership and authority-routed transfer

**Status:** Accepted

## Context

Agent-Scoped and Project-Scoped Chats can attach images, audio, and other media.
The HTTP-connected Device may be a controller while another Device owns the
Conversation Authority. If upload commits only on the controller and relies on
ordinary Sync, input can reach the authority before the referenced bytes.

Media also needs a stable ownership rule. Project Resource Root is user-owned
execution data and must not receive Rua-managed attachments. A per-Chat media
tree would duplicate files referenced by several Chats in one Workspace and
would couple storage layout to the conversation route.

The current server has Chat-scoped media endpoints, authority placement checks,
transfer authorization, Device Command Bus integration, and scoped storage.

## Decision

### D1 - Chat Media is owned by the scope Workspace

Canonical storage is:

```text
Agent scope:   <rua-root>/workspaces/<agent-id>/media/<uuid>
Project scope: <rua-root>/projects/<project-id>/media/<uuid>
```

The owning Workspace comes from the complete Chat Address. A Chat does not gain
its own Workspace or `sessions/<chat-id>/media` tree. Project Media never enters
the external Project Resource Root.

### D2 - Media references are location-independent

Durable Conversation State stores the canonical reference `media/<uuid>`. The
reference does not encode a Device, absolute path, original filename, Project
resource path, transport URL, or local replica location.

Metadata such as content type, size, checksum, and display name may accompany
the managed object, but location is resolved through Chat scope and current
authority.

### D3 - Foreground operations are Chat-addressed

Assistant display images sourced from local files are imported before the
assistant Message is committed. This applies to Markdown images and closed
Markdown Plus `@figure` blocks, not ordinary links, code examples, Thinking,
Tool output, or user text. The Agent can name the original source; the runtime
owns copying bytes, creating the managed reference, and rewriting the persisted
display content. Import failure fails the output persistence operation rather
than falling back to a viewing Device's filesystem.

The media sidecar and Message metadata retain the source Device ID, original
`src`, and the execution working directory for relative sources, together with
MIME, byte size, and SHA-256. These are provenance, never a read fallback. The
destination absolute path is derived from the local scope Workspace; it is not
a portable identifier. Media replication preserves the complete sidecar,
including unknown metadata fields. Existing HTTP, data, and blob sources are
not downloaded by the local-file importer.

Console Markdown resolves managed references using the complete Chat Address.
Unimported local sources are not previewed on the controller Device. Ordinary
Sync remains a replica mechanism, not a prerequisite for foreground reads.

Upload, read, and preview accept a complete Agent or Project Chat Address.
Agent and Project HTTP routes differ only in address construction and
authorization; both call one media application service and one Workspace
resolver.

An endpoint must not infer the active Agent from browser state or expose an
unscoped upload that stores data before Chat identity is known.

### D4 - Foreground transfer terminates at Conversation Authority

The current authority Device accepts and atomically persists uploads needed by
the Chat. If a controller receives the HTTP request, it routes control through
the shared Device Session and transfers bytes through the bounded bulk-transfer
path to the authority.

Read and preview resolve the same authority. A verified local replica may serve
a read as an optimization, but absence of a replica cannot make media
unavailable while the online authority can serve it.

### D5 - Upload and input are fenced by Authority Epoch

Each upload carries the Chat Address and observed Authority Epoch. The target
commits only if it is still authority for that epoch. The response includes the
canonical media reference and accepting epoch.

Conversation input referencing the upload must be accepted by the same
authority generation. If placement changes between upload and Submit, the stale
submission is rejected and the client routes the bytes and input to the new
authority. An unreferenced old-authority upload is eligible for garbage
collection.

### D6 - First upload may materialize a Lazy Chat

A browser-only draft cannot upload against an undefined placement. The first
stateful media operation materializes the addressed Chat and initial authority
according to normal Lazy Chat rules, then performs the fenced upload.

This operation does not invent a Conversation Turn and does not create a second
draft or media-specific placement state machine.

### D7 - Conversation input carries references, not bytes

Input delivery contains validated `media/<uuid>` references and required media
metadata. It never embeds attachment bytes in the conversation command.

The authority verifies that required media is present and valid before
accepting input that depends on it. This makes the accepted Turn self-contained
on its execution Device.

### D8 - Authority adoption prepares required media

Before an Agent-Scoped Chat changes authority, the target derives the Required
Media Set from selected stable Conversation State and obtains missing bytes by
targeted transfer. Conversation manifest validation and media availability are
separate checks and both must pass before adoption commits.

Project-Scoped Chats do not use this switch path because their authority remains
pinned to the resource Device.

### D9 - Sync is background replication

Workspace Sync may replicate media for offline access, recovery, and future
reads. It is not the foreground correctness path for upload, input acceptance,
preview, read, or authority-switch preparation.

Deletion is reference-aware. Removing one Chat removes its media reference but
does not reclaim shared content still referenced by another retained Chat.
Physical blob reclamation belongs to Media garbage collection.

## Hard invariants

- Workspace scope owns Media; Project Resource Root never does.
- Durable media references contain no Device or absolute path.
- Foreground upload commits on the current authority Device.
- Upload and dependent input use the same Authority Epoch.
- Input commands carry references, never attachment bytes.
- Authority adoption verifies required media independently from history hashes.
- Sync is not required for foreground success.
- Shared referenced content is not deleted prematurely.

## Authority, persistence, concurrency, and failure boundaries

**Authority.** Conversation Authority decides foreground media placement and
acceptance. Media storage owns bytes and metadata; Device Session owns transfer;
Sync owns later replication.

**Persistence.** Upload uses atomic file commit under the scope Workspace.
Conversation State persists only canonical references. Transfer authorization
is short-lived runtime state and is not a durable media identity.

**Concurrency.** Epoch validation linearizes upload with authority movement.
Remote transfer retries use their operation identity and atomic commit rules;
separate uploads of identical bytes may still receive different Media UUIDs.
Input acceptance verifies availability after upload completion.

**Failure.** Stale placement returns a typed redirect or conflict. Interrupted
transfer leaves no committed partial object. Missing authority, invalid media,
or unavailable required bytes fails explicitly without local fallback or
waiting for a global Sync cycle.

## Consequences

- Controllers can upload to and read from a remote authority immediately.
- Agent and Project Chats share one media service.
- Authority switch preparation includes bounded binary transfer.
- Orphan and shared-content collection need explicit reference accounting.
- Conversation records remain portable because media references are path-free.
- Content identity is the Media UUID/reference; byte equality alone does not
  imply deduplication.

## Rejected alternatives

- Upload to the HTTP-connected Device and wait for Sync.
- Store Project Chat attachments in Project Resource Root.
- Give each Chat a separate Workspace-level media tree.
- Encode Device identity or absolute paths in `media_ref`.
- Embed bytes in Conversation Input Delivery.
- Treat manifest equality as proof that all media bytes exist.
- Fall back to controller-local media on authority failure.

## Implementation and verification

Current implementation anchors include:

- `crates/rua-server/src/endpoints/chat_media.rs`;
- `crates/rua-server/src/services/chat_media_transfer.rs`;
- `crates/rua-server/src/services/device_command_bus.rs`;
- `crates/rua-server/src/services/sync_transfer.rs`;
- `libs/rua-core/src/chat_store.rs`;
- `crates/rua-server/src/services/chat_storage.rs`.

### Current implementation gaps

The prepare step is epoch-fenced, but final transfer commit is not.
`crates/rua-server/src/services/device_command_bus.rs` loads placement and
compares `ChatMediaPrepareCommand.authority_epoch` before calling
`ChatMediaTransferRegistry::authorize_upload`. However,
`AuthorizedMediaUpload` in
`crates/rua-server/src/services/chat_media_transfer.rs` stores no Chat Address
or Authority Epoch, and
`complete_upload` commits metadata using only the operation ID and transfer
attributes. It cannot revalidate placement if authority changes after prepare
and before commit.

Current tests cover stale epoch rejection during prepare and rejection of
dependent input carrying an old upload epoch, including
`crates/rua-server/tests/infra/sync_transfer.rs` and
`crates/rua-server/tests/infra/conversation_authority_target.rs`. They do not
cover the prepare-success, authority-switch, commit race. D5 remains the
accepted requirement: final commit must execute under an authority gate or
otherwise compare the stored Chat Address and accepted epoch before making the
object visible.

Verification must cover both scopes, local and remote upload/read, stale epoch,
first-upload materialization, authority change between prepare and commit,
interrupted transfer cleanup, required-media preparation, shared references,
and no resource-root writes. Relevant suites
include `crates/rua-server/tests/contract_console_http/chat_media.rs`,
`crates/rua-server/tests/infra/conversation_authority_target.rs`,
`crates/rua-server/tests/infra/sync_transfer.rs`, and
`crates/rua-server/tests/endpoints/media_sync_regression.rs`.

## Related decisions

- ADR 0018: Chat Address and Lazy Chat.
- ADR 0019: Workspace and Project Resource Binding.
- ADR 0022: Authority placement and epoch fencing.
- ADR 0023: Input acceptance and Turn ownership.
- ADR 0027: History references and durable replacement.
- ADR 0037: Device Network Runtime and shared Device Sessions.
- ADR 0039: Domain sync adapters and publication seam.
- ADR 0046: Chat deletion tombstones and storage accounting.
