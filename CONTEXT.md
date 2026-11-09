# Rua — Domain Language

The canonical vocabulary for Rua design docs, issues, PRs, and code. Use the
terms below consistently; implementation details and decision rationale belong
in the linked ADRs, rather than being repeated here.

Read only the relevant domain. [The ADR index](docs/adr/README.md) records decision
status and implementation gaps: Accepted defines the contract, Proposed is a
target design, and neither status alone proves implementation or platform support.
Build commands and repository conventions belong in [AGENTS.md](AGENTS.md).

## Runtime and product shells

Details: [runtime paths](docs/adr/0002-runtime-layout-and-agent-path-contract.md),
[execution authority](docs/adr/0003-runtime-authority-and-external-process-environment.md),
[Desktop Shell](docs/adr/0049-tauri-desktop-shell-and-local-resident-server.md),
[desktop automation](docs/adr/0054-bundled-browser-and-computer-use.md),
[Phone Use](docs/adr/0057-phone-use-native-capabilities-and-private-execution.md).

| Term | Meaning and boundary |
|---|---|
| **Native Android Runtime** | The unified Android `rua` ELF launched by the APK from `nativeLibraryDir`. Owns the resident server and orchestration; the APK owns UI, foreground service, lifecycle, and authenticated Phone Use bridge. Starts independently of the Linux guest. |
| **Linux Compatibility Runtime** | The managed Debian rootfs/toolchain entered through bundled PRoot/loader. Android shell, Python, Skills, MCP stdio, ACP, plugins, and terminal shells execute here. PRoot supplies compatibility under the APK UID, not a security sandbox. |
| **Android External Execution Runtime** | The common Android managed-subprocess policy: Runtime Authority plans guest execution; missing guest/toolchains fail the affected operation with diagnostics. No host fallback or command-text routing. |
| **APK Native Payload** | APK-versioned Rua ELF, WebUI, terminal bridge, PRoot/loader, and native dependencies. Excludes Debian rootfs and Python/Node toolchains; native executables run from installer-managed locations resolved again after APK updates. |
| **Mobile Runtime Manifest** | Trusted artifact/version/architecture/source and verification metadata for automatic Debian/Python/Node preparation. Acquisition uses verified staging, independent Debian/Node mirror selection, offline import, cancellation, and atomic activation. Updates preserve usable environments and user data. |
| **Mobile Tool Profile** | Verified Android tool/runtime capabilities, distinguishing native resident availability from guest readiness. Downloading a package alone does not establish support. |
| **Skill Runtime Requirement / Skill Runtime Probe** | A Skill's compatibility needs and bounded install/enable-time checks inside the guest. Portable requirements remain separate from local executable paths and launch plans; failed probes do not select another runtime. |
| **Runtime Layout** | Shared device path, bind, HOME, PATH, cwd, and environment facts used by all subprocess surfaces. |
| **Runtime Authority** | Device-local owner of toolchain discovery, runtime selection, and typed launch plans. Its Runtime Inventory is a refreshable local read model, never Agent config or Sync data. |
| **Agent Path Contract** | Agent-facing stable path rules derived from Runtime Layout on the execution Device. Android phone files use `/sdcard/...`, Rua files use `workspace_dir`, and `/root` is guest HOME. |
| **Desktop External Toolchain** | Device-local Python, Node, Git, and similar executables discovered by Runtime Authority. Missing tools degrade affected features, not core startup. Bundled automation drivers are separate product dependencies. |
| **Toolchain Setup** | User-approved local installation and re-probing guided by the packaged Skill, normal package managers, and Tool Guard. Does not silently replace working tools or create a private package manager. |
| **Desktop Shell** | Tauri native window, tray, integration, updates, and resident-process lifecycle around the unified `rua` entry point and HTTP Console. Backend business behavior remains in the resident Runtime. Distribution/support details belong in release documentation. |
| **Desktop Tools** | App/WebUI settings and readiness surface for automation on the connected execution Device. Viewer location does not determine execution location; app maintenance belongs to About. |
| **Browser Use** | Web automation through bundled `agent-browser` and MCP, using compatible installed Chromium. Sessions are scoped by execution Device and complete Chat Address; browser binaries, profiles, logins, and handles remain local. |
| **Computer Use** | Native-app automation through bundled Cua Driver and MCP. The resident owns native permission hosting and shared physical-desktop input coordination; per-Agent MCP connections do not create separate desktops. |

## Agent configuration and Harness

Details: [Agent resolution](docs/adr/0005-agent-profile-running-config-and-resolved-agent.md),
[work contract](docs/adr/0008-agent-work-contract-and-collaboration-mode.md),
[execution bounds](docs/adr/0009-shared-agent-execution-budget.md),
[Chat snapshots](docs/adr/0028-chat-running-config-and-turn-stable-snapshot.md).

| Term | Meaning and boundary |
|---|---|
| **AgentProfile** | Resolved Agent identity and content, including loaded prompt text. Behavior knobs belong in `AgentsRunningConfig`, not parallel profile fields. |
| **AgentsRunningConfig / Agent Running Config** | Agent runtime-behavior configuration and creation-time template for Chats. Later template edits do not silently change existing Chat snapshots. `max_iters` counts Tool rounds; `0` removes that round limit. |
| **ResolvedAgent** | Registry result combining `AgentProfile` and `Arc<AgentsRunningConfig>` coherently. Pass this value through execution routes rather than separate arguments. |
| **Explicit Configuration Loading** | Read the explicitly selected current-schema configuration and propagate errors. Creation/seed owns defaults; reads preserve unknown extension fields and do not repair invalid JSON or synthesize fallback Agents. |
| **Agent Tool Configuration** | Explicit Agent-owned tool inventory. Missing or empty entries grant no builtin tools; catalog additions and reads do not backfill unrelated tools. |
| **Workspace Prompt Directory** | Per-Agent `workspaces/{agent}/prompts/` containing ordinary prompt files. Prompt selections are relative to it; the workspace root is not scanned as prompt context. |
| **Agent Prompt File Sync** | Committed per-Agent prompt Markdown publication, separate from global Prompt Library and Structured Memory. Explicit deletion/rename facts remain recoverable through Full Export. |
| **Prompt Library File Commit** | Global library owns per-file Markdown writes/publication, including explicit builtin restoration. Bulk operations validate first but commit per file; startup seed and non-Markdown attachments retain local-only semantics. |
| **Chat Running Config / Chat Configuration Snapshot** | Complete materialized Chat-owned configuration: Harness, Hook revisions, model/reasoning, tools, authorization, and environment binding. Immutable snapshots have one coherent current head/Configuration Epoch; they are not sparse live Agent overrides. |
| **Effective Turn Running Config** | The immutable snapshot captured when Authority claims a Turn. Model calls, tools, retries, and projection share it; configuration edits affect later claimed Turns, including queued inputs. |
| **Harness Configuration** | Chat-owned declarative selection/composition of existing execution capabilities. Slots, Contributions, and settings cannot create canonical state, own resource lifecycle, or weaken Authority/Security. Explicit invalid selections fail. |
| **Effective Harness** | Resolved immutable implementation handles for frozen Turn inputs. Keeping a handle callable does not transfer ownership of the implementation's lifecycle; mutable execution state is separate. |
| **Runtime Composition** | Read-only process-level resolver producing an Effective Harness. Configuration mutations remain with their owning domains. |
| **Config Tool** | Current-Chat setting read/write Tool. One call changes one typed setting atomically after cross-setting validation; `reset=true` materializes current defaults. Active Turns remain unchanged; external availability is checked at Turn resolution. |
| **Base Agent Contract** | Runtime-owned mandatory working guidance, separate from user prompt files, conditional product protocols, and execution permissions. |
| **Model Call Context** | Immutable per-call Harness, prompt, environment, capability, and model inputs captured at a legal boundary. Context Projection alone assembles these with history into the normalized request. |
| **Agent Collaboration Mode Compatibility** | Existing `Default`/`Plan`/`Execute` compatibility values map to Turn Harness overrides. Do not add a second independent Mode state or confuse them with ACP Chat Mode. |
| **Execution bounds** | Limits owned by actual runtimes: iterations, optional deadlines, inactivity, tool/output limits, concurrency, and cancellation. Provider usage is observation; the removed shared Execution Budget ledger is not a current entity. |

Harness schema backfill adds only missing required settings under explicit schema
rules; it does not refresh existing values from changed defaults. Module Disable
retains settings; Remove deletes them after active uses finish. A required Slot
needs a valid replacement before its implementation can be disabled or removed.
`Config` uses stable setting IDs, rejects unknown keys, and leaves the prior
configuration intact on validation failure; `null` is not a reset sentinel.

## Model reasoning

Details: [model knowledge and reasoning](docs/adr/0006-model-knowledge-catalog-and-reasoning-control.md).

| Term | Meaning and boundary |
|---|---|
| **Model Knowledge Catalog** | Known model capabilities and suggested defaults used for creation/import/migration. Runtime requests read explicit configuration, not model-name guesses. |
| **Reasoning Control** | Request-side reasoning strength, separate from emitted thinking blocks and display/history policy. User-facing budget strength is **Reasoning Budget**; the portable field remains `thinking_budget`. |
| **Model Reasoning Default** | Explicit per-model reasoning settings. Supported effort defaults to `medium`; budget defaults come from model metadata, not a global inference formula. |
| **Chat Model Reasoning Preference** | Complete reasoning snapshot for the selected `{provider_id, model}` slot. Overrides the model default in that Chat; changing model resets to its defaults without editing provider configuration. |

## Chat identity, storage, and navigation

Details: [address and lazy materialization](docs/adr/0018-chat-address-scope-and-lazy-materialization.md),
[Projects](docs/adr/0019-project-scoped-chat-and-fixed-resource-binding.md),
[Session storage](docs/adr/0020-session-file-and-append-only-message-log.md),
[Chat Index proposal](docs/adr/0021-chat-index-complete-non-authoritative-projection.md).

| Term | Meaning and boundary |
|---|---|
| **Chat** | The user-facing conversation with one immutable Scope, durable history, and Turns. Agent-Scoped and Project-Scoped Chats share one domain model; bare “session” is ambiguous. |
| **Chat Scope / Chat Namespace** | The fixed Agent or Project storage/routing partition. Scope chooses Workspace, Chat Index, Media root, and Sync partition; it does not substitute for behavior policy or execution Authority. |
| **Chat Address** | Complete immutable locator: `{agent_id, chat_id}` or `{project_id, chat_id}`. A Project Chat's selected Agent is not part of its address. |
| **Chat ID** | UUID generated before navigation. Global uniqueness does not remove the need for complete scoped addresses in storage, controls, Tasks, approvals, and Sync. |
| **Chat Policy** | Agent Assignment, Resource, and Authority Placement policies independent of storage Scope. Shared Chat logic evaluates policy without duplicating Agent/Project implementations. |
| **Agent-Scoped Chat** | Chat in a fixed Agent Namespace, normally with fixed Agent selection and selectable execution Authority. Namespace membership does not give the Agent a second ownership of Chat state. |
| **Project-Scoped Chat** | Chat in a Project Namespace, normally with selectable Agent and resource-bound Authority. Changing Agent never moves its storage or resource binding. |
| **Chat Agent Selection** | The executor selected for future Turns under Chat policy. |
| **Turn Agent Assignment** | Immutable executor choice captured for a claimed Turn; changing future selection does not relabel existing work. |
| **Workspace Ownership / Chat Scope Store** | Scope determines the managed storage container; shared stores operate on a complete Chat Address without inventing a global Chat directory. |
| **Rua Root** | Rua's managed application-data root, distinct from any user's Project source directory. |
| **Workspace Root** | Managed Agent or Project workspace for Chat data, Media, and related Rua-owned state. |
| **Project Resource Root** | User directory on the Project Resource Device where execution operates; distinct from the managed Project Workspace. |
| **Secret Root** | Secret-material root configured separately from Rua Root and Workspace paths. Its location does not determine each domain's replication policy. |
| **Chat Media** | Scope-workspace attachments with portable `media/<uuid>` references. Foreground transfer routes to Authority; authority-switch readiness verifies Required Media separately from the Conversation Manifest. |
| **Lazy Chat** | Valid scoped address before durable materialization. Read-only navigation/discovery and staged attachments do not create a Chat; the first accepted stateful operation materializes coherent state/configuration. |
| **Chat Draft / Draft Agent Assignment** | Browser-local unsent input, attachments, and draft executor/options. Draft state is not saved history or execution Authority. |
| **Empty Conversation State** | Valid empty/lazy presentation; it does not prove a backend Chat was created. |
| **Active Chat** | Complete address selected by the route. Switching views detaches observation without stopping backend work. |
| **Agent Chat Route / Project Chat Route** | Canonical `/agents/{agentId}/chats/{chatId}` and `/projects/{projectId}/chats/{chatId}` routes. Scope comes from the address, not a global selector. |
| **Conversation Landing Resolution / Agent-Scoped Chat Navigation** | Resolve an entry action to a complete Agent Chat route before mounting its view, using validated per-Agent browser hints or a fresh Lazy Chat ID. |
| **Chat Navigation Hint / Browser Last Active Chat / Recent Chat Stack** | Bounded browser-local per-Agent navigation hints. They are not backend metadata or saved-list ordering. |
| **Active Project Resolution / Browser Project Navigation Hints** | Resolve Project entry/navigation from an explicit Project and validated browser-local hints, preserving the complete Project Chat Address. |
| **Project Agent Selector / Project Last Selected Agent** | Future-Turn/draft Agent choice and its browser-local convenience hint, separate from Project identity and persisted Turn assignment. |
| **Unavailable Project Chat Agent** | A retained selection that cannot currently execute; expose its reason rather than silently selecting another Agent. |
| **Project Agent Selector Active-Work Guard** | Agent switching must respect active-work/configuration admission; it never rewrites an already claimed Turn. |
| **Console Conversation Navigation / Conversation Navigation Activity** | Route transitions and pending navigation presentation, separate from backend Turn and queue state. |
| **Chat Drawer** | Scoped Chat-list/navigation presentation; does not own history or execution. |
| **Chat Metadata / Chat List Order** | Durable title, times, pin/archive state, preview, and list-facing ordering. Browser navigation hints do not change these facts. |
| **Session File** | Small authoritative Chat header: identity, metadata, control/configuration heads, and durable-history pointers. It does not contain a growing full transcript. |
| **Message Log** | Append-only raw chronology with stable message IDs and monotonic sequence numbers. Source for history, Recall, and Context Projection tail reads. |
| **Turn Index / Turn Metadata** | Durable Turn/variant boundaries and lifecycle metadata, including selected response path and timing. |
| **Message Metadata** | Message-level identity, Turn, role, timing/status, and provider/tool linkage for exact retrieval and audit. |
| **Chat Index** | Repairable non-authoritative scoped list projection, not another owner of Session state. ADR-0021 remains Proposed. |
| **Assistant Response Variant** | Regenerated candidate response for one Turn; the next user Turn commits the chosen path. |
| **Conversation Branch Point** | Inclusive assistant response anchoring rollback/branch; later content is excluded. |
| **Session Context Snapshot** | Persisted provenance of context that affected execution, rather than only a pointer to mutable Agent defaults. |

## Conversation execution and control

Details: [Authority and fencing](docs/adr/0022-conversation-authority-placement-and-epoch-fencing.md),
[Turn Writer and queue](docs/adr/0023-active-turn-turn-writer-and-input-queue.md),
[Stop/disconnect](docs/adr/0024-stop-interjection-and-viewer-disconnect.md),
[Runtime Notes](docs/adr/0025-runtime-notes-as-typed-durable-conversation-state.md),
[Chat CLI](docs/adr/0035-chat-cli-single-request-adapter-and-resident-server-boundary.md).

| Term | Meaning and boundary |
|---|---|
| **Conversation Authority** | Single backend owner of one Chat Address's placement, epoch, Active Turn, input queue, controls, and Turn Writer. Viewers/transports never become a second Authority. |
| **Conversation Authority Placement / Authority Device** | Explicit Device chosen to execute the Chat. Agent Chats may switch by the authority contract; Project Chats remain resource-bound. Unreachability never authorizes automatic failover. |
| **Authority Epoch** | HLC generation fencing placement and mutations. Session control state owns the current epoch; historical stamps do not select today's authority. |
| **Conversation Control State** | Durable placement/epoch and ACP binding affecting future execution, stored in the Session File. |
| **Conversation Control View** | Read-only authority-routed projection of active Turn, queue, placement, availability, and allowed controls. A replica's empty local runtime is not authoritative idle state. |
| **Conversation Authority Router** | Common local/remote operation route. May follow one verified higher-epoch redirect within an operation; an explicit switch targets the selected new Authority. |
| **Authority Supersession / Authority Redirect** | Committed higher-epoch replacement notice and typed response identifying the newer Authority. They do not elect a Device or imply that it is reachable. |
| **Verified Authority Observation** | Verified newer placement learned by a replica, persisted without minting another epoch or republishing itself as author of the switch. |
| **Conversation Turn** | Durable semantic exchange: one user input and selected assistant/tool/Runtime Note path. `turn_id` is the foreground execution identity; there is no separate business Run. |
| **Active Turn** | At most one non-terminal Turn per Chat, owned by Conversation Authority, with transient execution phase and allowed controls. Approval wait remains active. |
| **Active Turn Execution** | Runtime-only cancellation, driver, stream, approval, and foreground-tool handles keyed by `turn_id`. Not another durable state machine or identity. |
| **Turn Writer** | Single Authority-owned write surface for one Turn's canonical messages, tool events, notes, variants, and terminal persistence. |
| **Conversation Driver** | Builtin ReAct, ACP, or another execution adapter. Owns executor internals but writes Rua Conversation State through Authority/Turn Writer. |
| **Conversation Input Queue** | Runtime-only Authority-owned accepted input queue. Order, edits/deletes, pause/drain, and `input_id` control stay on that Device; optimistic viewer buffers are mirrors, not synced history. |
| **Viewer Queued Input** | Non-durable remote submission; once acknowledged, Authority owns it independently of the viewer's connection. No durable source outbox. |
| **Conversation Display Adapter** | Bounded fan-out/replay implementation for live output. It cannot decide busy, idle, orphan, or queue business state. |
| **Viewer Disconnect** | Observation ends or reconnects; execution, Tasks, and accepted queue entries continue. |
| **User Stop** | Explicit interruption of the Active Turn, preserving committed content and queued inputs while pausing automatic user-queue drain. Background result delivery has its own continuation policy. |
| **Provider Disconnect** | Upstream model failure candidate persisted by Authority; distinct from viewer loss or User Stop. |
| **Turn Terminal Event** | Published after durable terminal persistence and coherent transition to the next Turn or idle. Driver EOF, approval wait, and viewer closure are not terminal facts. |
| **Runtime Note** | Typed durable runtime-authored fact in Message Log chronology, separate from authored messages and transient Live Activity. Injected only at legal model boundaries. |
| **Chat CLI Invocation** | One resident-server request that may queue, streams only its own eventual Turn, and closes at that Turn's terminal event. CLI is not a second execution runtime. |

## Hooks — proposed vocabulary

Details: [two-layer Product Hooks](docs/adr/0029-two-layer-product-hooks.md)
(**Proposed**). These names describe that design; they do not establish a complete
implemented Hook capability or override current lifecycle/security contracts.

| Term | Meaning and boundary |
|---|---|
| **Active Turn Hook / Operation Hook** | Extension of a Turn lifecycle event or canonical Model/Tool operation respectively. Neither owns Authority, persistence, or external ACP internals. |
| **Stop Hook / Candidate Completion** | Evaluation of a Driver's typed completion proposal before terminal commit. Rejection continues the same `turn_id`; acceptance permits commit. |
| **Completion Protocol Capability** | Driver declaration of accept-only versus continuable completion support. Incompatible Stop Hooks fail validation before execution. |
| **Hook Definition / Qualified Hook Identity** | One event-specific immutable configuration, named by owner namespace plus logical ID and revision digest. Definitions compose additively rather than overriding by display name. |
| **Hook Handler Implementation / Hook Handler Kind** | Reusable executable mechanism (`command`, isolated `plugin`, trusted `native`), separate from event binding and trust. Reserved unsupported kinds fail explicitly. |
| **Hook Data Profile** | Revision-bound bounded event projection permitted for a handler. Expanded content access is an explicit revision/risk change; secrets are not implicit context. |
| **Hook Executor Content Mode** | `live` resolves current declared content; `pinned` verifies an immutable artifact. Definition revision and executor bytes are distinct identities. |
| **Hook Execution Profile / Hook Execution Placement** | Declared local execution capabilities/limits, resolved by Runtime Authority on the enclosing Turn's Authority Device. Portable definitions never store resolved device-local plans. |
| **Scoped Hook Defaults / Project Hook Declaration** | Agent/Project selection defaults and explicit repository declarations. Discovery is not enablement; new logical Hooks start disabled and expanded execution semantics require renewed enablement. |
| **Chat Hook Configuration / Effective Hook Set** | Materialized Chat revision selection and the ordered immutable set frozen for a Turn. Later source edits do not mutate active work. |
| **Hook Ordering Boundary** | Per-event definition ordering constrained by fixed lifecycle/security boundaries. Validate cycles, required dependencies, and ambiguous targets before execution. |
| **Hook Invocation Identity** | Stable logical event/handler idempotency key, separate from delivery attempt. Unknown synchronous mutation outcomes are not blindly replayed after loss. |
| **Hook Activity** | Typed Turn-bound Runtime Note recording a material Hook effect, provenance, outcome, and continuation cause. Not a parallel transcript. |
| **Hook Budget Accounting** | Synchronous work consumes caller-provided remaining Turn time; local timeouts and continuation cannot extend it. Post-terminal observers have separate managed delivery bounds. No shared token ledger. |
| **Hook Revision Lifecycle** | Archive before deletion; preserve referenced revisions/artifacts while defaults, Chats, active work, or deliveries still require them. |

## Conversation projection, history, and Memory

Details: [model-input assembly](docs/adr/0026-context-projection-owns-model-call-assembly.md),
[History Replacement](docs/adr/0027-durable-history-replacement.md),
[Structured Memory](docs/adr/0045-structured-memory-aggregate.md),
[tool output limits](docs/adr/0010-tool-output-visible-limits-and-raw-capture.md).

| Term | Meaning and boundary |
|---|---|
| **Conversation State** | Durable Chat messages, variants, notes, and context-folding/checkpoint state. It is not long-term Memory or live execution handles. |
| **Context Projection** | Sole bounded normalized model-request assembly from effective system/capability inputs, selected durable history, pending notes, and current input. Providers and Hooks do not independently append history. |
| **Model Call Boundary** | Completed model response plus all matching tool results made durable before the next request is prepared. Safe point for projection and pending runtime delivery. |
| **Projection Read Path / Projection Cache** | Bounded active-checkpoint, retained-range, and raw-tail reads with disposable derived caching. Normal model calls do not scan the full lifetime transcript. |
| **Projection Planner / Context Target Band** | Shared fit/maintenance planning and model-derived target range. Measure the complete request before deciding maintenance; fitting input does not invoke summarization. |
| **Projection Truncation** | Optional shortening of older tool results in model input only, preserving raw Message Log. Distinct from execution-time tool output limits. |
| **Projection Exclusion Set** | Durable replicated non-user message IDs hidden from future model projection while remaining in history. Updates are validated by Authority. |
| **Compression** | Blocking Summary Checkpoint maintenance when the measured projection reaches its threshold, followed by rebuild and remeasurement. |
| **Reactive Compaction** | Provider-context-overflow recovery through the same planner/checkpoint path, not string-based request-size guessing. |
| **Early Background Compaction / Foreground Context Maintenance** | Background preparation and user-requested `/compact` using the same checkpoint creation/publication contract. |
| **Summary Checkpoint / Checkpoint Publish CAS** | Complete compressed projection for a durable prefix, retaining required verbatim ranges. Publish only if the starting checkpoint is still current; do not overwrite newer maintenance. |
| **History Replacement / History Replacement Record** | Non-blocking creation and durable publication of a validated replacement for an exact completed execution-trace range. Raw chronology remains intact. |
| **Execution Trace Block / Execution Trace Digest** | Eligible assistant/process content span and its derived concise replacement preserving exact identifiers, outcomes, side effects, failures, and unresolved work. Replacement applies only to matching source anchors. |
| **Context Window Management** | Short-term fit, compaction, checkpoint, and truncation machinery. A compressed conversation summary is not long-term Memory. |
| **Conversation Recall** | Current-Chat exact raw-message retrieval, including spans omitted from active projection. |
| **Conversation Search / Text Search Primitive** | Saved-Chat discovery/content search and its shared literal/regex search mechanism. Separate from current-Chat Recall and Memory Search. |
| **LLM Debug Capture** | Explicit local non-secret request/projection diagnostics outside Session/Sync. Failures do not affect normal execution; captures are not automatically uploaded. |
| **Memory / Explicit Memory Aggregate** | Long-term structured records, evidence, relations, visibility, pinned slots, and derived search indexes. Ordinary conversation/tool output stays in Chat history unless explicitly recorded as a durable fact. |
| **Agent-Scoped Memory API** | Agent-path-owned structured Memory operations, including search, writes, health, rebuild, and deletion; no header/body-selected or file-shaped parallel authority. |
| **Memory Visibility Grant** | Item-level user-controlled cross-Agent read permission, without changing record ownership. Foreign records may be read, not mutated by another Agent's AutoDream. |
| **Memory Search Projection** | Local rebuildable text/semantic/resource/graph search view of structured records. Indexes and optional embeddings are not synchronized canonical Memory. |
| **Pinned Memory Slot** | User-controlled references to selected structured Memory rendered into prompt context with Device provenance, not an automatic summary or TODO store. |
| **Structured Memory Sync Apply** | Domain-owned application of canonical Memory records and provenance, followed by derived-index refresh. |
| **Memory Device Context** | Runtime-stamped writing Device and resource Device provenance attached to structured records and replicated with them. Agents do not invent these fields. |
| **Structured Memory Protocol Prompt** | Runtime-injected guidance for `memory_search`, `memory_get`, and `memory_write`; not a user-toggleable `AGENT_MEMORY.md` file. |

Removed Memory Markdown (`MEMORY.md`, `MEMORY/*.md`) and `MemoryStore`,
`MemoryPolicy` / `DisplayPolicy` aliases are not compatibility surfaces.
Prompt APIs and Sync do not revive them. Context management and long-term Memory
keep separate public interfaces even if implementation objects are shared.

## Console and live observation

Details: [owned Console](docs/adr/0052-owned-console-and-conversation-view-runtime.md),
[whole-Turn DOM retention](docs/adr/0056-whole-turn-conversation-dom-residency.md),
[live hydration](docs/adr/0044-live-assisted-hydration-and-cross-device-conversation-continuation.md),
[Markdown Plus](docs/adr/0048-markdown-plus-editable-visual-documents.md).

| Term | Meaning and boundary |
|---|---|
| **Console Transport** | Shared browser-to-resident transport used by scoped feature runtimes. Transport connectivity is not Chat execution state. |
| **Conversation View Runtime** | Complete-address-keyed owner of mounted Chat hydration, live observation, optimistic input, control projection, and recovery. Route components/renderers do not independently run the lifecycle. |
| **Conversation UI Projection** | Render-ready stable/live view keeping content readiness, current Turn, live attachment, and submission delivery orthogonal. Synced `in_progress` history is not proof of current execution; terminal convergence is per `turn_id`. |
| **Model Dialogue Item** | Provider-neutral role plus ordered content parts assembled for a model call. It may group several fine-grained Message Log records; its boundary is not persisted message identity or UI semantics. |
| **Chat Render Block** | Shared presentation of tall/structured message content, with local collapse/sticky controls. It does not persist state into Message Log or own message lifecycle. |
| **Canonical Conversation Identity** | Authority-issued `turn_id`, `message.id`, and content/tool identities shared by live and stable paths. Optimistic IDs reconcile without remounting canonical content. |
| **Conversation Stable Snapshot Cache** | Disposable saved-history projection keyed by full Chat Address and durable revision, separate from live mutable state. |
| **Live Conversation Runtime State** | Runtime-only output, queue/control observations, and optimistic overlays. Never persisted or synced as stable history. |
| **Active Turn Recovery Checkpoint / Durable Conversation Prefix** | Coherent persisted in-progress boundary and its durable history prefix. Recovery data does not recreate execution resources on another Device. |
| **Conversation Live Activity** | Authority-owned runtime event timeline for user/assistant/tool output, control revisions, and terminal events before/during stable-history delivery. Not Sync or a second message store. |
| **Live Activity Replay Window / Live Activity Gap** | Bounded retained ordered activity and explicit notice that a requested position is too old. Gaps request durable refresh without blocking live ingestion. |
| **Live Activity Viewer / Live Activity Discovery** | Attached observer and the process verifying replicated placement against the target's initial Control View before accepting output. Local stable history loads in parallel. |
| **Active Stream Owner** | Typed `Chat { address }` or `Terminal { session_id }` identity. Terminal activity never enters fake Agent/Chat namespaces. |
| **Approval View Model** | Display projection joining tool-call rows with canonical Approval Authority state; UI does not manufacture decisions from message text. |
| **Markdown Plus** | Editable visual blocks embedded in authoritative Markdown source with bounded non-executable rendering. |

Loaded Message data is separate from retained whole-Turn DOM and presentation
state. ADR-0055's combined virtualization/history reclamation was withdrawn;
ADR-0056 governs current reading-anchor and DOM retention behavior.

## Collaboration and background Tool Tasks

Details: [collaboration Chats](docs/adr/0053-collaboration-chats-and-background-tool-tasks.md),
[durable Task state](docs/adr/0031-durable-task-runtime-and-attempt-state-machine.md),
[Task controls](docs/adr/0032-task-runner-watch-control-and-authority-routing.md),
[ACP Chat Mode](docs/adr/0033-acp-chat-mode-client-projection-and-permission-relay.md).

| Term | Meaning and boundary |
|---|---|
| **Collaboration Chat** | Independent Chat history/Turns executed by a configured Rua Agent or ACP runner, related to one source Chat. Executor identity is fixed; the source relationship is provenance, not lifecycle ownership. |
| **Chat Default Execution Environment** | Device-aware execution resource environment, separate from executor and managed Chat storage. Collaborators inherit the source environment, including Project working directory, rather than moving to the executor's Workspace. |
| **Background Result Continuation** | Runtime delivery of collaboration/Tool Task results at an active source's legal boundary or a follow-up Turn for an eligible idle source. No fake user message or read/wait/sleep loop to keep a Turn alive. |
| **Waiting for Background Results** | Presentation derived from pending work/delivery facts. An idle collaboration is not an Active Turn; only an active target Turn counts once toward running activity. |
| **Task / Background Tool Task** | Independently tracked background Tool work owned by one complete Chat Address. Agent and ACP conversations execute in Chats, not Task Attempts. |
| **Task Runtime** | Single process-level scheduler/Runner registry and durable local Task lifecycle owner. Runtime scope does not make Tasks globally visible. |
| **Task Record / Task Attempt / Task Result** | Immutable Task specification and dependencies, ordered executions, and preserved terminal results. Retry appends an Attempt under the same Task ID without rewriting prior outcomes or side effects. |
| **Task Runner / Tool Runner / Runner Session** | Adapter executing Tool work and its live process/session. Adoption preserves one already-running Tool Call rather than launching a duplicate. |
| **Task Execution / Task Cancellation** | Execution-device-owned work and its cancellation subtree. It may outlive the source Turn; viewer detachment does not cancel it. Runtime loss interrupts without automatic replay. |
| **Effective Task Authorization** | Attempt snapshot of submitting Chat/Turn configuration and authorization. Agent IDs are provenance, not live policy lookup; new Attempts/Retry must pass inherited-configuration admission. |
| **Task Control Surface** | `task_create`, `task_read`, `task_list`, `task_cancel`, `task_send_input`, `task_retry`. Read is bounded state/output with cursor and optional finite wait; wait expiry never cancels execution. |
| **Foreground Tool Wait Limit / Tool Transfer Policy** | Optional synchronous wait and transferability contract. Expiry adopts the same still-running eligible execution exactly once, without cancellation/replay; completion versus adoption has one foreground result. Wait time is not an execution deadline. |
| **Task Input / Task Input Capability** | Real Tool/process stdin or EOF supported by the Runner Session, not Agent steering or a second conversation. |
| **Task Activity Subscription / Runner Live Output** | Canonical output/state observed by UI independently of model reads. Deterministic output limits replace Agent-generated progress summaries; delivery/read coordination avoids duplicate automatic projection. |
| **Task Device Context / Task Context** | Runtime-stamped execution/resource provenance and Task-owned execution content. Local artifacts do not become portable merely because the terminal result is mentioned in Chat history. |

Collaboration is one level with finite per-source in-flight admission, enforced
outside prompts. Independent target Chats are not recursively stopped/deleted
with their source; late results never recreate a deleted source. Source security
and environment changes follow inherited-configuration admission (ADR-0028/0053).
User Stop preserves the user-input queue pause while background results can still
be processed. Task dependencies are bounded and same-Chat; retry re-evaluates
blocked dependants. Task records/output remain device-local. Moving Conversation
Authority does not relocate an accepted Task's execution Device.

SubAgent/Agent/ACP Task runners, model Watch/Unwatch, `task_await`, Task Notification
Turns, Agent completion prompts, and Handoff Directory are retired vocabulary,
not requirements for compatibility adapters. Current contracts are the amended
ADR-0031/0032 and ADR-0053.

## Interactive terminals

Details: [terminal authority and Viewer Runtime](docs/adr/0034-interactive-terminal-session-and-viewer-runtime.md).

| Term | Meaning and boundary |
|---|---|
| **Interactive Terminal Session** | Device-owned live PTY/shell independent of Chat Turns and viewer lifetime. Android uses a native PTY with a Linux guest shell. |
| **Terminal Execution Device / Terminal Session Authority** | Device running the PTY and authoritative owner of registry, output, geometry, leases, and lifecycle. |
| **Terminal Session Placement / Terminal Cross-Device Parity** | Explicit target Device and the same local/remote behavior above placement routing. Terminal identity is not a Chat Address. |
| **Terminal Device Command / Terminal Trust Boundary** | Authenticated remote operation validated by target-side terminal authority; connectivity does not bypass membership, lease, or session checks. |
| **Terminal Session Activity** | Ordered target-side output/control events with canonical recovery positions. |
| **Terminal Session Registry / Terminal Session List** | Runtime-owned live sessions and their discoverable read model; neither promises durable resume after process loss. |
| **Terminal Session Viewer / Terminal Viewer Identity** | Attached observer with its own viewer identity, distinct from Session and Device identity. |
| **Terminal Input Lease / Terminal Input Takeover** | Exclusive authority-issued input/geometry permission and explicit transfer. Passive viewers do not write or resize the PTY. |
| **Terminal Viewer Viewport / Terminal PTY Geometry** | Local presentation dimensions versus authoritative process rows/columns. Lease holder coalesces stable geometry changes; resize does not reattach or replay state. |
| **Terminal Viewer Runtime** | Long-lived owner of attach/hydrate/live/suspend/resync lifecycle, surviving route/tab changes. One queue applies positioned live PTY output. |
| **Terminal Stream Cursor** | Authority sequence plus original PTY byte offset, not JS string length or rendered rows. Duplicates are ignored; gaps require fresh authority snapshots. |
| **Terminal Output Replay Buffer** | Bounded runtime-only original-byte ranges for attach races and retransmission, not durable transcripts or visible catch-up animation. |
| **Terminal Session Reattach** | Recovery while the target PTY/registry still live. Coherent snapshot and tail hydrate an invisible staging xterm before reveal; ordinary navigation, lease takeover, resize, and input do not reattach. |
| **Terminal Detached TTL** | Cleanup window with no viewers; attached viewers suppress it and PTY exit ends it. It is not recovery after target process loss. |

## Device identity, networking, and remote commands

Details: [pairing proposal](docs/adr/0036-master-identity-device-list-and-pairing-trust.md),
[shared Device Sessions](docs/adr/0037-device-network-runtime-and-shared-device-sessions.md),
[Authority fencing](docs/adr/0022-conversation-authority-placement-and-epoch-fencing.md).

| Term | Meaning and boundary |
|---|---|
| **Device Identity** | Local Ed25519 identity used by endpoint and replica Author. It is not the network root; resetting Sync does not rotate it. |
| **NetworkSecret** | Network root determining the iroh-docs namespace and deriving SyncKey. Correct secret/bound membership, not a Device List row, establishes network membership. |
| **SyncKey** | In-memory symmetric key derived from NetworkSecret for envelope/blob encryption; not a separately stored per-Device recipient key. |
| **Device List** | Paired-device display and peer-selection metadata, not proof of decryption authority. |
| **Bound Device Network / Bound Sync Network** | Authenticated Device/network binding and the matching Sync namespace/consumer binding. Consumers do not reconstruct trust independently. |
| **Device Network Runtime** | Process-wide owner of endpoint/router, discovery, relay, shared Device Sessions, connectivity, and network lifecycle. Sync is a consumer rather than owner of general reachability. |
| **Device Session** | Lazy authenticated/versioned logical peer session, stable across replaceable physical connection generations. Session Manager owns connection establishment and bounded reconnect. |
| **Device Session Command Stream** | Bounded request/response transport for Device Command Bus. Business identity, epoch, lease, and acceptance stay feature-owned. |
| **Device Session Live Activity Stream** | Reconnectable feature subscription. Replaceable old connection generations must terminate their streams within bounded time; replay/recovery remain feature-owned. |
| **Device Session Bulk Transfer Stream** | Lower-priority verified transfer with acknowledged offsets, bounded resume, and atomic writes. Failure does not replace the shared session or create a durable queue. |
| **Device Connectivity Projection** | Ordered peer transport state: `connected`, `connecting`, `not_connected`. Business rejection/history/lease errors are not “Device offline.” |
| **Device Command Bus / Device Command Delivery Attempt** | Semantic control delivery to one selected Authority via a bounded send/reconnect operation. No acknowledgement means unconfirmed delivery; no durable source outbox or background retry. |
| **Authority Placement Binding** | Commands remain bound to explicit Device/epoch; failure, new viewers, or a more capable peer cannot silently change the target. |
| **Remote Conversation Input Delivery / Remote Conversation Input Delivery Payload** | Self-contained submission of address, Device/epoch, base manifest, idempotency/order IDs, input/media, and deadline. Authority alone accepts, queues, executes, and persists it. |
| **Submit Device ID / Remote Submission ID** | Input provenance and transport correlation/idempotency. Authority `input_id` owns queue controls; canonical Turn/message IDs own rendering. |
| **Remote Input Delivery Sequence / Remote Input Ordered Delivery** | Controller per-Chat attempt ordering, superseded by Authority queue order after acceptance. Wall-clock time is not an ordering authority. |
| **Remote Input Delivery State / Remote Submission Pending Projection** | Sending/unconfirmed, accepted, then canonical-visible transport presentation. Temporary pending UI is not a durable Run, Task, Turn, or job. |
| **Remote Input Attachment Transfer** | Future per-input transfer capability, distinct from target-local filesystem paths and existing Chat Media authority transfer. This name alone does not claim implementation. |
| **Authority Device Turn Writing / Authority Input Acceptance** | Target-side canonical acceptance and single-writer persistence. Controller optimism never prewrites official user messages or Turns. |
| **Conversation Authority Control Command** | Address/epoch-bound control validated by target Authority, distinct from replicated history. |
| **Remote Authority Device / Chat Execution Target** | Explicit execution placement, not a synonym for reachable/best Device. |
| **Conversation Manifest** | Revision/hash description of coherent durable Chat subfiles used to validate hydration and permitted authority continuation. |
| **Remote Turn Result Priority Sync** | Prioritized publication/replication of committed results, not replication of live execution state or a command-delivery queue. |
| **Local Authority Delivery Short-Circuit** | Local adapter for the same authority validation/acceptance contract, avoiding a network loop without bypassing it. |
| **Pairing / Pairing PIN / Pairing Rendezvous** | Authenticated network joining via one-time Magic Wormhole/SPAKE2 rendezvous and PIN. Ongoing traffic uses Device networking; bare “mailbox” is reserved for this rendezvous, not User Inbox. |
| **Transport (iroh)** | Peer transport/discovery plus docs/blob replication infrastructure hidden behind Device Network Runtime and Sync. Public relays/rendezvous do not own canonical user data. |
| **Always-Online Assumption / Sync Mode** | User operations expect internet and Relay-style device handoff. Peer unreachability still fails bounded operations; concurrent portable edits use domain merge rules rather than silent blanket overwrite. |

Targeted Chat Materialization, Targeted Chat ID Reservation, and Pending Targeted
Chat Projection are retired remote-first creation terms. Materialize locally
before an optional Manifest-verified authority switch. Chat Execution Context is
the old read-only Control View name; Sync Runtime Network Presence is replaced
by Device Network Runtime presence.

## Replication and durable hydration

Details: [encrypted operations](docs/adr/0038-sync-operations-hlc-and-envelope-encryption.md),
[domain publication](docs/adr/0039-domain-sync-adapters-and-publication-seam.md),
[Sidecar Store](docs/adr/0040-sqlite-inbound-inbox-and-sync-sidecar-store.md),
[First Sync proposal](docs/adr/0041-bidirectional-first-sync.md),
[incremental repair](docs/adr/0042-incremental-apply-and-repair-scan.md),
[live-assisted hydration](docs/adr/0044-live-assisted-hydration-and-cross-device-conversation-continuation.md).

| Term | Meaning and boundary |
|---|---|
| **Sync Scope** | Portable profiles/security, prompts, Skills, providers (including envelope-encrypted API keys), Structured Memory, and durable Chat snapshots/history. Device-local defaults, env vars, Cron jobs, Channel/login state, local-model entries/weights, usage ledger, paths, runtime inventory, power settings, queues, and Task execution do not travel. |
| **Merge Strategy** | Data-shaped merge: structured fields merge independently; free text uses three-way merge and HLC for overlapping hunks; binary uses whole-file HLC; Chat subfiles use their domain records. Text sniffing preserves executable modes. |
| **Conflict Policy** | Silent resolution or non-blocking Review retaining losing free-text content in sidecars. No conflict markers in executable/live files and no blocking prompt. |
| **Deletion** | Explicit domain tombstone, never inferred from absent files. Ordering/retention follow the domain contract; permanent Chat Delete Intent is terminal rather than an ordinary editable file. |
| **Committed Portable Fact / Replication Emit** | Domain reports a durable local canonical commit for explicit publication. Endpoints and filesystem watchers do not invent replication events. |
| **Replication Publication Seam** | Converts typed committed facts into bound-network HLC operations, encryption/blob context, and coalesced Sync Trigger notifications. Raw `SyncChange` is internal transport vocabulary. |
| **Full Export / Seed Manifest** | Export current canonical records/tombstones while distinguishing untouched automatically seeded bytes from user edits. First Sync snapshots must not republish newly received peer content as local authorship. |
| **SyncChange / Wire Payload** | Encrypted operation carrying schema, op/domain/record/kind, Device, HLC, optional base, and payload. State/subfile snapshots remain separate from live token deltas; small file bytes inline, large files use one encrypted blob reference. |
| **Opaque Replica Log Key** | Stable HMAC of SyncKey, domain, and record ID. Operation IDs, paths, HLC, and payload stay inside encrypted values, not path-shaped or random operation keys. |
| **iroh-docs Namespace / Author mapping** | One NetworkSecret-derived namespace; each Device Identity signs its own entries as Author. |
| **HLC Timestamp** | `(wall_ms, logical_counter, device_id)` ordering with causal advancement and deterministic tie-breaking. It is not an ordinary display timestamp or an opaque-key field. |
| **Envelope Cipher** | ChaCha20-Poly1305 content encryption with SyncKey before docs/blobs storage, in addition to transport encryption. |
| **Sync Blob Store** | Local Sync-owned encrypted blob infrastructure, not Media's canonical storage. Domain writes publish facts rather than precreating blob sidecars. |
| **Inbound Apply / Inbound cache refresh** | Sync decrypts/routes to the domain adapter; the adapter merges/writes canonical state and refreshes its live managers before success advances the cursor. Inbound application does not echo as a new local commit. |
| **Sync Sidecar Store / Inbound Inbox** | One rebuildable local SQLite store for durable received work, retry/idempotency, version cursors, and repair metadata. Canonical domain data remains outside this bookkeeping store. |
| **Repair Scan** | Bounded anti-entropy/recovery comparison, not the normal per-event path. Normal inbound work is incremental; “Sync now” performs one selected-peer cycle plus local repair/drain. |
| **First Sync** | Proposed bidirectional reconciliation of independently populated pre-pair state without an existing common merge base. Consult ADR-0041 for unresolved conformance gaps. |
| **Durable Conversation Fact / Stable Conversation Fact** | Coherent publishable history snapshot, possibly at a completed in-progress Model Call Boundary; stable facts are the terminal/stable subset. Neither transfers execution resources. |
| **Durable Publication Watermark** | Exact latest successfully published Conversation Manifest prefix. Unpublished local disk advances cannot become remote readiness requirements. |
| **History Watermark Reached** | `history_watermark_reached`: a specific advertised durable prefix is locally materialized. Does not imply terminal or active execution. |
| **History Refresh Required** | `history_refresh_required`: merge current local durable history after a gap/change, without blocking Live ingestion. Removed `history_hydrated` is not an alias. |
| **Live-Assisted Durable Hydration** | Foreground acceleration of already-published durable facts over Device Session through the normal inbound adapter seam. It does not introduce a second Conversation store. |
| **NetworkSecret Backup** | User-controlled secret export; no recovery service. Losing every copy prevents recovery of encrypted network data. |
| **Media Retention TTL / Media Soft Cap** | Configured creation-age expiry with replicated media deletion, versus a local size advisory. Access does not renew TTL; exceeding soft cap does not force eviction. |

## Presence, mobile power, and Sync scheduling

Details: [presence and power](docs/adr/0043-sync-presence-mobile-power-and-background-scheduling.md).

| Term | Meaning and boundary |
|---|---|
| **Sync Runtime** | Replication consumer/scheduler attached to Device Network Runtime. General network presence belongs to Device Network Runtime, not to a Sync Cycle or Sync-owned presence subsystem. |
| **Sync Cycle / Sync Trigger** | Bounded replication attempt and its coalesced scheduling cause, not ownership of general networking. |
| **Mobile Sync Power Policy / Sync Power Policy Config** | Device-local mode and normalized scheduling/residency settings; never replicated as user portable state. |
| **Sync Runtime Host Profile** | Explicit `mobile-apk`, `desktop`, or `headless` lifecycle model. Android background suspension rules do not silently apply to desktop/headless. |
| **Mobile Lifecycle Source** | Android shell's authoritative foreground/background events, not inferred paths, ports, process ancestry, or WebUI visibility. |
| **Foreground Operation Lease** | Eligible active execution/viewing/transfer claim retaining mobile residency. Approval-only wait and detached sessions do not by themselves keep networking awake. |
| **Background Flush Grace** | Short opportunity to publish local writes before background policy suspends networking. |
| **Device Network Suspension / Warm Suspend / Cold Suspend** | Intentional complete network unreachability after grace; warm retains process/store state, cold also tears it down. Neither silently changes Authority placement. |
| **Power Saver Sync Promise** | Background flush then no continuous remote pull/apply; foreground catches up. No separate push-wake path. |
| **Balanced Sync Promise** | Occasional system-scheduled background work, not continuous realtime networking. |
| **Foreground Sync Resume** | Rebind Device networking, attach Sync, repair/drain as needed, and trigger a cycle. Chat/Terminal observation resumes once networking is usable rather than waiting for Sync completion. |

## Security and capability disclosure

Details: [capability snapshot](docs/adr/0011-effective-capability-snapshot-and-tool-exposure.md),
[Tool Guard and approvals](docs/adr/0012-per-agent-tool-guard-and-process-wide-approval.md),
[plugin isolation](docs/adr/0014-plugin-process-and-protocol-isolation.md).

| Term | Meaning and boundary |
|---|---|
| **Tool Public Name** | Canonical model/config/approval/audit name, with no parallel runtime alias identity. |
| **Effective Capability Snapshot** | One immutable per-call available/valid/selected catalog driving both visible schemas and dispatch. Saving/previewing configuration does not connect MCP or discover tools. |
| **Tool Disclosure / Tool Exposure** | Presentation strategy and Direct/Deferred/Hidden model visibility, separate from enablement and permission. |
| **Core Tool Surface / Bundled Tool Surface** | Product-defined tool groups used for exposure classification; classification itself grants no permission. |
| **Disclosure Bridge** | Deferred search/describe/call surface. Unwrap to the real tool name/arguments before Guard, hooks, logging, and execution. |
| **Tool Guard** | Common allow/deny/needs-approval gate before real builtin/MCP side effects, using the claimed Chat/Turn authorization snapshot. |
| **Agent Security Profile / Trust Delegation** | Complete independent Agent default policy and delegated tool freedom. Defaults materialize into Chat authorization; delegation cannot escalate or re-read newer defaults mid-execution. |
| **Approval Owner** | Typed complete Chat Address or explicit `{agent_id, session_id}` Runtime Session owner. ACP protocol sessions are not fabricated Chats. |
| **Approval Request / Approval Service** | Per-call pending user decision coordinated by process-wide Approval Authority. Current approval blocks the side effect within the same active Turn; obsolete parked-call forced replay is not the contract. |
| **Builtin Rule Set** | Shipped policy template/reference, adopted explicitly rather than automatically merging updates into saved profiles. |

## Configuration persistence

Details: [Cell Store](docs/adr/0004-config-persistence-cell-store.md).

| Term | Meaning and boundary |
|---|---|
| **Cell / Cell Layout** | Typed logical configuration unit and its file/JSON-section location. Callers use the domain wrapper rather than addressing storage layout. |
| **Cell Store** | Layout-agnostic atomic persistence engine; domain validation and Sync publication remain outside it. |
| **Cells Wrapper** | Domain-typed owner of Cell layouts and handles, such as `AgentCells` or `ProviderCells`. |
| **mtime-tagged cache / Disk-first writes** | Disk-authoritative reads invalidate on changed mtime; successful atomic disk commit precedes cache update. Failed persistence leaves cached state unchanged. |

## Project resources and lifecycle

Details: [Project resources and organization](docs/adr/0019-project-scoped-chat-and-fixed-resource-binding.md),
[Chat deletion proposal](docs/adr/0046-chat-deletion-tombstones-and-storage-accounting.md).

| Term | Meaning and boundary |
|---|---|
| **Project / Project Descriptor / Project Catalog** | Synchronized logical context, portable listing metadata, and process-wide descriptor index. Directory content, Git state, and authoritative local paths are not synchronized Project metadata. |
| **Project Resource Binding** | Resource-Device-local nonempty directory collection, stable directory IDs, primary ID, and revision. A Project uses one immutable resource Device; attached directories do not merge executable configuration. |
| **Project Directory Picker** | Browses/creates an explicitly named child on that Device without overwrite or implicit ancestor creation. Android uses filesystem paths with native permissions, not SAF document URIs as bindings. |
| **Project Primary Directory / Project Chat Working Directory** | Default for new Chats versus directory ID frozen at first accepted Submit. Changing the primary does not move existing Chats; collaborators inherit the source choice. |
| **Project Resource Policy** | Working directory determines cwd; attached roots supply default writable scope. Reads may extend outside unless separately denied; outside writes require approval/permissive policy. |
| **Project Approval Reuse** | Chat-scoped approvals follow full Project Chat Address across Agent switches; Agent-scoped approvals follow Agent. Archive/Reopen preserves Chat reuse; deletion removes it with Chat-owned data. |
| **Project Sidebar Preferences** | User-owned pins/manual ordering independent of activity/descriptors, synchronized per preference record rather than replacing the whole sidebar. |
| **Project Availability** | Current resource-serving ability. Unavailable Projects/history remain visible and drafts editable; Submit reports/blocks on the specific resource reason without creating a deferred job. |
| **Project Chat Concurrency** | One Authority/Turn/queue per Chat, with no Project-wide execution lock. Separate working trees require distinct resource bindings/Projects. |
| **Project Chat Archive** | Reversible individual Chat listing state, independent of Project Archive. “Clear Archive” is permanent deletion. |
| **Archived Project / Project Archive Confirmation** | Project-wide stop of Turns, queues, waits, ACP work, and Tasks before reversible archive, with explicit stop counts when available. Preserve executor selection; revoked-Device exception follows ADR-0019. |
| **Project Reopen** | Online resource validation then explicit return to active state. Does not restart stopped work, queues, approvals, or Tasks. |
| **Project Delete** | Confirmed permanent removal of an already archived Project and Rua-owned Chat data. Never modifies/deletes the bound source directories or Git repository. |
| **Project Knowledge / Project Instructions / Project Skills** | Reviewable resource documents, selected-root `AGENTS.md`, and in-place `.agents/skills` discovery. Read on the resource Device; do not invent Project Memory or merge instructions from unrelated attached roots. |
| **Pinned Chat Delete Protection** | Recheck synchronized local pin state at every permanent-delete entry point. |
| **Project Chat Delete / Cross-Device Chat Deletion / Chat Delete Intent** | Proposed terminal replicated deletion by complete address, applicable locally without waiting for the resource Device. Deleting Chat-owned Rua state never deletes Project source files. |
| **Local Chat Deletion Apply / Chat Deletion Coordinator** | Proposed shared idempotent stop/cleanup after durable delete intent, with reference-safe Media handling. ADR-0046 tracks remaining gaps. |
| **Chat Storage Accounting** | Bounded attribution/reclaimability for one Chat, respecting shared references rather than treating every referenced byte as exclusively reclaimable. |

## User Inbox and notifications

Details: [best-effort notifications](docs/adr/0047-best-effort-device-notifications.md).

| Term | Meaning and boundary |
|---|---|
| **User Inbox** | One global durable collection explicitly addressed to the user, not partitioned by Agent. Distinct from Sync's internal Inbound Inbox. |
| **Inbox Message / Inbox Sender** | Uniform title/Markdown content and open sender identity (`kind`, optional ID, display snapshot). Producers choose delivery; technical Task/approval events do not automatically create messages or custom action cards. |
| **User Notification** | Explicit transient best-effort attention signal, independent of Inbox delivery. No durable queue, retry/history/unread guarantee, or effect on the producer's business result. |
| **Notification Sender / Rua URI** | Opaque display ID/name and optional `rua://` activation target. Resource domains own URI shapes; notification code does not interpret business ownership. |
| **Notification Adapter** | Exactly one target-Device presentation path. Explicit native-shell capability takes precedence; standalone browsers use browser presentation. One send capability routes local/remote Device delivery without Sync persistence. |

## Slash commands and MCP lifecycle

Details: [command dispatch](docs/adr/0017-slash-command-layered-dispatch-and-discovery.md),
[ACP Chat Mode](docs/adr/0033-acp-chat-mode-client-projection-and-permission-relay.md),
[per-Agent MCP isolation](docs/adr/0013-mcp-per-agent-manager-isolation.md).

| Term | Meaning and boundary |
|---|---|
| **Slash Command** | `/` input dispatched to a defined command path instead of ordinary Agent conversation. Distinct from an OS shell command. |
| **Daemon Command / Control Command / Conversation Command** | Process-level action, authority/runtime control, or Chat-scoped operation respectively, intercepted where its required context exists. |
| **ACP Chat Mode Controls** | Authority-routed ACP binding/control; ACP is a Conversation Driver, not a Task runner or alternate Chat authority. |
| **ACP Tool Projection** | Session/Tool-call-keyed accumulated runner activity with exact replacement/merge semantics and stable Rua identity. Structured snapshots are not appended as text deltas. |
| **ACP Permission Relay** | Transient runner-owned permission choices, preserving exact option IDs/order/meaning. Returns the selected ID unchanged and creates no Rua approval reuse rules. |
| **Configured ACP Runner Status** | Saved enabled/disabled state, not a readiness/handshake cache. Concrete operations establish readiness without background preheat or rewriting configuration status. |
| **Agent-Required** | Conversation-command metadata indicating required Agent context, not a separate command tier. |
| **Command Fast Path / Slash Command Dispatcher / Priority Registry** | Deterministic context-aware dispatch through executable registries; no duplicated frontend-only command semantics. |
| **Discovery API** | Available command descriptions generated from those registries. |
| **CLI Desired State** | Repeating satisfied enable/disable/update/uninstall succeeds with unchanged status; creation/import retains conflict protection unless replacement is explicit. |
| **MCP Installation / MCP Uninstall** | Register/enable or remove Rua-scoped transport configuration and owned credentials. Does not take ownership of external npm/Python/system packages. |
| **Shared MCP Catalog** | Reusable configuration store; explicit broadcast copies to an Agent before execution. Shared edits do not automatically broadcast or connect. |
| **MCP Config Import** | Batch creation from JSON/stdin with the same scope and name-conflict rules. |
| **MCP Secret Input** | Hidden input, stdin, environment reference, or OAuth; all CLI/API reads mask resolved secrets. |
| **MCP Login** | Explicit blocking URL-MCP authentication after registration, in Agent runtime scope. Installation does not implicitly launch OAuth; shared catalog has no runtime login. |

## Skills: discovery and installation

Details: [Skill bundle trust](docs/adr/0015-skill-bundle-install-sources-and-scan-trust.md).
Market/Hub service behavior belongs to `rua-skills-market` and `rua-skills-hub`;
CLI, WebUI, and Agent Tools are adapters over the same services.

| Term | Meaning and boundary |
|---|---|
| **Skill Market / Skill Hub** | Read-only cross-platform discovery versus source fetching/validated installation. Market may reuse Hub transport; Hub does not depend on Market search. |
| **Market Provider / Market Result** | External search implementation and normalized candidate with source URL/provenance, separate from installation-source handlers. |
| **Market Search Outcome** | Provider-grouped results plus partial errors. One failing source does not erase successes; no invented unified relevance or silent deduplication. |
| **Source URL** | Canonical connection between discovery and installation; Hub resolves it to a source handler. Clients do not maintain a second platform/slug conversion scheme. |
| **Install Target** | Selected Agent Workspace or global Skill Pool. Pool content is not Agent-enabled until distributed; in-flight work retains its captured Agent owner. |
| **CLI Stable Completion** | Installation blocks until installed/failed/cancelled, hiding internal asynchronous polling. A task ID alone is not successful completion. |
| **Skill Update / Install Meta** | Reload authoritative `_meta.json` source/version/content-hash provenance; reject overwriting local modifications unless explicitly forced. Local Skills without provenance have no guessed update source. |
| **Skill Management API** | Agent-path-owned Workspace operations and global pool operations. Changing frontend selection never retargets running installation, cancellation, or publication. |
| **Skill Uninstall / Skill Import** | Confirmed removal from selected scope versus validated local directory/ZIP import. Online installation accepts Source URL, not disguised local paths. |
| **Install Selection** | Explicit user choice of one candidate/source before install. A previously explicit source/selection already satisfies consent; search alone or model ranking does not. Scanning still gates installation. |
| **Skill Discovery Workflow** | Optional packaged `find-skills` guidance using shared search/install Tools, not shell-to-local-CLI recursion or a second platform client. Users may disable/uninstall it. |

## Test architecture

Details: [layered tests](docs/adr/0050-layered-test-architecture.md),
[release-gate proposal](docs/adr/0051-release-verification-gates.md).

| Term | Meaning and boundary |
|---|---|
| **Layered Test Suite / Theme Aggregator** | Crate-local tests grouped by logical layer and functional theme, with aggregator binaries instead of hundreds of one-file binaries. |
| **Unit Test / Contract Test** | Isolated behavior versus public boundary/DTO/protocol guarantees. |
| **Integration Test / E2E Test** | Collaborating modules/resources versus complete user-observable product flows on real entry points. |
| **Test Priority** | Risk- and behavior-driven verification scope. Use focused checks for iteration; acceptance evidence does not become a terminology definition. |
