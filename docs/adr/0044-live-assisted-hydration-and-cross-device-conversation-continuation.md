# ADR-0044: Live-assisted hydration and cross-device conversation continuation

> **Status:** Accepted

## Context

A Conversation may be viewed from a Device that does not currently execute its
Active Turn. Stable Evergreen history converges through Sync, while Live
Activity provides low-latency runtime output. A viewer can therefore observe
content before ordinary Sync materializes the same complete Messages locally.

Route changes, replay-window gaps, and authority changes must not let viewers
write reconstructed output into canonical history. Cross-device continuation
also cannot assume identical files, tools, operating systems, or runtime paths.
The model needs one convergence protocol that preserves the Conversation
single-writer rule and makes authority movement explicit.

## Decision

### D1 - Conversation Authority remains the only writer

Only the current Conversation Authority creates Turn lifecycle, canonical
Message identities, Message Log records, checkpoints, and terminal facts.
Console and remote viewers render Live Activity but never persist token deltas,
replay frames, or folded UI projections into Evergreen.

Every representation carries the same Chat Address, Authority Epoch, `turn_id`,
and canonical `message.id` where applicable.

### D2 - Durable history and Live Activity are independent inputs

Conversation View Runtime renders the locally materialized durable prefix
immediately, then attaches Control and Live Activity. Stable history is never
cleared or delayed because Live, authority routing, or hydration fails.

Current generating state comes from verified Control View and Live Activity,
not from an `in_progress` history record.

### D3 - Live-assisted hydration accelerates normal Sync

After the authority commits and publishes a complete Durable Conversation Fact,
Live Activity may advertise the authority's Durable Publication Watermark to an
active remote viewer. The current implementation then requests a targeted Sync
Cycle to that authority Device, runs Repair Scan and Inbound Inbox drain, and
verifies the local Conversation Manifest. The published SyncChange still
travels through the ordinary Docs/Blobs Sync path, not as a Device Session
logical stream.

Foreground acceleration and later periodic Sync therefore reuse the same
replica entry, `op_id`, record identity, HLC/cursor, and Durable Publication
Watermark. Both enter the normal Inbound Inbox and SyncRegistry apply seam; a
duplicate arrival is idempotently already applied.

### D4 - Unpublished output stays runtime-only

Output newer than the latest complete Message boundary may be recovered through
an authority-authored Active Turn Recovery Checkpoint. The current runtime
builds this as a bounded sequence of replay frames marked
`activity_checkpoint`, carrying the original authority epoch, Turn/Message
identity, and Conversation Activity positions.

It may include an incomplete assistant Message, but it is not History, a
SyncChange, a replacement record, or permission for the viewer to publish.
Later durable history covers the same Message identity and replaces only the
runtime suffix.

### D5 - Conversation View Runtime has bounded retention

The Device-local view runtime outlives a single React route through a retained
Console cache. The current cache is bounded to eight Conversations and an
estimated four MiB process-wide budget; entries with non-terminal live state are
not evicted merely to satisfy the limit, and capacity pressure is reported.
The architectural identity remains the full Chat Address.

The current cache key is `${agent_id}:${chat_id}` rather than a serialization of
the complete Chat Address. It therefore does not yet encode Agent versus Project
scope and cannot be treated as proof of collision-free cross-scope retention.

Eviction must not silently drop an uncovered prefix. The runtime first obtains
a newer durable prefix or an authority recovery checkpoint; otherwise it keeps
the last proven projection and reports recovery unavailable.

### D6 - Gap recovery is staged and non-destructive

On attach, generation reset, or replay gap, the viewer preserves visible
content, reads local History, continues Live ingestion, hydrates any advertised
published watermark not yet applied, and consumes the authority checkpoint
frames returned with a gap when they are available.

It validates Chat Address, Authority Device, Authority Epoch, message
identities, manifest identity, watermark, and coverage position before an
atomic projection advance.

### D7 - Continuation requires explicit authority placement

Opening a Chat on another Device does not move Conversation Authority. A
user-directed continuation sends one adoption command to the selected target.
The target compares durable manifests, obtains any stable-history gap and
required Media through the shared Device Session Bulk Transfer path, prepares
its local runtime, and only then acknowledges its new Authority Epoch. This
direct preparation transfer is distinct from viewer hydration through targeted
Sync and does not create new SyncChange identities.

The source Device does not execute the next Turn after adoption. Failure leaves
the prior placement intact or reports the target unavailable; there is no
last-viewer-wins election or silent local fallback.

### D8 - Runtime differences are disclosed on demand

Stable Conversation State is portable, but runtime capabilities and local
paths are not. Cross-device continuation may surface coarse Device activity and
uses the explicit runtime environment/tool contract when exact local details
are needed. Dynamic Device details are not injected into every model request.

### D9 - Agent and Project Chats share convergence, not placement rules

Agent-scoped and Project-scoped Chats use the same identity, durable watermark,
hydration, recovery, and rendering rules. Project Resource Binding fixes Project
execution to its Resource Device, so the user-directed cross-Device authority
adoption described above applies only where authority placement is movable. It
does not override the Project placement invariant.

## Hard invariants

- Only Conversation Authority persists Conversation facts.
- Console never persists SSE or Device Session output as Messages.
- Durable prefixes end only at complete Message boundaries.
- Foreground hydration uses targeted normal Sync and reuses the original
  published SyncChange identity.
- Active Turn Recovery Checkpoints remain runtime-only.
- Stable history, recovery checkpoint, and Live suffix merge by canonical
  identity without duplicate Messages.
- Opening or viewing a Chat never transfers authority.
- Authority adoption is explicit, epoch-checked, target-prepared, and cannot
  move a Project Chat away from its fixed Resource Device.
- Route unmount is not Stop and does not immediately destroy active view state.
- Runtime-environment differences are not silently assumed away.

## Authority, persistence, and failure boundaries

Conversation Authority owns canonical writes and Active Turn runtime. Sync owns
durable publication and local materialization. Device Session carries targeted
Live, checkpoint, and authority-adoption preparation traffic without becoming
canonical storage; viewer hydration remains a targeted normal Sync operation.
Conversation View Runtime owns only bounded local projection state.

Live failure does not erase durable history. Hydration failure falls back to
ordinary Sync and Repair Scan. A stale epoch, identity mismatch, or incomplete
target preparation rejects authority adoption. A terminal Live event may end
the spinner, but runtime suffix state is released only after matching durable
history covers it.

## Consequences

- Remote viewers see durable content sooner without adding a second writer.
- Navigation can preserve active output within bounded memory.
- Replay gaps recover without clearing already proven content.
- Cross-device continuation is explicit and safe across different runtimes.
- Conversation diagnostics must distinguish Live receipt, hydration, apply,
  checkpoint coverage, authority adoption, and durable visibility.

## Rejected alternatives

- Writing viewer-folded token deltas into Evergreen.
- Publishing one SyncChange per token or SSE frame.
- Giving foreground hydration a new operation id or merge path.
- Keeping every visited Conversation runtime without bounds.
- Clearing the current Turn after a replay gap.
- Moving authority when a viewer opens a Chat.
- Electing authority from presence or last-writer timestamps.
- Injecting full runtime/device details into every model prompt.

## Implementation and verification

- `crates/rua-server/src/services/conversation_live_activity.rs`
- `crates/rua-server/src/services/active_stream_broadcast.rs`
- `crates/rua-server/src/services/chat_activity_wake.rs`
- `crates/rua-server/src/services/conversation_history_readiness.rs`
- `crates/rua-server/src/services/conversation_manifest.rs`
- `crates/rua-server/src/services/durable_publication_watermark.rs`
- `crates/rua-server/src/services/conversation_authority.rs`
- `crates/rua-server/src/services/device_command_bus.rs`
- `console/src/pages/Chat/owned/conversationLiveActivityViewer.ts`
- `console/src/pages/Chat/owned/useConversationViewRuntime.ts`
- `console/src/pages/Chat/owned/__tests__/conversationLiveActivityViewer.test.tsx`
- `console/src/pages/Chat/owned/__tests__/useConversationViewRuntime.test.ts`
- `crates/rua-server/tests/infra/device_session_control_view.rs`
- `crates/rua-server/tests/infra/targeted_chat_materialization.rs`

Verification covers local-history-first rendering, identity-preserving
hydration, duplicate delivery, replay gaps, checkpoint validation, bounded
retention, epoch rejection, target preparation, and local/remote parity. The
current Console cache key is still `agent_id:chat_id`, not a serialized full
Chat Address; the listed tests do not establish complete Agent/Project cache
isolation.

## Related decisions

- ADR-0018 defines Chat Address and Scope identity.
- ADR-0019 fixes Project-Scoped Chat execution to its Resource Device.
- ADR-0020 defines the Session File and append-only Message Log.
- ADR-0022 defines Conversation Authority placement and Epoch fencing.
- ADR-0023 defines Active Turn single-writer execution.
- ADR-0037 provides shared Device Session transport.
- ADR-0039 provides the publication and inbound apply seam.
- ADR-0042 preserves ordinary Sync and Repair Scan fallback.
