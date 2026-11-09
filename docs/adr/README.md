# Architecture decision records

This directory is the public index of the 57 architecture records. The
records are grouped by domain for discovery; their numbers are stable
identifiers, not a required reading order.

The set contains 49 accepted decisions, seven proposed
decisions (ADR 0007, 0021, 0029, 0036, 0041, 0046, and 0051),
and one withdrawn record (ADR 0055).

Accepted records are normative decisions even when their implementation section
lists known conformance gaps. Proposed records describe target designs that are
not yet normative. Implementation completion is tracked separately from
decision status.

This directory keeps current decisions and withdrawal markers. Removed designs
and old number meanings are not part of the current contract. New ADRs need unique
stable identifiers, and existing identifiers are not reassigned.

## Runtime, configuration, and process ownership

| ADR | Status | Summary |
|---|---|---|
| [0001 - Workspace 分层依赖方向与进程组合](0001-workspace-layering-and-process-composition.md) | Accepted | Keeps dependencies flowing from product crates to foundation libraries and separates process-level composition from per-Agent runtime composition. |
| [0002 - Runtime Layout 与 Agent Path Contract](0002-runtime-layout-and-agent-path-contract.md) | Accepted | Defines the APK-hosted native Android resident, independently installed Linux environment, shared runtime paths, and stable Agent path projections. |
| [0003 - Runtime Authority 与外部进程执行环境](0003-runtime-authority-and-external-process-environment.md) | Accepted | Centralizes subprocess launch planning; Android tools and terminal shells use the Linux Compatibility Runtime with explicit readiness diagnostics. |
| [0004 - 配置持久化 Cell Store](0004-config-persistence-cell-store.md) | Accepted | Defines typed Cells over a shared disk-authoritative store with atomic, disk-first updates and mtime-aware caching. |
| [0005 - Agent Profile、Running Config 与 ResolvedAgent](0005-agent-profile-running-config-and-resolved-agent.md) | Accepted | Separates Agent identity/content from runtime behavior and requires registry lookups to return both as one coherent resolved value. |
| [0006 - Model Knowledge Catalog 与 Reasoning Control](0006-model-knowledge-catalog-and-reasoning-control.md) | Accepted | Separates model capability knowledge, persisted model configuration, and Chat-level reasoning preferences without runtime name guessing. |
| [0007 - 进程生命周期和有界分阶段关闭](0007-process-lifecycle-and-bounded-phased-shutdown.md) | Proposed | Proposes one cancellation-aware, ordered and bounded shutdown controller; current server cleanup is not yet fully wired through it. |

## Agent harness, capabilities, security, and extensions

| ADR | Status | Summary |
|---|---|---|
| [0008 - Agent Work Contract 与 Collaboration Mode](0008-agent-work-contract-and-collaboration-mode.md) | Accepted | Keeps runtime-owned work guidance, typed Default/Plan/Execute control, and structured plans while preventing Mode from becoming a capability or security axis. |
| [0009 - Bounded execution guardrails without a shared budget entity](0009-shared-agent-execution-budget.md) | Accepted | Rejects a durable global budget ledger and assigns iteration, timeout, cancellation, Task, and Hook bounds to their actual owners. |
| [0010 - Tool 输出可见限制与 Raw Capture](0010-tool-output-visible-limits-and-raw-capture.md) | Accepted | Separates raw capture limits, execution-time model-visible token limits, bounded recall, and later history projection truncation. |
| [0011 - Effective Capability Snapshot 与 Tool Exposure](0011-effective-capability-snapshot-and-tool-exposure.md) | Accepted | Resolves one immutable per-call capability snapshot that drives both callable dispatch and Direct, Deferred, or Hidden model exposure. |
| [0012 - Chat 快照授权、Tool Guard 与进程级 Approval](0012-per-agent-tool-guard-and-process-wide-approval.md) | Accepted | Materializes Agent security defaults into Chat authorization snapshots and coordinates process-wide approvals without delegation privilege escalation. |
| [0013 - MCP Per-Agent Manager 隔离](0013-mcp-per-agent-manager-isolation.md) | Accepted | Gives each Agent runtime an isolated MCP manager, connection state, discovery cache, reload boundary, and shutdown lifecycle. |
| [0014 - Plugin process and protocol isolation](0014-plugin-process-and-protocol-isolation.md) | Accepted | Defines process/protocol isolation without claiming an OS sandbox, and separates Plugin RPC from the trusted internal Agent Hook SPI. |
| [0015 - Skill Bundle 安装来源与扫描信任](0015-skill-bundle-install-sources-and-scan-trust.md) | Accepted | Treats a Skill as a directory bundle with explicit provenance, staged installation, bounded discovery, and content scanning before activation. |
| [0016 - Channel Trait、实现隔离与 Feature Gates](0016-channel-traits-implementation-isolation-and-feature-gates.md) | Accepted | Keeps Channel contracts and platform implementations isolated while distinguishing compile availability, configuration enablement, and runtime health. |
| [0017 - Slash Command 多层调度与 Discovery](0017-slash-command-layered-dispatch-and-discovery.md) | Accepted | Defines deterministic dispatch across built-in, conversation, and Agent commands with discovery generated from the executable registries. |

## Chat, conversation state, projection, hooks, and media

| ADR | Status | Summary |
|---|---|---|
| [0018 - Chat Address, scope, and lazy materialization](0018-chat-address-scope-and-lazy-materialization.md) | Accepted | Makes the scoped Chat Address the complete immutable identity and permits valid Chats to remain unmaterialized until a stateful operation succeeds. |
| [0019 - Project, Project-Scoped Chat, resource bindings, and sidebar organization](0019-project-scoped-chat-and-fixed-resource-binding.md) | Accepted | Defines multiple resource directories on one fixed Device, stable Chat working directories, expanded Project actions, and synchronized manual sidebar organization. |
| [0020 - Session File and append-only Message Log](0020-session-file-and-append-only-message-log.md) | Accepted | Splits the small authoritative Chat header from append-only chronology and from durable or disposable derived state. |
| [0021 - Chat Index as a complete non-authoritative projection](0021-chat-index-complete-non-authoritative-projection.md) | Proposed | Proposes a complete repairable list projection; preview projection, deterministic rebuild, and recoverable pin ordering are not yet complete. |
| [0022 - Conversation Authority, placement, and epoch fencing](0022-conversation-authority-placement-and-epoch-fencing.md) | Accepted | Assigns each Chat one durable execution authority and fences every mutation with an authority epoch. |
| [0023 - Active Turn, Turn Writer, and Conversation Input Queue](0023-active-turn-turn-writer-and-input-queue.md) | Accepted | Enforces one foreground Active Turn and one writer per Chat while serializing accepted input through an authority-owned queue. |
| [0024 - Stop, interjection, and Viewer Disconnect](0024-stop-interjection-and-viewer-disconnect.md) | Accepted | Distinguishes authority-routed Stop and interjection semantics from passive viewer disconnection and provider failure. |
| [0025 - Runtime Notes as typed durable Conversation State](0025-runtime-notes-as-typed-durable-conversation-state.md) | Accepted | Persists bounded typed runtime facts separately from authored messages and injects them only at legal model-call boundaries. |
| [0026 - Context Projection owns model-call assembly](0026-context-projection-owns-model-call-assembly.md) | Accepted | Makes Context Projection the sole assembler and measurement boundary for normalized model requests. |
| [0027 - Durable History Replacement](0027-durable-history-replacement.md) | Accepted | Defines optional background generation of validated durable digests that replace eligible trace blocks only in projection. |
| [0028 - Complete Chat configuration snapshots and inherited authorization](0028-chat-running-config-and-turn-stable-snapshot.md) | Accepted | Defines complete configuration epochs, source-to-collaboration inheritance, and coherent admission while preserving active Turn snapshots. |
| [0029 - Two-layer Product Hooks](0029-two-layer-product-hooks.md) | Proposed | Proposes bounded Active Turn and canonical Operation Hook layers without delegating authority or security decisions. |
| [0030 - Chat Media ownership and authority-routed transfer](0030-chat-media-ownership-and-authority-routed-transfer.md) | Accepted | Stores media under the scope workspace and routes foreground transfer to the Conversation Authority using location-independent references. |

## Tasks, ACP, terminal, and CLI

| ADR | Status | Summary |
|---|---|---|
| [0031 - Durable Task Runtime and Attempt state machine](0031-durable-task-runtime-and-attempt-state-machine.md) | Accepted | Defines one process-level durable Task aggregate with immutable specs, explicit Attempts, retries, reconciliation, and Chat-scoped ownership. |
| [0032 - Tool Task Runner, bounded Read, and authority routing](0032-task-runner-watch-control-and-authority-routing.md) | Accepted | Defines six Task Tools, bounded Read, UI-only activity subscriptions, and execution-device control routing. |
| [0033 - ACP Chat Mode, client projection, and permission relay](0033-acp-chat-mode-client-projection-and-permission-relay.md) | Accepted | Treats ACP Chat Mode as a Conversation Driver with one client projection and exact runner permission relay distinct from Rua approval. |
| [0053 - Collaboration Chats and background Tool Tasks](0053-collaboration-chats-and-background-tool-tasks.md) | Accepted | Defines collaboration Chat, Tool-only Tasks, source Turn release, result-driven continuation, and visible background waiting (D35). |
| [0034 - Interactive Terminal Session and Viewer Runtime](0034-interactive-terminal-session-and-viewer-runtime.md) | Accepted | Defines Device-owned terminal sessions, native Android PTY with a Linux guest shell, detachable viewers, canonical snapshots, and lease-controlled input. |
| [0035 - Chat CLI single-request adapter and resident server boundary](0035-chat-cli-single-request-adapter-and-resident-server-boundary.md) | Accepted | Keeps CLI Chat as one resident-server request that may wait in the canonical queue and streams only its own eventual Turn. |

## Device identity, networking, and synchronization

| ADR | Status | Summary |
|---|---|---|
| [0036 - Master identity, Device List, and pairing trust](0036-master-identity-device-list-and-pairing-trust.md) | Proposed | Proposes transactional authenticated pairing around NetworkSecret and Device identity; durable acknowledgement and rollback are incomplete. |
| [0037 - Device Network Runtime and shared Device Sessions](0037-device-network-runtime-and-shared-device-sessions.md) | Accepted | Centralizes network ownership and manager-owned shared session establishment; the 2026-11-05 cancellation/recovery amendment awaits implementation. |
| [0038 - Sync operations, HLC ordering, and envelope encryption](0038-sync-operations-hlc-and-envelope-encryption.md) | Accepted | Defines immutable encrypted SyncChange envelopes, opaque replica keys, and HLC ordering while leaving canonical state in domain stores. |
| [0039 - Domain Sync Adapters and the publication seam](0039-domain-sync-adapters-and-publication-seam.md) | Accepted | Routes committed local facts and inbound changes through registered domain-owned adapters without making Sync a second domain store. |
| [0040 - SQLite Inbound Inbox and Sync Sidecar Store](0040-sqlite-inbound-inbox-and-sync-sidecar-store.md) | Accepted | Uses one rebuildable SQLite store for durable inbound work, retry state, idempotency, record cursors, and repair metadata. |
| [0041 - Bidirectional First Sync](0041-bidirectional-first-sync.md) | Proposed | Proposes fail-closed bidirectional pre-pair export; common-baseline merge and capture failure propagation are incomplete. |
| [0042 - Incremental apply and Repair Scan](0042-incremental-apply-and-repair-scan.md) | Accepted | Makes remote inserts drive normal incremental apply and reserves bounded Repair Scan for anti-entropy and recovery. |
| [0043 - Sync presence, mobile power, and background scheduling](0043-sync-presence-mobile-power-and-background-scheduling.md) | Accepted | Separates network presence from Sync cycles and lets host profile, Android lifecycle, leases, and policy govern residency. |
| [0044 - Live-assisted hydration and cross-device conversation continuation](0044-live-assisted-hydration-and-cross-device-conversation-continuation.md) | Accepted | Combines authority-owned durable history with runtime Live Activity, bounded gap recovery, and explicit cross-device continuation. |

## Durable user data and product shells

| ADR | Status | Summary |
|---|---|---|
| [0045 - Structured Memory Aggregate](0045-structured-memory-aggregate.md) | Accepted | Makes structured records, evidence, relations, visibility, and pinned slots the only durable long-term Memory model. |
| [0046 - Chat deletion, tombstones, and storage accounting](0046-chat-deletion-tombstones-and-storage-accounting.md) | Proposed | Proposes permanent replicated delete intent, idempotent cleanup and reference-safe accounting; coordinator coverage and recovery are incomplete. |
| [0047 - Best-effort Device notifications](0047-best-effort-device-notifications.md) | Accepted | Provides an independent Device-addressed notification capability with no durable queue, retry guarantee, or duplicate presentation path. |
| [0048 - Markdown Plus editable visual documents](0048-markdown-plus-editable-visual-documents.md) | Accepted | Extends Markdown with bounded editable visual blocks while keeping source text authoritative and rendering non-executable. |
| [0049 - Tauri Desktop Shell and local resident server](0049-tauri-desktop-shell-and-local-resident-server.md) | Accepted | Keeps Tauri as a native lifecycle and packaging shell around the external unified Rua resident server and HTTP boundary. |
| [0054 - Bundled Browser Use and Computer Use](0054-bundled-browser-and-computer-use.md) | Accepted | Bundles fixed-version agent-browser and Cua Driver programs, reuses system Chromium browsers, and keeps MCP connections and native desktop ownership explicit. |
| [0052 - Owned Console and Conversation View Runtime](0052-owned-console-and-conversation-view-runtime.md) | Accepted | Keeps Console state subordinate to Rust and centralizes scoped projection and shell UI; the 2026-11-05 shared transport/local-first amendment awaits implementation. |
| [0055 - Bounded Conversation Rendering and History Residency](0055-bounded-conversation-rendering-and-history-residency.md) | Withdrawn | Withdrawn after scrolling regressions; ADR-0056 defines the accepted replacement. |
| [0056 - Whole-Turn Conversation DOM Residency](0056-whole-turn-conversation-dom-residency.md) | Accepted | Bounds mounted whole Turns with generous retention, independent view state, stable reading anchors and identity-based history location. |
| [0057 - Phone Use native capabilities and private execution](0057-phone-use-native-capabilities-and-private-execution.md) | Accepted | Embeds Android accessibility and visible single-photo capture in the APK, with private authenticated Runtime access, bounded observed interactions, and a transient per-operation Terminate control. |

## Testing and release

| ADR | Status | Summary |
|---|---|---|
| [0050 - Layered test architecture](0050-layered-test-architecture.md) | Accepted | Classifies tests by failure signal, keeps Rust suites crate-local and theme-aggregated, and expands verification with change risk. |
| [0051 - Release verification gates](0051-release-verification-gates.md) | Proposed | Proposes fail-closed aggregate release evidence; config roundtrip and feature-flag helpers can currently under-report failure. |

## Maintenance rules

- State current decisions directly without compatibility text for retired
  numbering or wording.
- Keep identifiers unique and stable. Do not reassign an existing identifier.
- Fix stale repository references directly when an ADR is renamed or replaced.
- Put stable constraints in ADRs and implementation detail in the owning guide
  or module documentation.
- Check `CONTEXT.md` before introducing domain terminology.
- Update this index and all authoritative references when an ADR is replaced,
  renamed, or removed.
