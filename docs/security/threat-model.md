# Rua Threat Model

**Status:** accepted

This document records the security boundaries that must remain true for local,
mobile, plugin, tool, channel, and multi-device operation. It is intentionally
version controlled: architecture changes must update the relevant threat,
mitigation, owner, tests, and release gate in the same change.

## Trust boundaries

- The browser and Android WebView are untrusted API clients.
- The Desktop Shell WebView and any foreign browser origin remain untrusted;
  Tauri capability grants are command-specific and do not replace HTTP policy.
- Loopback does not imply authorization; host allowlists and tokens remain
  explicit policy.
- Agent, tool, MCP, plugin, skill, channel, and Device inputs are untrusted.
- Synced Devices authenticate as paired members, but business authority checks
  still apply to every command.
- Filesystem paths, archives, media URLs, and provider responses are untrusted.

## T01 - Loopback API exposure

**Threat:** A process or WebView on the local machine calls a loopback API that
was assumed to be private and bypasses authentication.

**Mitigation:** Authentication middleware protects API routes after bootstrap;
`allow_no_auth_hosts` is explicit, normalized, device-local policy rather than
an implicit loopback bypass.

**Owner:** `rua-server` auth and security endpoints.

**Tests / fixtures:** `crates/rua-server/tests/contract_auth_http/` and
`crates/rua-server/tests/contract_security_http/security_config.rs`.

**Release gate:** Rust contract tests plus
`scripts/tests/check-threat-model-coverage.sh`.

## T02 - WebView privilege confusion

**Threat:** The Android WebView is treated as trusted native code and gains
access to commands, terminal input, or local files without the same API checks
as another browser client.

**Mitigation:** WebView requests use the normal HTTP API, authority contracts,
input leases, and viewer identities. No JavaScript bridge bypasses those
boundaries.

**Owner:** `rua-server`, `console`, and `mobile/mobile-shell`.

**Tests / fixtures:** `mobile/mobile-shell/app/src/androidTest/kotlin/com/rua/mobile/`
and `console/tests/terminal-canary/xterm-compatibility.spec.ts`.

**Release gate:** Android native runtime E2E and Terminal compatibility canary.

## T03 - Token leakage

**Threat:** Provider, OAuth, MCP, channel, or device tokens leak through API
responses, diagnostics, logs, error messages, or synced configuration.

**Mitigation:** Secrets are encrypted at rest, masked as `***` at management
boundaries, omitted from diagnostics, and re-encrypted for each receiving
Device.

**Owner:** `rua-config`, `rua-mcp`, `rua-provider`, `rua-sync`, and
`rua-security`.

**Tests / fixtures:** `crates/rua-mcp/tests/store/secret_mask.rs`,
`crates/rua-mcp/tests/store/secret_field_io.rs`, and
`libs/llm-multiplex/tests/contract_provider_errors/http_error_mapping.rs`.

**Release gate:** Config round-trip, Rust contract tests, and threat-model gate.

## T04 - MCP command escalation

**Threat:** An MCP command executes outside the selected Agent's policy,
changes transport type unexpectedly, or reaches a private endpoint.

**Mitigation:** MCP configuration is Agent-scoped, secrets are masked, HTTP
transports enforce SSRF policy, and tool execution passes through the shared
security and approval pipeline.

**Owner:** `rua-mcp`, `rua-server`, and `rua-security`.

**Tests / fixtures:** `crates/rua-mcp/tests/protocol/http_protocol_compliance.rs`,
`crates/rua-mcp/tests/store/agent_mcp_cells_layout.rs`, and
`crates/rua-server/tests/contract_mcp_http/`.

**Release gate:** MCP contract tests and threat-model gate.

## T05 - Skill upload and package execution

**Threat:** A skill upload installs malformed, unsigned, oversized, or unsafe
content and later executes it as trusted code.

**Mitigation:** Skill bundles are normalized, scanned, size limited, installed
into controlled directories, and remain subject to Agent security policy.

**Owner:** `rua-skills`, `rua-skills-hub`, `rua-skills-scanner`, and
`rua-server`.

**Tests / fixtures:** `crates/rua-skills-scanner/tests/all/`,
`crates/rua-skills-hub/tests/all/bundle.rs`, and
`crates/rua-server/tests/contract_skills_http/`.

**Release gate:** Skills contract tests and threat-model gate.

## T06 - Archive traversal

**Threat:** A ZIP or plugin archive writes outside its destination through
absolute paths, `..`, symlinks, or crafted archive entries.

**Mitigation:** Archive entries are normalized and rejected unless they remain
under the selected destination; unsafe paths fail before extraction.

**Owner:** `rua-server` workspace/plugin installation boundaries.

**Tests / fixtures:** `crates/rua-server/tests/integration_workspace_files/zip_roundtrip.rs`
and `crates/rua-server/tests/plugin/plugin_install_errors.rs`.

**Release gate:** Workspace archive and plugin contract tests.

## T07 - Media download abuse

**Threat:** A channel-controlled media download reaches an unsafe URL, consumes
unbounded resources, disguises content, or writes an unsafe filename.

**Mitigation:** Media resolution validates schemes, status, size, content type,
local-file authority, traversal, and sanitized temporary filenames.

**Owner:** `rua-channel` core media boundary.

**Tests / fixtures:** `crates/rua-channel/tests/core_media/`,
`crates/rua-channel/tests/unit_dingtalk/media_download.rs`, and
`crates/rua-channel/tests/onebot_all/onebot_media_and_auth.rs`.

**Release gate:** Channel contract tests and threat-model gate.

## T08 - Filesystem path traversal

**Threat:** User, Agent, plugin, or synced input escapes an Agent workspace or
approved root through traversal, symlink, encoded, or non-canonical paths.

**Mitigation:** Paths are canonicalized against explicit roots; parent
traversal, foreign authorities, and unsafe archive paths are rejected before
read or write.

**Owner:** `rua-workspace`, `rua-tool`, `rua-server`, and `rua-security`.

**Tests / fixtures:** `crates/rua-workspace/tests/config_misc/canonicalize_under.rs`,
`crates/rua-workspace/tests/unit_workspace/agent_path_validation.rs`, and
`crates/rua-security/tests/path_overreach.rs`.

**Release gate:** Workspace, tool, and security contract tests.

## T09 - Secret redaction regression

**Threat:** A management GET, doctor report, provider error, or diagnostic
returns the original secret instead of a redacted value.

**Mitigation:** Public DTOs expose masked values or presence booleans; update
requests containing `***` preserve the stored secret.

**Owner:** `rua-config`, `rua-provider`, `rua-mcp`, and `rua-security`.

**Tests / fixtures:** `crates/rua-security/tests/all/redaction.rs`,
`crates/rua-mcp/tests/store/mcp_secret_hints.rs`, and
`scripts/tests/check-config-roundtrip.sh`.

**Release gate:** Config round-trip, security tests, and threat-model gate.

## T10 - Approval replay or scope confusion

**Threat:** An approval is replayed for another Agent, Chat, Turn, tool call, or
different arguments.

**Mitigation:** Approval Authority keys requests and reuse rules by explicit
Agent/Chat/tool identity and canonical arguments; hard deny rules cannot be
overridden by replay.

**Owner:** `rua-security` Approval Authority and `rua-server` approval API.

**Tests / fixtures:** `crates/rua-security/tests/all/approval_authority.rs`,
`crates/rua-server/tests/contract_approval_http/`, and
`crates/rua-server/tests/agent_approval/approval_card_roundtrip.rs`.

**Release gate:** Approval contract tests and threat-model gate.

## T11 - Cron-triggered tool execution

**Threat:** A cron job invokes a tool outside its owning Agent/Chat scope or
bypasses normal task, security, and approval boundaries.

**Mitigation:** Cron persists scoped dispatch targets, runs through the shared
task/runtime path, and does not own a privileged tool execution bypass.

**Owner:** `rua-cron`, `rua-workspace`, and `rua-server`.

**Tests / fixtures:** `crates/rua-server/tests/contract_cron_http/`,
`crates/rua-server/tests/infra/cron_loop_integration.rs`, and
`crates/rua-security/tests/all/structured_rules.rs`.

**Release gate:** Cron and security contract tests.

## T12 - IM spoofing

**Threat:** A group or direct-message sender spoofs a bot mention, user,
conversation, or approval callback and causes unauthorized Agent dispatch.

**Mitigation:** Channel adapters validate provider-native identities,
deduplicate message IDs, preserve group/DM scope, and apply mention and ACL
gates before dispatch.

**Owner:** `rua-channel` provider adapters.

**Tests / fixtures:** `crates/rua-channel/tests/core_pipeline/mention_gate_test.rs`,
`crates/rua-channel/tests/core_pipeline/acl_gate_test.rs`, and provider
channel contract tests.

**Release gate:** Channel contract tests and threat-model gate.

## T13 - Local network peer impersonation

**Threat:** A local network or Relay peer opens a targeted Device connection
without being the paired Device or a member of the same network.

**Mitigation:** Device Sessions authenticate the iroh identity, paired Device
identity, network membership, protocol version, and capabilities before
logical streams are accepted.

**Owner:** `rua-device-network` and `rua-sync`.

**Tests / fixtures:** `crates/rua-device-network/tests/contract_session/session_manager.rs`
and `crates/rua-sync/tests/contract_device_network/runtime_sync.rs`.

**Release gate:** Device Session contract tests, Android E2E, and threat-model
gate.

## T14 - Android exported component exposure

**Threat:** An Android exported Activity, Service, Receiver, or Provider accepts
untrusted external intents that start privileged runtime behavior.

**Mitigation:** Components are exported only when an intentional launcher,
share, or platform integration requires it; internal runtime and transfer
services remain non-exported and validate all forwarded data.

**Owner:** `mobile/mobile-shell` Android application.

**Tests / fixtures:** `mobile/mobile-shell/app/src/main/AndroidManifest.xml`,
`mobile/mobile-shell/app/src/test/kotlin/com/rua/mobile/`, and
`mobile/mobile-shell/app/src/androidTest/kotlin/com/rua/mobile/`.

**Release gate:** Android unit tests, APK packaging checks, Android E2E, and
threat-model gate.

## T15 - Desktop Host origin and native-bridge confusion

**Threat:** A foreign browser origin reaches the loopback Desktop Host API, or
Console/plugin JavaScript uses a broad Tauri capability to execute arbitrary
shell commands, read files, or stop a Runtime it does not own.

**Mitigation:** Desktop Host launches bind only to loopback, validate the Host
header, reject foreign Origin access to API/SSE/WebSocket and all mutating
requests before endpoint behavior, and preserve no-Origin native CLI clients.
Readiness and version are the only intentional cross-origin bootstrap reads.
Desktop shutdown requires a constant-time per-launch token and Tauri grants
only named commands to the main window and Runtime-hosted loopback origins.

**Owner:** `rua-server` Desktop Host middleware and `console/src-tauri`
capabilities/runtime supervisor.

**Tests / fixtures:** `crates/rua-server/tests/contract_startup_http/desktop_security.rs`,
`crates/rua-server/tests/contract_startup_http/desktop_shutdown.rs`, and
`console/src-tauri/tests/lifecycle_policy.rs`.

**Release gate:** Desktop Host contract tests, Tauri capability review, and
`scripts/tests/check-threat-model-coverage.sh`.

## T16 - Authority-routed Chat Media spoofing and unsafe reclamation

**Threat:** A Controller spoofs a target Device or stale Authority Epoch, opens
an unauthorized Bulk Transfer, escapes a Workspace through a Media id or
symlink, exhausts memory or disk with oversized/abandoned transfers, causes
orphan collection to delete newly referenced Media, or leaks filenames, paths,
content, or Device payloads through logs.

**Mitigation:** Chat Media uses UUID-only ids and complete Chat Addresses;
Authority Devices validate target Device and Authority Epoch before issuing a
short-lived, single-operation transfer authorization. Bulk transfer remains
bounded and resumable, partial files use hidden temporary paths, final writes
and MIME metadata are fsync/rename transactions, and reads reject symlinks.
Orphan collection operates only inside a resolved Workspace `media/` root,
requires a grace period, protects Stable Conversation State references, and
quarantines then rechecks before deletion. Logs contain operation ids, scope,
byte counts, timing, and outcomes but no original filename, absolute path,
attachment content, prompt, secret, or raw Device Session payload.

**Owner:** `rua-server` Chat Media service, Conversation Authority Router, and
shared Device Session Bulk Transfer; `rua-sync` Media adapter.

**Tests / fixtures:** `crates/rua-server/tests/contract_console_http/chat_media.rs`,
`crates/rua-server/tests/infra/sync_transfer.rs`,
`crates/rua-server/tests/infra/conversation_authority_target.rs`, and
`crates/rua-sync/tests/sync_foundation/owner_scoped_media_adapter.rs`.

**Release gate:** scoped Media contract tests, Device Session transfer tests,
owner-scoped Media Sync tests, and `scripts/tests/check-threat-model-coverage.sh`.


## T17 - Bundled automation owner confusion and uncertain input replay

**Threat:** Model arguments switch browser profiles or native sessions, bypass
leaf input approvals through a composite call, revive an input after Stop, or
mix a new Shell with old drivers during update.

**Mitigation:** Product adapters restrict the native tool surface and derive
session identities from the execution Device and complete Chat Address. The
existing Chat/Turn security snapshot and Tool Guard authorize real leaf actions.
A sequence holds device control; cancellation prevents subsequent submissions.
Browser acknowledgements are never retried, and uncertain native input blocks
its host generation until explicit restart. Cleanup uses owned process trees,
private owner pipes and namespaces. Pinned source/patch/archive checks, a
compiled release receipt, signed App validation and Windows PE image digests
bind the component set. Mutable profiles stay outside the installation.

**Owner:** `rua-mcp`, `rua-security`, `console/src-tauri`, Desktop release.

**Tests / fixtures:** `crates/rua-mcp/tests/integration_mcp/bundled_browser.rs`,
`bundled_computer.rs`, `crates/rua-agent/tests/runtime/react_config.rs`,
`console/src-tauri/tests/automation_host.rs`, `update_bundle.rs`, and
`scripts/tests/check-desktop-automation-staging.py`.

**Release gate:** focused ownership/replay tests, touched crate gate, and signed
platform installer acceptance in the release workflow.
Protocol fixtures do not validate real GUI delivery or system permission grants.
