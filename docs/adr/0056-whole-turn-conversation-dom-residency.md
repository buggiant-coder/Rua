# ADR-0056: Whole-Turn Conversation DOM Residency

> **Status:** Accepted

## Context

Loaded Chat history previously remained entirely mounted. Cache accounting and
memoization reduced repeated work but did not limit presentation growth. ADR-0055
was withdrawn after ordinary scrolling regressions. The accepted replacement uses
a narrower policy: generous retention with whole-Turn reclamation, separate data
caching, and stable reading and interaction state. Oversized single-Turn rendering
and active-history data eviction are outside this delivery.

## Decision

- Conversation View Runtime remains a client projection, never Conversation
  authority. DOM disposal does not Stop, cancel or alter durable history.
- Use canonical message identities for rendered responses; prepending history
  cannot rename or remount existing responses by their array index.
- Reclaim whole connected Turns. Existing AskUser presentation can join adjacent
  Turns; a connected group stays indivisible. User input, response, tool calls and
  results cannot be split by residency.
- Keep all loaded content while within the generous presentation budget. The
  starting mobile policy is a 192-group high watermark and 128-group target;
  desktop uses 256 / 192. These are adjustable engineering defaults, not measured
  universal frame-rate or memory limits. Visible and protected groups may
  temporarily exceed the budget.
- Retain current and recent reading areas; batch-reclaim distant completed groups.
  Restore nearby groups ahead of reading, one whole group per scheduled task.
  Avoid timer-driven eviction merely because content has not been visited lately.
- Use one scroll container and one controller for anchors, loading, search,
  navigation, resizing and follow-latest. A low-level TanStack virtualizer tracks
  variable heights and visible indexes. Rua owns retention and reading intent;
  the engine must not apply the same scroll compensation a second time.
- Keep source data and stable indexes separate from the mounted group set.
  Consecutive reclaimed ranges use one height spacer, not one permanent DOM
  placeholder per message. Recovered content uses the same message renderer.
- Save lightweight collapse/expansion and AskUser draft choices independently of
  group DOM. Protect active reading, focus, selection, user-answer requests,
  approvals, media playback and related dialogs. Speech cleanup only cancels the
  instance-owned utterance.
- Search consumes the existing Rust message_id / turn_id fields. If data is not
  loaded, Agent and Project history GET accept around_turn_id with a bounded
  turn_limit, mutually exclusive with before_turn_ordinal. Explicitly represent
  the intervening unloaded range and fill it by complete history pages.
- Restore-cache policy is independent: 8 Chat entries, estimated 128 MiB on mobile
  and unknown hosts or 256 MiB on recognized desktops, allocated on demand. DOM
  reclamation does not delete the underlying loaded history. Active-history data
  paging/eviction is a future change.

## Constraints and consequences

Native browser Find and Select All cover mounted DOM. App search and complete
message copying use data; do not claim equivalence with a full transcript DOM.
Unknown heights after width changes remain estimates until restored and measured.
Reading anchors use stable identity and in-viewport offset, not old scrollTop
alone. Global CSS / viewport geometry changes need explicit anchor verification.

A whole group can still be expensive to restore. Early scheduling reduces blank
content but is not itself proof that main-thread pauses disappear. Mobile typing,
continuous reversals, media, large completed outputs and first budget crossing
must be tested in addition to DOM counts.

Temporary selection/focus protection and retained raw data mean this policy is
not a hard browser-process memory cap. No single-Turn fragmentation, automatic
device stress testing, permanent per-frame sampling or global residency framework
is introduced.

## Verification and implementation

Main implementation: ConversationMessageList, useConversationViewport,
conversationTurns, turnResidency and the render-state controls. Server history
locators are shared in rua-chat and used by existing Agent / Project endpoints.

Verification includes component and runtime behavior, scoped HTTP history tests,
Chromium/WebKit end-to-end tests for bounded growth, whole-turn restoration,
reading anchors, search, width changes, selection and the existing composer
regressions. Android results must state the actual device and scope; a high-memory
device or synthetic input does not establish low-end phone or real-IME latency.
