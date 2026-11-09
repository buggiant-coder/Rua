# AGENTS.md — rua-rust

High-signal guidance for OpenCode sessions. Every line answers "would an agent get this wrong without help?"

> **Reviewed:** 2026-09-02.

This is the single repository instruction file for agents. Do not add nested
`AGENTS.md` files under `crates/`, `libs/`, `console/`, or `mobile/`; keep
scope-specific guidance here or in ordinary docs linked from here.

## Build & verify

```bash
scripts/tests/clippy-touched.sh              # clippy only on changed crates
scripts/tests/clippy-touched.sh --test        # + nextest on changed crates
scripts/tests/clippy-touched.sh --deps        # + reverse dependencies
scripts/tests/clippy-touched.sh main --test   # diff vs main (PR-level check)
```

**Never** run `cargo clippy --workspace` or `cargo test --workspace` locally — 50 workspace members, cold build ~10min. Use `clippy-touched.sh` which maps `git diff` → affected crates → runs only those. Uses `cargo nextest` for touched crates and libtest for `rua-server`'s heavyweight per-test fixtures (required: `cargo install cargo-nextest --locked`; profile in `.config/nextest.toml`). Falls back to `cargo test` if nextest missing. If Git metadata or a comparison baseline is unavailable, the script checks all workspace members and the standalone Desktop crate; this is the explicit exception to focused local checks for source archives.

Verification scripts live under `scripts/tests/`; do not use stale root-level paths such as `scripts/clippy-touched.sh` or `scripts/release-gate.sh`.

For each individual issue, run only focused tests that directly cover the
changed behavior, plus `scripts/tests/clippy-touched.sh` for Clippy on affected
crates. Do not run the full touched test suite after every issue. Do not use
`cargo test -p <crate>` as the default verification command: select the test
binary with `--test`, then narrow further by module path or exact test name when
possible. Expand to a complete theme binary or `clippy-touched.sh --test` only
after focused tests pass and the change's risk justifies the wider run.

At the end of the whole PRD, run
`scripts/tests/clippy-touched.sh main --test`, the combined PRD E2E acceptance,
and `scripts/tests/release-gate.sh` before the final push (fmt + clippy + test +
feature-flags + config-roundtrip + threat-model).

Run Nextest and other long verification commands without allocating a PTY.
Interactive in-place progress rendering can disappear from agent tool output;
plain piped output must preserve per-test status, pass/fail counts, and the final
summary for the user.

When a long verification command is already running in the background, do not
interrupt it just because the user asks a follow-up question or wants analysis
in parallel. Leave it running unless the user explicitly asks to stop it, it is
blocking the next required edit through a lock, or it is clearly doing
destructive/unwanted work. Poll it later and report the result.

For desktop + Android emulator E2E, build debug artifacts with
`cargo build -p rua-cli` and `scripts/package-mobile-apk.sh --debug`.
Start the server with `target/debug/rua server start`; install the APK with
`adb -s <serial> install -r target/mobile-apk/rua-debug-arm64-v8a.apk`. Do not build
`release-android`/thin-LTO artifacts unless the test specifically validates a
release build. Start the desktop build and APK build concurrently because they
are independent; building them serially adds several minutes to every E2E run.

Build uses `sccache` (configured in `.cargo/config.toml`).

Worktree dev: keep Rust `target/` local to each worktree; shared Cargo metadata
and incremental artifacts can interfere across branches. `console/node_modules`
may symlink to main only when `console/package.json` and `package-lock.json`
match; otherwise use local dependencies installed with `npm ci`. Initialize
CodeGraph in each worktree with `codegraph init` when the CLI is available.

CI (`.github/workflows/release.yml`): fmt → clippy → check → test on PR/push (Rust 1.91). Artifact builds on `v*` tags. Release-gate runs on PRs and main pushes.

## Workspace architecture

```
libs/       11 workspace libraries (no binaries, foundational types/traits)
crates/     36 application crates (business logic + binaries)
vendor/     2 vendored workspace crates
console/    WebUI frontend (Vite + React + Playwright)
mobile/     Android APK host, native runtime packaging, and device validation
android/    Android-specific build outputs (placeholder)
python_sdk/ rua_plugin_sdk — Python SDK for plugin authors
```

**Entrypoints**: `rua-server` (Axum HTTP, binary `rua-server`), `rua-cli` (binary `rua`), `rua-pack-tool` (binary, signs Pack zips).

**Dependency direction**: `crates/` → `libs/`, never reverse. Foundation libs (top of dep graph): [`rua-core`](libs/rua-core/) (types + runtime), [`rua-contract`](libs/rua-contract/) (frozen trait registry), [`agent-types`](libs/agent-types/) (LLM types).

## WHERE TO LOOK

| Task | Location | Notes |
|------|----------|-------|
| HTTP route, SSE, middleware | [`crates/rua-server/`](crates/rua-server/) | 51 endpoint modules in `src/endpoints/` |
| `rua` CLI subcommand | [`crates/rua-cli/`](crates/rua-cli/) | one file per subcommand |
| Agent runtime / ReAct loop | [`crates/rua-agent/`](crates/rua-agent/) + [`crates/react-agent/`](crates/react-agent/) | agent = orchestration, react = ReAct loop/runtime adapter |
| Tool implementation | [`crates/rua-tool/`](crates/rua-tool/) | core in `src/`, extra in `src/extended/`, builtins in `src/builtin/` |
| Channel (DingTalk, Telegram, …) | [`crates/rua-channel/`](crates/rua-channel/) | shared domain plus feature-gated platform modules |
| Config schema / migration | [`crates/rua-config/`](crates/rua-config/) | round-trip enforced by `scripts/tests/check-config-roundtrip.sh` |
| Multi-device sync | [`crates/rua-sync/`](crates/rua-sync/) | encrypted operations + HLC and domain publication; ADR-0038/0039 |
| Tool Guard / approval | [`crates/rua-security/`](crates/rua-security/) | per-agent policy, process-wide approval; ADR-0012 |
| MCP client | [`crates/rua-mcp/`](crates/rua-mcp/) (impl) + [`libs/mcp-manager/`](libs/mcp-manager/) (traits) | |
| Memory / vector store | [`crates/rua-memory/`](crates/rua-memory/) (logic) + [`libs/sqlite-vecstore/`](libs/sqlite-vecstore/) (storage) | |
| LLM provider | [`crates/rua-provider/`](crates/rua-provider/) + [`libs/llm-multiplex/`](libs/llm-multiplex/) | |
| Workspace runtime / lifecycle | [`crates/rua-workspace/`](crates/rua-workspace/) | wires every subsystem |
| Console Chat / UI runtime | [`console/src/pages/Chat/owned/`](console/src/pages/Chat/owned/) + [`console/src/ui/`](console/src/ui/) | scoped Conversation View Runtime and Rua-owned UI; ADR-0052 |
| Plugin process/protocol isolation | [`crates/rua-plugin-runtime/`](crates/rua-plugin-runtime/) (runtime) + [`crates/rua-plugin/`](crates/rua-plugin/) (install) | subprocess + JSON-RPC over stdio; no OS sandbox |
| Skills | [`crates/rua-skills/`](crates/rua-skills/) (core) + `rua-skills-scanner` (security) + `rua-skills-hub` (download) | |
| Frozen trait surface | [`libs/rua-contract/`](libs/rua-contract/) | RFC required to modify |
| Domain vocabulary | [`CONTEXT.md`](CONTEXT.md) | single source of truth |
| Architecture decisions | [`docs/adr/`](docs/adr/) | ordered current decision set; start with `README.md` |

## CODE MAP — top symbols by reuse

| Symbol | Crate | Role |
|--------|-------|------|
| `RuaError` / `Result` | `libs/rua-core::error` | only error crossing public API boundaries |
| `AgentRuntime` | `libs/rua-contract::agent` | top trait, impl in `rua-agent` & consumed by `rua-server`/`rua-workspace` |
| `Tool` | `libs/rua-contract::tool` | impls in `rua-tool` & MCP client |
| `Channel` / `Process` / `ChannelManager` | `crates/rua-channel::core` | shared contracts and composition for feature-gated platform modules |
| `TaskRuntime` | `crates/rua-task` | canonical durable Task lifecycle, Runner registry, control, Watch, and Retry |
| `ShutdownController` | `libs/rua-core::runtime` | process shutdown phases and bounded subsystem cleanup |
| `ResolvedAgent` | `crates/rua-agent::registry` | `(AgentProfile, Arc<AgentsRunningConfig>)` — **never split into two args** (ADR-0005) |
| `ToolGuardEngine` / `ApprovalService` | `crates/rua-security` | per-agent gate, process-wide approval map |
| `SyncEvent` / `OpKind` / `HLC` | `crates/rua-sync` | event-sourced sync log (one immutable iroh-docs entry per event) |

## Test organization (critical — easy to get wrong)

**No `#[cfg(test)] mod tests` inline in `src/`.** All tests live in `tests/` directories.

**Layered theme aggregation** — tests follow ADR-0050's logical layers (`unit` / `contract` / `integration` / `e2e`) while staying crate-local to avoid hundreds of one-file test binaries:

```
crates/<X>/tests/
├── unit_<theme>.rs         # aggregator: #[path = "unit_<theme>/file.rs"] mod file;
├── unit_<theme>/           # actual test files under this layer/theme
├── contract_<surface>.rs
├── contract_<surface>/
├── integration_<flow>.rs
├── integration_<flow>/
├── ...
└── fixtures/               # shared data
```

Cargo.toml template:
```toml
[package]
autotests = false

[[test]]
name = "endpoints"
path = "tests/endpoints.rs"
```

Themes are by **logical layer + functional area** (`unit_runtime`, `contract_channels`, `integration_endpoints`, etc.), not by file count or alphabet. Existing old themes (`endpoints`, `infra`, etc.) can migrate gradually when touched. See ADR-0050 for the full rule.

Run: `cargo test -p rua-server --test endpoints` (theme-level),
`cargo test -p rua-server --test endpoints health_endpoint` (filtered within
that theme), or `cargo test -p rua-server --test contract_chats_http
chats_lifecycle::agent_scoped_put_get_delete_repeat_delete_404 -- --exact`
(single test). Avoid unfiltered `cargo test -p <crate>` during normal iteration.

## Coding conventions

- `cargo clippy` and `cargo build` must produce 0 warnings
- All file writes must be atomic: temp → fsync → rename → fsync parent
- All async tasks accept `CancellationToken`; stop must cancel the whole subtree
- serde: preserve unknown fields (`#[serde(flatten)]`), **never** `deny_unknown_fields`
- Secrets: GET returns `***`, PUT/PATCH with `***` preserves old value

### API contract

The Rust product contract is authoritative. Use `CONTEXT.md`, `docs/adr/`,
route tests, console API types, and endpoint DTOs as the current source of
truth:
- Prefer canonical agent-scoped API paths, e.g. `/api/agents/{agentId}/chats/{chatId}/console/chat`.
- Keep DTO shapes stable at the Rust HTTP boundary.
- Build stream/chunked output through the established server/channel helpers.
- Error bodies use the server-standard `{"detail": "..."}` shape.

Root compatibility routes are retained only when documented in
`crates/rua-server/src/route_compat.rs` with a retention reason and exit
condition.

### Fix strategy

**No minimum-change patches.** If implementation contradicts the current Rust
domain model, ADRs, or public contracts, refactor toward the current Rust
architecture instead of patching around the wrong shape.

## Feature flags

Project-wide contract (in workspace `Cargo.toml` comment, defined per-crate):
`mobile-default`, `mobile-full`, `mobile-dev`, `local-inference`, `external-im`, `browser-tools`, `office-tools`, `wasm-runtime`, `legacy-python-tools`

`mobile-default` must not pull heavy deps (no llama-cpp, candle, wasmtime, chromiumoxide, embedded Python runtime). Enforced by `scripts/tests/check-feature-flags.sh`.

## Android / mobile

The product is pre-release. Implement only the current native APK + PRoot Debian
architecture; do not add old-APK, Termux-prefix, or Ubuntu migration adapters or fixtures.

Cross-compile targets: `aarch64-linux-android`, `x86_64-linux-android`. Custom linkers in `.cargo/config.toml`. Release profile `release-android` (thin LTO, panic=abort, opt-level=z).

The Android resident server is linked into the unified Android ELF `rua`. The APK directly launches it from the installer-managed `nativeLibraryDir` with `server start --profile mobile-apk`, as a separate process. The APK owns foreground service, lifecycle, and the authenticated native Phone Use bridge. Resolve installed executable paths again after APK updates; keep native bridge credentials out of tool subprocesses.

The APK carries Rua/WebUI, the terminal bridge, PRoot/loader, and required native dependencies. Debian rootfs and Python/Node toolchains are automatically prepared on first APK launch from verified official/mirror artifacts under the Mobile Runtime Manifest, with offline import and atomic activation. Select the lowest-latency eligible HTTPS metadata source separately for Debian and Node; do not require source selection or an install click. Preserve explicit cancellation and never reinstall a usable environment on ordinary launch. Keep workspace/configuration/user files independent of rootfs and preserve user-installed dependencies on updates. Full Linux environment construction is not a prerequisite for APK packaging.

All Android Agent-managed subprocesses and interactive terminal shells use the Linux Compatibility Runtime through Runtime Authority. Native Rua owns terminal PTYs; the Node terminal-state bridge runs in the guest. Missing guest runtime/toolchains fail the affected operation with diagnostics, without host fallback or blocking unrelated resident features. PRoot provides compatibility, not a security sandbox. See ADR-0002/0003/0034 for the authoritative architecture; acceptance records establish implementation coverage.

## Domain vocabulary

Terms are canonicalized in `CONTEXT.md` (single source of truth for terminology). Key gotchas:
- "Native Android Runtime" = Android ELF resident directly launched by the APK.
- "Linux Compatibility Runtime" = PRoot-managed Debian environment for external tools and terminal shells.
- "APK Native Payload" and "Mobile Runtime Manifest" separate application releases from Linux environment acquisition.
- "APK" = Android package. Never bare "package".

See `CONTEXT.md` for multi-device sync terminology (Master Identity, Device List, Envelope Cipher, etc.).

## Reference files

- `CONTEXT.md` — canonical domain terminology
- `docs/adr/` — current architecture decisions grouped in `README.md`; ADR-0002/0003 define the Android runtime and external-process boundary
