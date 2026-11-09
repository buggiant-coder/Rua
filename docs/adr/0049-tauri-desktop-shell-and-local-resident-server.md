# ADR-0049: Tauri Desktop Shell and local resident server

> **Status:** Accepted

## Context

macOS and Windows users need a native application lifecycle, tray integration,
notifications, updater, and a packaged Console. Moving product behavior into
Tauri commands would create a desktop-only backend beside the existing Rust
server and HTTP contract. Embedding all server logic in the WebView process
would also weaken process ownership, recovery, CLI parity, and release testing.

The desktop package must therefore remain a shell around the same resident
Rua Runtime used by other hosts, while still owning native installation and
update responsibilities.

## Decision

### D1 - Tauri is the Desktop Shell

The Tauri application is distributed for macOS and Windows. It owns the native
window, system tray, desktop preferences, notification integration, updater,
single-owner reconciliation, and lifecycle of the local resident process.

It embeds the built Console in the system WebView. It does not own Agent,
Conversation, Channel, Cron, Sync, Workspace, or security business behavior.

### D2 - The resident Runtime is an external process

Desktop Shell launches the packaged unified `rua` executable through
`rua server start --profile desktop`. The server binds the local HTTP
interface and serves the Console/API contract.

The bundle does not include or depend on a separate desktop-only
`rua-server` executable. Process discovery and shutdown use explicit
ownership metadata rather than killing arbitrary matching processes.

### D3 - HTTP remains the application boundary

Console calls the same resident-server HTTP routes in Tauri and browser-hosted
deployments. Tauri commands are limited to native capabilities such as process
lifecycle, window/tray behavior and notification presentation. Browser Use,
Computer Use, system permission requests, and CLI installation belong to the
resident Runtime and are exposed through HTTP in every viewer. App update and
legacy app migration actions also use HTTP; the resident can coordinate the
installed Shell through a private authenticated loopback transport. These
maintenance actions have no Tauri JavaScript command surface.

Desktop-only command APIs must not become alternate implementations of domain
CRUD or Conversation control.

### D4 - Shell lifecycle is bounded and recoverable

Startup establishes one owned resident server, waits for readiness, and opens
the Console only against that instance. Exit and update paths stop the owned
runtime with bounded waiting. Failed update installation restarts the previous
runtime when possible instead of leaving the shell permanently disconnected.

Stale ownership metadata is reconciled conservatively; another valid owner is
not terminated merely because a port or process name matches.

### D5 - Packaging keeps one Rua runtime entrypoint

The packaged `rua` executable supplies both resident-server and command-line
surfaces. Windows installation exposes the installer-owned path entry and
removes only that entry on uninstall. macOS offers an explicit install-command-
line-tool action rather than silently editing shell configuration when an app
bundle is copied. The Runtime installs a user-owned link to the packaged
`rua` executable, preserving sibling driver discovery and whole-app updates.

External Python, Node, Git, ffmpeg, and similar Agent toolchains remain
device-local dependencies discovered by Runtime Authority; they are not part of
the Desktop Shell contract.

[ADR-0054](0054-bundled-browser-and-computer-use.md) adds two explicit product
dependencies: bundled `agent-browser` and Cua Driver programs. They execute in
separate processes and are distributed with the app; the system Chromium
browser remains a device-local dependency. The resident Runtime owns driver
lifecycle, native permission probes and Agent behavior. macOS permission
attribution follows the actual process responsibility chain, whether Rua was
started by its Shell or independently; no Tauri viewer is required.

### D6 - Distribution is direct and signed

Desktop artifacts and signed updater metadata are published through GitHub
Releases. Supported bundles are macOS application/DMG artifacts for Apple
architectures and a Windows current-user NSIS installer.

Mac App Store and Microsoft Store constraints are outside the product boundary.
Release metadata, versions, embedded Console resources, and sidecar binaries
must be produced atomically from one release version.

### D7 - Linux remains CLI/headless

Linux supports the native CLI/server deployment and browser Console. Rua does
not promise a packaged Tauri Linux shell, AppImage, deb, or rpm. This avoids
making WebKitGTK and distribution-specific installer behavior part of the
desktop product contract.

### D8 - Native bridges are capability boundaries

The shell exposes only explicit native capabilities required by the Console,
such as local notifications and lifecycle state. Browser fallback remains
available outside a native shell. Detection uses Tauri capability/runtime APIs,
not user-agent heuristics.

## Hard invariants

- Tauri remains a shell, not a second business backend.
- The packaged runtime entrypoint is the unified `rua` executable.
- Desktop profile is explicit at server launch.
- Console uses the resident HTTP contract for product behavior.
- Exactly one shell owner manages its resident process.
- Update installation stops the owned runtime before replacing files.
- Windows uninstall removes only installer-owned PATH state.
- macOS does not silently modify shell configuration.
- Linux has no promised packaged Desktop Shell.
- General-purpose external Agent toolchains are not bundled as shell internals;
  the two product drivers follow ADR-0054.

## Authority, persistence, and failure boundaries

Desktop Shell owns its window lifecycle, packaged app updater and viewer
bridges. The resident server owns application state, automation driver hosts,
CLI installation and HTTP behavior.
Domain stores remain under the normal Rua data layout. The Console is a
client and does not gain filesystem authority merely because it runs in Tauri.

Server startup failure is surfaced as a shell/bootstrap failure with
diagnostics. Shell exit affects process availability, not durable business
state. Update failure must not silently claim success or delete user data.
Missing external toolchains degrade their dependent features without preventing
the core desktop application from starting.

## Consequences

- Desktop and browser Console behavior share one backend contract.
- Native packaging does not fork Agent or Conversation architecture.
- Runtime lifecycle and updates can be tested independently of domain logic.
- Direct distribution requires signing, manifest, installer, and recovery
  verification.
- Linux remains simpler and aligned with its existing deployment model.

## Rejected alternatives

- Implement domain behavior as Tauri commands.
- Ship a separate desktop-only server binary.
- Run the resident server inside the WebView frontend.
- Kill processes by name or port without ownership proof.
- Silently modify macOS shell startup files.
- Bundle every external Agent toolchain.
- Promise Linux Tauri packages across distributions.
- Depend on app-store sandbox and update policy.

## Implementation and verification

- `console/src-tauri/src/runtime.rs`
- `console/src-tauri/src/owner.rs`
- `console/src-tauri/src/tray.rs`
- `console/src-tauri/src/update.rs`
- `console/src-tauri/src/desktop_install.rs` (legacy app migration only)
- `console/src-tauri/src/control.rs` (private app updater transport)
- `crates/rua-server/src/desktop_automation.rs`
- `crates/rua-server/src/endpoints/desktop_control.rs`
- `libs/rua-core/src/desktop_cli.rs`
- `console/src-tauri/src/native.rs`
- `console/src-tauri/tauri.conf.json`
- `console/src-tauri/tests/owner_reconciliation.rs`
- `console/src-tauri/tests/lifecycle_policy.rs`
- `console/src-tauri/tests/desktop_install_policy.rs`
- `console/src-tauri/tests/capability_boundary.rs`
- `scripts/ci/prepare-desktop-bundle.sh`
- `scripts/ci/build-desktop.sh`
- `scripts/tests/check-desktop-release-contract.py`
- `scripts/tests/verify-macos-desktop-bundle.sh`
- `scripts/tests/verify-windows-desktop-installer.ps1`
- `.github/workflows/release.yml`

Verification covers owner reconciliation, startup readiness, bounded stop,
update rollback, bundle contents, version agreement, signed manifest shape,
platform installer policy, PATH ownership, and native capability boundaries.

## Related decisions

- ADR-0001 defines workspace and runtime composition.
- ADR-0002 defines runtime layout and device-local configuration.
- ADR-0007 defines process lifecycle and bounded phased shutdown.
- ADR-0035 defines the CLI adapter and resident-server boundary.
- ADR-0047 defines the native notification capability boundary.
- ADR-0050 defines the layered test architecture.
- ADR-0051 proposes release verification gates for packaged artifacts.
- ADR-0052 defines the Rua-owned Console and Conversation View Runtime.
- [ADR-0054](0054-bundled-browser-and-computer-use.md) defines bundled desktop
  automation drivers and system Chromium browser reuse.
