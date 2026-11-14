# ADR 0058: Tool selection policies with default enablement and explicit restrictions

**Status:** Accepted

**Date:** 2026-11-14

**Implementation:** Implemented. Validation distinguishes policy/transport regression coverage from native platform acceptance below.

## Context

Rua currently persists an exact inventory of enabled Tools in Agent configuration
and Chat capability snapshots. New product Tools are registered at runtime, but
older Agents and Chats do not select them. The Agent configuration UI also lists
saved entries rather than the complete product catalog, so a newly shipped Tool
can be absent from both execution and the enablement surface.

A Phone Use investigation exposed the mismatch: `list_tools` advertised Phone
Tools from the full registry, while the Chat configuration excluded them. Direct
calls failed lookup and background Task submission rejected the same Tools.
Discovery and execution were describing different capability sets.

Phone Use is provided through native Android Tools; Browser Use and Computer Use
use bundled MCP sources. Their transports differ, but their configuration and
discovery must follow one selection contract. Ordinary assistants should acquire
new product Tools without repeated configuration backfill. Deliberately restricted
Agents, including the read-only research preset in ADR-0053, must retain an exact
Tool boundary across upgrades.

## Decision

### D1 - Default enablement and explicit restriction are two Tool Selection Policy modes

An ordinary newly created Agent uses **Default-Enabled** selection: eligible Tools
are selected unless explicitly disabled. Persist the disabled Tool identities and
disabled product capability identities, not an expanded inventory of every Tool
available when the Agent was created.

A deliberately constrained Agent uses **Restricted** selection: only explicitly
allowed Tool identities are selected. The read-only research preset remains in
this mode. A new Tool is not implicitly allowed, even if it is read-only or belongs
to an already configured MCP source. This is an ordinary configuration mode,
not a new Agent/executor type or a second security system.

The policy has an explicit mode. An empty disabled set in Default-Enabled selects
all eligible Tools; an empty allowed set in Restricted selects none. Missing,
null, malformed, or unsupported policy data is not an empty Default-Enabled
policy. Switching mode is an explicit, reviewable configuration change; do not
maintain conflicting allow/deny inventories for the same policy.

For an eligible Tool `t`:

- Default-Enabled selects `t` unless its identity or product capability is disabled.
- Restricted selects `t` only when its exact identity is allowed.

Eligibility, policy selection, disclosure, and execution authorization remain
separate checks. Neither selection mode grants execution permission.

### D2 - Product capability growth is distinct from external source enrollment

The default-eligible product catalog contains Rua-owned built-in Tools and trusted
bundled product sources, including Phone Use, Browser Use, and Computer Use.
Product ownership is established by trusted registration metadata, never by a
`phone_*`, `rua_*`, or `mcp__*` string prefix or a third-party source's claimed name.

Default-Enabled Agents and Chats can acquire newly shipped product Tools without
a new inventory entry. A disabled product capability remains disabled for all
its present and future Tools. An individual Tool disablement remains independent:
reenabling a capability does not erase its individual disabled entries.

Retain the existing adapters and owners:

| Capability | Execution integration | Selection identity |
|---|---|---|
| Phone Use | Native Android Tools and private authenticated bridge | Product capability plus canonical Tool identity |
| Browser Use | Per-Agent bundled MCP source | Trusted product source plus original Tool identity |
| Computer Use | Per-Agent bundled MCP source | Trusted product source plus original Tool identity |

A source-wide disablement excludes all its Tools before exposure and dispatch.
Product capability identity is stable configuration identity, not the UI's
presentation group or a list of names expanded at save time.

This decision does not automatically install, enroll, or enable arbitrary MCP
servers, Plugins, or Skills. An external source must first be explicitly configured
and selected under its existing contract. In Default-Enabled mode, newly discovered
Tools within an already selected external source follow its saved disabled-Tool
selection. Restricted mode additionally requires an exact allowed Tool identity.
Adding an external source to an Agent does not add it to an existing Chat without
an explicit Chat configuration update.

Match external Tools by source kind, stable source identity, and original Tool
name; built-ins use their canonical identity. Preserve disabled/allowed identities
that the current Device does not provide. Moving between Devices, source removal,
or temporary unavailability must not erase a restriction. Resolved paths, transport
credentials, handles, and live schemas remain device-local runtime data.

### D3 - Chat and Turn snapshots freeze policy, not the product catalog

Chat materialization copies the Agent's complete Tool Selection Policy, external
source selections, and existing per-Tool behavioral settings into its immutable
Chat Configuration Snapshot. Branching copies the source Chat policy. Project
Agent switching and collaboration creation retain ADR-0028/0053's contribution
and publication rules.

Later Agent policy edits do not alter existing Chats. Explicit Chat edits publish
a new Configuration Epoch; an Active Turn retains the policy and authorization
captured at claim time. Selecting Default-Enabled is a complete policy, not a
sparse override that rereads the current Agent template.

At each Model Call Boundary, resolve that frozen policy against the current
product catalog, selected external sources, execution Device capabilities, and
runtime readiness. A new product Tool or restored driver may therefore enter the
next Effective Capability Snapshot without changing the saved policy or Chat
epoch. Catalog/availability changes must invalidate prepared capability caches.

This catalog growth is intentional in Default-Enabled mode. It is not adoption
of new Agent defaults. In Restricted mode, catalog growth alone cannot select a
new Tool. Each model response's Tool Calls finish against their captured snapshot;
do not replace an in-flight executor with one from a newer generation. Catalog
growth does not relax ADR-0054's driver-update admission or permit replacing an
existing Tool contract mid-Turn.

### D4 - Discovery and dispatch share one resolved snapshot

Generate direct schemas, `list_tools`, Deferred search/describe/call, and execution
lookup from the same post-selection Effective Capability Snapshot. Remove the
full-registry summary captured inside `ListToolsTool` at registry construction.

`list_tools` advertises only currently selected and model-discoverable Tools.
Deferred entries use the established schema-discovery/call path. Hidden and
unavailable entries are not advertised as available; structured diagnostics may
explain an excluded capability without making it callable.

Background Tool submission uses the submitting invocation's resolved selection
and authorization, rather than a wider process registry. Direct calls, generic
dispatch, and Task submission cannot bypass either policy mode.

A captured snapshot does not guarantee that Android permissions, OS permissions,
or a transport remain valid until the side effect. Execution retains the existing
last-moment permission, cancellation, target, and lifecycle checks. A driver or
permission failure reports its actual reason and never triggers fallback through
Shell, a different Device, or an alternate invocation route.

### D5 - Configuration surfaces show catalog, policy, and readiness separately

The Agent Tool UI projects the complete configurable product catalog together
with saved policy, including Tools absent from old per-Tool entries. It supports
enabling a newly shipped Tool without hand-editing Agent JSON. Descriptions,
schemas, and presentation groups come from the catalog; per-Tool behavioral
settings remain user configuration.

Show selected/disabled/not-allowed separately from execution-Device support,
driver/bridge readiness, and required system or model capability. For example,
an enabled screenshot Tool may still require a model that accepts image input.
A status/permission diagnostic must remain explainable when an action Tool is
unavailable.

Configuration reads and static policy validation do not write defaults, connect
MCP sources, probe Plugins, or wait for live discovery. Existing explicit readiness
and permission actions remain owned by the execution Device. A desktop viewer
does not turn a phone-executed Chat into desktop execution.

### D6 - Preserve security, delegation, and Task boundaries

Both policy modes retain ADR-0012's canonical-name Tool Guard, File Guard,
Approval, and hard-deny precedence. Phone accessibility and camera authorization
remain separate; browser/desktop permissions and visible-image projection retain
ADR-0054/0057's requirements.

Default enablement intentionally expands the selectable surface across product
upgrades. A security policy that authorizes broad Tool categories may consequently
permit a newly added Tool. Use Restricted mode when the selected Tool identities
must remain exact. Restricted selection does not freeze Tool implementations or
guarantee that their side effects remain unchanged across upgrades.

For collaboration, the executor contributes its Tool Selection Policy and the
source execution contributes authorization/environment under ADR-0053. Do not
merge the source and executor Security Profiles or assume the executor's security
defaults protect a read-only preset. Its Restricted selection continues to exclude
writable and unknown Tools even when the source could authorize them. New Agent
defaults or direct human input cannot change that saved selection implicitly.

A Task Attempt retains its submitting configuration/policy and effective
authorization evidence. Admission resolves the requested Tool under that policy;
execution does not acquire extra Tools from a worker's registry. Retry/new Attempt
follows existing fresh-admission rules using the applicable Chat configuration,
not the latest Agent defaults. A catalog update neither replays a Task nor changes
a running Attempt's captured executor.

### D7 - One-time data cutover; no backward compatibility

Migrate the existing development data once, outside application execution, before
starting the new build. Back up the affected Agent documents and Chat snapshots;
quiesce writers and preserve source enrollment, per-Tool behavior, secrets,
unknown extension fields, and portable/off-device identities.

Ordinary Agents and their Chats adopt Default-Enabled, retaining known individual
disables. Deliberately constrained Agents and their Chats retain Restricted with
their exact allowed identities. Move any per-source per-Tool restrictions into
the typed policy during this one-time operation. Upgrade all persisted Chat
configuration snapshots, including those referenced by Tasks, without breaking
epoch references. Publish the migrated canonical data through the existing Sync
contract; devices must use the new build at cutover.

The product supports only the new policy format. Do not add an old-Agent/old-Chat
conversion UI, conversion endpoint, runtime migration, old-inventory interpreter,
or automatic fallback. A missing or malformed policy is invalid configuration.
Normal configuration updates edit a complete current-format policy; they are not
schema migration operations. One-time migration tooling is an operational artifact,
not a permanent application feature.

## Consequences and rejected alternatives

Default-Enabled removes repetitive per-Agent/per-Chat backfill for product Tool
releases, at the cost of an intentionally growing selectable surface. Restricted
retains exact boundaries for research and other constrained Agents. Two explicit
modes express these different needs without inventing a separate read-only runtime.

Rejected alternatives:

- Fixed expanded inventories for every Agent and Chat: new product Tools remain
  absent indefinitely and require repeated backfill.
- A blacklist for every Agent: silently broadens the read-only preset.
- Automatically enroll all external sources: confuses product delivery with user
  configuration and source trust.
- Default-enable only at Agent level while freezing names in Chat: preserves the
  original problem for existing Chats.
- Fill missing entries during reads/startup: obscures user intent and mutates
  configuration outside its authority.
- Special dispatch bypasses for Phone/Browser/Computer: repeats the mismatch and
  weakens existing execution boundaries.

## Implementation and verification

The implementation stores an explicit `tools.selection_policy`, with typed Tool
identities and stable capability disablements. Ordinary Agent creation uses
Default-Enabled; the research preset uses Restricted. Missing policy remains an
invalid configuration. Normal Agent policy edits commit against a whole-document
revision using atomic compare-and-exchange, retaining unknown fields and secrets.
Source enrollment remains independent. Existing data is migrated once outside the
application; no old-format reader or conversion product flow is retained.

Chat capture freezes policy, per-Tool behavioral settings, external source
selection, and portable Project directory identity. Explicit complete-configuration
preview/apply publishes a new epoch. Model Call Boundaries and Tool Tasks resolve
that saved configuration against the current eligible catalog. Trusted registry
namespace and bundled transport metadata establish product membership.

Direct/Deferred exposure, `list_tools`, dispatch, and Task submission use the
selected toolkit. Raw Agent-scoped MCP calls and Computer sequence leaf actions
also check the policy. Phone readiness is evaluated at capture while native
execution retains its permission and lifecycle checks. Configuration UI provides
mode changes, capability disablements, and explicit Chat
updates. Opening MCP settings does not automatically discover live tools.

Regression coverage includes policy growth and exact restriction, typed external
identities, off-device retention, atomic policy-edit conflicts, invalid data rejection,
Chat epoch isolation, real Tool Tasks, Project directory capture, and Computer
sequence leaf restrictions. The Android native Agent acceptance test exercises
real test-App observations/clicks, stale references, termination, Stop, and
accessibility revocation. Its HTTP fixture resolves the port published by the
private bridge rather than assuming a fixed listener.

Browser/Computer policy, bridge, and composite-control regression tests do not by
themselves establish new native page/window acceptance. Screenshot/photo model
input and the remaining cross-device/platform cases retain the acceptance
requirements below.

Verification must cover:

1. A persisted Default-Enabled Agent and Chat discover a new product
   Tool without backfill; a Restricted Agent/Chat does not.
2. Individual and whole-capability disables survive catalog growth, restart,
   source disappearance/reappearance, and cross-device Sync.
3. Agent policy edits do not change saved Chat policy; Chat edits affect later
   claimed Turns; catalog refresh cannot replace an in-flight executor.
4. Both Chat scopes, branches, Project Agent switching, collaboration, direct
   human input, Task admission, and Retry preserve policy/authorization ownership.
5. Direct schemas, `list_tools`, Deferred discovery/call, dispatch, and Task
   admission agree on inclusion/exclusion. Disabled calls produce no side effects.
6. Configuration reads perform no write or live discovery. One-time cutover
   preserves explicit disables and Restricted boundaries; invalid data never
   defaults open. Concurrent policy edits/Turn claim preserve epochs.
7. Native Phone Use and bundled Browser/Computer work through the ordinary
   Agent path on their execution Device. Verify actual test-App/window/page
   effects, image input, missing permissions, cancellation, and unavailable
   drivers; mocks alone do not establish platform acceptance.

## Related decisions

- [ADR-0011](0011-effective-capability-snapshot-and-tool-exposure.md): effective
  snapshots and Direct/Deferred/Hidden exposure.
- [ADR-0012](0012-per-agent-tool-guard-and-process-wide-approval.md): authorization
  and Approval ownership.
- [ADR-0013](0013-mcp-per-agent-manager-isolation.md): per-Agent MCP managers.
- [ADR-0028](0028-chat-running-config-and-turn-stable-snapshot.md): Chat/Turn
  configuration, amended to freeze Tool policy instead of expanded inventories.
- [ADR-0053](0053-collaboration-chats-and-background-tool-tasks.md): Restricted
  read-only preset and inherited execution/authorization.
- [ADR-0054](0054-bundled-browser-and-computer-use.md): bundled desktop capabilities.
- [ADR-0057](0057-phone-use-native-capabilities-and-private-execution.md): native
  Phone Use and private execution.
