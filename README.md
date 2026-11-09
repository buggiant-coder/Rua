<p align="center">
  <img src="console/src-tauri/icons/128x128.png" width="88" alt="Rua logo" />
</p>

# Rua

**All your devices. One mind.**

Your personal AI, across your phone and computers. Every device runs a full
agent. Pair your devices, keep conversations in sync, and choose where the
work happens.

Ask your workstation to work on a codebase from your phone. Pick up the same
conversation on your laptop. Let your phone use its own files and tools.

[简体中文](README.zh-CN.md) · [Getting started](#get-started) · [Architecture](docs/adr/README.md)

> **Pre-release:** Rua is under active development. Platform packaging and
> acceptance are still in progress; see [platform status](#platform-status).

## Why Rua

### A full agent on every device

Your phone, laptop, and workstation each own their agent runtime, local files,
and tools. Android runs Rua natively and automatically prepares a Debian
Linux tool environment for scripts, skills, and external programs.

### Pair your devices. Rua handles the connection.

Connect across networks without setting up a public IP, port forwarding, or a
VPN. Rua automatically discovers connection paths and attempts NAT hole
punching to establish a direct connection. When a direct path is unavailable,
it can use a relay while keeping the connection end-to-end encrypted.
Connectivity still depends on network restrictions and relay availability.
See the [device network design](docs/adr/0037-device-network-runtime-and-shared-device-sessions.md).

### Conversations that travel with you

Pair your devices to synchronize conversations, agent configuration, skills,
and structured memory through encrypted peer-to-peer sync. Open a
conversation from another paired device and keep its context within reach.
Local data remains authoritative, and local writes do not wait for another
device to come online.

### Work on the device that has what you need

Choose the execution device when creating a chat. Keep the conversation on
your phone while a paired computer works with its own repositories, files,
and tools. Each device remains an equal peer, with its own capabilities.

### Your coding agents, within reach

Use ACP-compatible coding agents such as Codex or Claude Code on the
execution device while viewing the conversation from another paired device.
Rua keeps the coding conversation and relays the runner's permission requests.
The coding agent must be installed and configured on that execution device.

## What you can do

| Situation | With Rua |
| --- | --- |
| Your repository is on your workstation; you have your phone | Start a chat targeting the paired workstation and ask it to work with that repository. |
| You move from your phone to your laptop | Open the synchronized conversation on the paired laptop. |
| You want to work directly on Android | Run the phone's agent with its local files, Linux tools, and Android integrations. |
| You use an external coding assistant | Configure it on an execution device and use its ACP chat through Rua. |

Cross-device execution requires the selected device to be online and its tools
to be available. A chat keeps the execution target chosen at creation.

## Build your own setup

- **Models:** configure model providers and compatible model-service endpoints.
- **Tools:** connect MCP servers, enable skills, and install plugins. Tool and
  dependency availability varies by device.
- **Automation:** use scheduled tasks and supported external messaging channels.
- **Device interaction:** desktop browser and computer automation, plus native
  Android integrations, subject to platform support and system permissions.
- **Control:** per-agent security profiles, approval flows, and audit records.
- **Memory:** structured memory with local vector search and portable state sync.

Rua's runtime is built in Rust. The web console is shared across desktop and
Android. Model calls use your configured provider; local-first storage does
not mean every model or tool works offline. Android's PRoot environment provides
Linux compatibility, not a security sandbox. See the [threat model](docs/security/threat-model.md).

## Platform status

The release target is native desktop applications for Windows, macOS, and Linux,
plus an Android APK. The current source includes the following build paths:

| Platform | Architecture | Package | Release readiness |
| --- | --- | --- | --- |
| macOS | Apple Silicon, Intel | `.dmg` | Build path present; production signing, notarization, and installation acceptance required. |
| Windows | x64 | `.exe` installer | Build path present; native build, signing, and installation acceptance required. |
| Linux | x64 | `.deb` desktop app | Packaging added; native build and installation acceptance pending. |
| Android | ARM64 | `.apk` | Build path present; current builds use a public test key. Production signing and device acceptance required. |

These are build targets, not a claim that public installers are already
available. Desktop automation has additional platform requirements; see
[desktop automation](docs/desktop-automation.md).

## Get started

1. Run Rua on your device and configure a model provider.
2. Create a chat and try a task using a small test directory.
3. Pair another device, then create a chat with that device as its execution target.

The [中文使用入门](docs/getting-started.zh-CN.md) covers model setup, pairing,
permissions, updates, and data locations. Bring your own provider credentials;
Rua does not include model-service credits.

### Run from source

Install Rust 1.91+, Node.js/npm, Python 3, protoc, and your platform's build
tools. The repository uses sccache by default; install it or set
`RUSTC_WRAPPER=` for the build. Desktop shell dependencies are listed in the
[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/).

```bash
cd console
npm ci
cd ..
RUA_ROOT="$HOME/.rua-dev" cargo run -p rua-cli -- init
RUA_ROOT="$HOME/.rua-dev" cargo run -p rua-cli -- server start --host 127.0.0.1 --port 8765 --no-auth
```

In another terminal, start the frontend:

```bash
cd console
npm run dev
```

Open `http://127.0.0.1:5173`. The development API listens on
`http://127.0.0.1:8765`. This development entry point disables authentication
and is intended for local use. Configure providers in the console or run
`cargo run -p rua-cli -- setup-env`.

Desktop packaging uses [build-desktop.sh](scripts/ci/build-desktop.sh) on the
matching native platform; see [release signing](docs/release-signing.md) for
signing requirements. Android packaging is documented in
[Android builds](docs/android-packaging.md).

## Contribute

Use the [domain vocabulary](CONTEXT.md) and [architecture decisions](docs/adr/README.md)
to understand the project. Read [AGENTS.md](AGENTS.md) for test organization
and contribution constraints.

| Path | Purpose |
| --- | --- |
| `libs/` | Foundational types, contracts, storage, and provider abstractions |
| `crates/` | Agent runtime, server, sync, tools, channels, security, memory, and CLI |
| `console/` | Shared web console and desktop shell |
| `mobile/mobile-shell/` | Native Android application host |
| [python_sdk/](python_sdk/README.md) | Python plugin SDK |

Run focused checks for changed crates:

```bash
scripts/tests/clippy-touched.sh --test
```

Use Git diffs for focused local checks. When Git metadata or the comparison
baseline is unavailable, the script checks every workspace member and the
standalone Desktop crate; this full validation takes longer.

## Licensing

Rua is a multi-license repository:

- The Rust workspace, Rua-owned documentation, and the web console are
  licensed under the [Apache License 2.0](LICENSE), unless a file or subtree
  states otherwise.
- Portions of the early web console and selected compatibility modules
  incorporate or derive from QwenPaw, used under Apache License 2.0. Rua is
  an independent Rust implementation, not a QwenPaw fork. See [`NOTICE`](NOTICE).
- The current [`mobile/mobile-shell/`](mobile/mobile-shell/) host source is
  Apache-2.0. Bundled PRoot and native dependencies retain their own licenses;
  see [`mobile/runtime/licenses/`](mobile/runtime/licenses/).
- Vendored dependencies retain their upstream licenses.

See [`LICENSES.md`](LICENSES.md) and
[`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md) for license boundaries.
The four built-in document skills track pinned public Apache-2.0 upstream
versions. See the [distribution licensing requirements](docs/open-source-licensing.md)
for provenance and remaining source-delivery and release requirements.
