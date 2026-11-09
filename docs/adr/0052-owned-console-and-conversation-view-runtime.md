# ADR-0052: Owned Console and Conversation View Runtime

> **Status:** Accepted
> **Amended:** 2026-11-05 — shared Console transport and local-first reads.

## Context

The Console is a client of the Rust product contract, but Chat is stateful
enough that browser-local state can accidentally become a second authority.
Route selection, durable history, Active Turn state, Live Activity, queued
input, approvals, Tasks, model context controls, and reconnect recovery all
change independently.

The current Console uses Rua-owned Chat modules and UI primitives. It does
not depend on a vendor Chat runtime or antd component state to decide
Conversation identity, generating state, terminal state, or message
projection.

Shared navigation, page-header actions, and skin tokens also need one owning
module. Duplicating them per page causes desktop/mobile drift and makes visual
state appear to define product behavior.

Native APK cold-start observation also exposed transport coupling: four
long-lived browser subscriptions and two remote approval queries occupied the
HTTP/1.1 connection pool. Local sidebar reads spent tens to hundreds of seconds
waiting in the browser even though server processing took only milliseconds.
Reading local stores is insufficient if unrelated remote work can consume all
of the client transport's capacity.

## Decision

### D1 - The Rust HTTP contract is authoritative

The Console consumes the canonical Agent-Scoped and Project-Scoped APIs.
Durable Conversation State, Conversation Authority, Approval, Task lifecycle,
and synchronization remain server-owned.

DTO shapes and error semantics are defined at the Rust HTTP boundary. Errors use
the standard `{"detail":"..."}` body. Browser state may cache and optimistically
present data, but it must reconcile with server and synchronized state.

### D2 - Complete Chat Address owns route identity

The active Conversation is selected by one complete scoped route:

- `/agents/{agentId}/chats/{chatId}`;
- `/projects/{projectId}/chats/{chatId}`.

Changing Scope, owner, or Chat remounts or resets the scoped controller. A
global Agent selector, a bare Chat ID, or stale browser state cannot complete or
relabel the address of an already mounted Conversation.

The active Conversation surface and runtime accept one `ConversationAddress`
with named runtime options. The runtime derives the API target, Media scope,
browser cache owner, and view key once from that address; callers do not pass
parallel Agent/Project identity hints. Existing browser draft/cache namespaces
are retained. Project execution Agent selection is a separate submit option.
It cannot reset Live attachment or invalidate a pending control response for
the same address. Async results are checked against the complete view identity;
a real Scope, owner, or Chat change invalidates results from the previous view.

### D3 - Conversation View Runtime owns active Chat projection

The Rua-owned Conversation View Runtime loads stable history, consumes Live
Activity and Control View state, reconciles reconnects, and produces one
render-ready Conversation UI Projection.

Content readiness, Active Turn state, Live attachment, pending delivery, and
control availability remain separate facts inside the runtime. Render
components consume the projection and dispatch explicit commands; they do not
infer backend state from DOM state, spinner presence, or stream connection
presence.

Viewer navigation, unmount, or transport loss is passive. It does not imply
Stop, cancellation, or deletion.

Within that runtime, the scoped Conversation Convergence Module owns activity
acceptance and history acceptance before presentation state is released. Live
and replay use the same identity/cursor checks, event folding, and history,
hydration, and approval refresh decisions. Replay stages a complete batch before
publishing its projection and resume cursor; a disconnected partial batch must
remain recoverable on the next attachment. Control and history mutations retain
their activity order within the batch.

History reads capture the view identity and mutation version. Responses from
before rollback, another view, or older durable coverage cannot release Live
content. Matching durable Messages replace that content in place through the
existing rendering and coverage rules. The Module performs no network I/O;
the hook executes reconciliation requests through the existing worker and feeds
results back through history acceptance. History queries and hydration never
block later Live ingestion. This is an internal ownership refinement, not a
change to Rust authority, wire contracts, or display retention policy.

### D4 - Protocol data is normalized before rendering

Persisted history and live frames are converted into Rua Conversation events
and then folded through the same projection rules. Render components do not
interpret provider wire envelopes or decide whether a Turn is terminal.

Typed content parts remain typed. Text renders as text, Tool Use and Tool Result
retain their identity, and unsupported content uses an explicit fallback rather
than displaying a serialized protocol envelope.

A model-context editing surface changes the durable Projection Exclusion Set.
It does not edit or delete Message Log records and must be labelled and
presented as model-context selection rather than transcript mutation.

### D5 - Product UI primitives are Rua-owned

Reusable product controls live under `console/src/ui/`; feature composition
lives under `console/src/pages/` and `console/src/components/`. Product state
and interaction semantics must not depend on antd, Ant Design X, or a vendor
Chat runtime.

Low-level React, accessibility, parsing, rendering, and positioning libraries
may be used when they do not introduce a second product-state model.

### D6 - Shared shell structures have one source

Navigation definitions come from `console/src/layouts/navGroups.tsx`.
Desktop and mobile navigation consume the same groups.

Standard routed-page action layout is owned by `PageHeader`, including the More
menu and Refresh placement. Pages contribute commands and state, not a parallel
header layout convention.

Color, spacing, typography, radius, and skin behavior are expressed through the
shared token and skin files under `console/src/styles/`. Feature pages consume
those tokens instead of defining independent product-wide themes.

Compatibility HTTP routes exist only when recorded in
`crates/rua-server/src/route_compat.rs` with a retention reason and exit
condition.

Console Chat has no root-scoped compatibility routes. Submit, live, and stop
use `/api/agents/{agentId}/chats/{chatId}/console/{submit|live|stop}`;
push-message polling and recent-message reads use
`/api/agents/{agentId}/console/{push-messages|recent_messages}`. Callers select
the Agent explicitly in the path rather than relying on `X-Agent-Id` or a
default Agent to resolve a root Console route.

### D7 - Conversation display policy after the bounded-view rollback

[ADR-0055](0055-bounded-conversation-rendering-and-history-residency.md) was
withdrawn after scrolling regressions. [ADR-0056](0056-whole-turn-conversation-dom-residency.md)
replaces that display policy with generous whole-Turn DOM retention and a single
scroll/anchor controller. Loaded data remains separate from DOM residency;
active-history data eviction and single-body paging are not reintroduced.
Display policy remains independent of Message Log, model-visible Context
Projection, and Turn lifecycle.

### D8 - One Console transport owns persistent connections

One Console Transport module per document and authenticated resident context
owns a single versioned WebSocket connection. Chat and Terminal Live Activity,
conversation wake/status, Device connectivity, and other Console subscriptions
are independently addressed logical channels on that connection. Components
acquire/release channels; they do not create their own EventSource, streaming
fetch, socket, or reconnect loop. Separate tabs/windows remain independent
clients; this is not a new Device identity or a cross-tab broker.

Short local reads and ordinary bounded HTTP operations retain canonical APIs.
Requests that wait for a remote Device, including remote approval/permission
snapshots, use correlated logical requests through Console Transport rather
than holding ordinary HTTP connection slots. The multiplexed adapter invokes
the same Rust application services, scoped DTOs, authorization, and error
semantics as the HTTP adapters; it introduces no second business backend.
Large uploads/downloads use separately bounded transfer admission and chunks,
so neither bulk traffic nor remote Media hydration can starve local reads or
control messages. This decision does not put every HTTP request on WebSocket.

The connection preserves the canonical APIs' TLS/secure-origin policy. The
handshake validates the allowed origin and applies the existing Console
authentication policy before admitting any logical channel. Browser clients use a
bounded initial authentication exchange without credentials in URLs or logs;
every operation retains its existing scope and authority checks. Logout,
credential revocation, backend/account changes, and document destruction close
the connection and invalidate its channels. Unsupported protocol versions fail
explicitly rather than silently recreating feature-local streaming transports.

The transport owns request/subscription IDs, connection generations, bounded
per-channel/global buffers, fair scheduling, and one reconnect policy. Channel
callbacks never await remote I/O while dispatching unrelated frames. A slow
subscriber cannot block all channels: it receives explicit overflow/gap state
and reconciles through its domain's cursor/snapshot contract. Reconnection
resubscribes active interests; it does not replay mutations automatically.
Late replies from cancelled requests or obsolete generations cannot update a
new view. Cancelling a channel releases its resources without stopping backend
Turn/Task execution unless an explicit domain Stop/cancel command was sent.

### D9 - Local content readiness is independent of remote freshness

Chat/Project lists, sidebar organization, and locally available durable history
load from resident-owned local stores before remote refresh completes. They do
not wait for Device Session admission, approvals, Sync/hydration, Live attach,
or current remote authority availability. An empty local projection is shown
as such with independent refresh state; it is not held behind an indefinite
spinner. Remote updates reconcile later through the existing domain rules.

Cached remote control/approval information remains distinguishable from a
fresh authoritative snapshot. An unreachable authority means unknown/stale
control state, not an empty approval queue, a granted permission, or authority
to execute locally. Actions requiring remote validation fail or wait within
their own declared bounds without disabling local navigation and reading.

Transport admission preserves capacity for local reads even with hung remote
queries, active streams, and pending Media. Repeated refresh triggers coalesce
per scope while a read is in flight instead of accumulating polling requests;
unmount/cancellation releases pending work and rejects stale results. Loading
remote data cannot clear already rendered local content. Browser visibility
controls view subscriptions, while Android host residency remains governed by
ADR-0043.

## Hard invariants

- Browser state is never durable Conversation or control authority.
- One Console Transport owns persistent connections for each document/resident context.
- Remote waits and subscriptions cannot exhaust local-read transport capacity.
- Local content readiness is independent of remote freshness and control readiness.
- Reconnect never implicitly replays a mutation or converts stale approval state into permission.
- Active Chat identity is always a complete Chat Address.
- Conversation terminal state comes from canonical history and current
  Control/Live state, not connection presence or renderer-local state.
- Persisted and live content pass through the same Conversation UI Projection.
- Context editing never rewrites or deletes Message Log records.
- Route changes and Viewer Disconnect do not Stop backend work.
- Product surfaces do not introduce a vendor-owned Chat runtime.
- Navigation groups, standard page-header layout, and skin tokens each have one
  shared source.
- Compatibility routes are explicit and temporary.

## Authority, persistence, and failure boundaries

The server owns durable product state and command acceptance. Conversation View
Runtime owns bounded browser projection and reconnect state. Render components
own presentation only. Browser navigation hints and drafts are disposable
device-local state.

A failed Live connection preserves already loaded durable content and triggers
bounded reconciliation. An EOF without a terminal event is an explicit
interrupted or unknown runtime outcome; it cannot leave an indefinite
generating state.

## Consequences

- Browser, desktop, and mobile shells present the same Rust product model.
- Chat behavior is testable through one owned runtime instead of vendor
  contexts and DOM side channels.
- UI caches improve responsiveness without becoming a second source of truth.
- Shared navigation, page-header, and skin behavior cannot silently drift
  between pages.

## Rejected alternatives

- Let a vendor Chat runtime own session identity or terminal state.
- One long-lived HTTP subscription and reconnect loop per UI feature.
- Block local lists on a combined local/remote `Promise.all` or an exhausted HTTP pool.
- Rely on larger timeouts, hostname sharding, or browser connection-limit tuning
  as the transport ownership model.
- Repair missing backend semantics with permanent browser-local state.
- Let each page define navigation, header actions, or product-wide skin rules.
- Treat route changes or stream disconnect as Stop.
- Expose Tauri commands as a second application backend.
- Present Projection Exclusions as edits to the raw transcript.

## Implementation and verification

Current implementation anchors include:

- `console/src/pages/Chat/owned/`;
- `console/src/pages/Chat/owned/useConversationViewRuntime.ts`;
- `console/src/pages/Chat/owned/conversationConvergence.ts`;
- `console/src/pages/Chat/owned/__tests__/conversationConvergence.test.ts`;
- `console/src/pages/Chat/owned/conversationProjectionReducer.ts`;
- `console/src/pages/Chat/owned/renderState.ts`;
- `console/src/api/modules/conversation.ts`;
- `console/src/layouts/navGroups.tsx`;
- `console/src/components/PageHeader/`;
- `console/src/styles/theme.css`;
- `console/src/styles/skin-configs.ts`;
- `console/src/ui/`;
- `crates/rua-server/src/route_compat.rs`.

Verification must cover scoped route switching, stable-history plus Live
reconciliation, EOF without terminal state, Viewer Disconnect, typed Tool
Result rendering, model-context exclusion presentation, shared navigation,
PageHeader action ordering, and skin-token consumption.

### Conformance gaps and required acceptance

The 2026-11-05 amendment is accepted architecture, not a claim that the shared
WebSocket transport exists today. Feature-local subscriptions and synchronous
remote snapshot HTTP requests remain migration work. Existing canonical HTTP
clients retain their contract; any compatibility streaming routes kept during
migration need the documented retention reason and exit condition from D6.

Acceptance must exercise actual browser/WebView connection behavior, in
addition to mocked hook tests:

- Keep the remote Device and its approval/permission/Media responses blocked.
  A cold page with no browser cache still renders seeded local Chat/Project
  lists, sidebar, and history within the fixture's local-read deadline without
  releasing the remote gate. Repeat after APK reopen and while navigating.
- Simultaneous Chat, Terminal, activity, and Device subscriptions share one
  socket; one slow channel or transfer cannot delay another channel or local GET.
- Lost sockets reconnect once and reconcile subscriptions by cursor/snapshot;
  a gap/overflow is explicit and mutations are not silently resent.
- Route switches, cancelled waiters, logout, unpairing, and resident restarts
  fence obsolete frames and release demand; offline approvals remain unknown.
- Repeated focus/poll triggers leave bounded in-flight refresh work; fresh
  remote updates merge without clearing local content or changing its authority.

## Related decisions

- ADR-0018 defines Chat Address and scoped routes.
- ADR-0020 defines append-only Message Log storage.
- ADR-0024 distinguishes Viewer Disconnect from Stop.
- ADR-0026 owns model-visible Context Projection.
- ADR-0035 defines the separate single-request CLI adapter.
- ADR-0044 defines durable and live Conversation reconciliation.
- ADR-0049 defines the Tauri Desktop Shell boundary.
- ADR-0037 owns the separate Device-to-Device session lifecycle.
- ADR-0043 owns mobile residency independently of Console transport liveness.
