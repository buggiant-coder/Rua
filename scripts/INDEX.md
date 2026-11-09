# Scripts Index

Public scripts cover installation prerequisites, application packaging, and the
release checks used by CI. Local service launchers, device-specific acceptance
drivers outside CI, and maintainer utilities are excluded by the repository allowlist.

## Packaging entry points

| Script | Purpose |
|---|---|
| `install-protoc.sh` / `install-protoc.ps1` | Install the host Protocol Buffers compiler required by prost-build. |
| `package-mobile-apk.sh` | Build a debug or release APK; `--install -s SERIAL` installs to an explicitly selected adb target. |
| `ci/build-desktop.sh` | Package the Tauri desktop app on the matching native platform. |
| `ci/build-release.sh` | Build Linux command-line release archives. |

Install desktop and Android applications using their DMG, NSIS, or APK packages.
See [Android packaging](../docs/android-packaging.md) and
[release signing](../docs/release-signing.md).

## Build and release support

| Location | Purpose |
|---|---|
| `android/` | Build the native runtime, stage and verify APK payloads, and prepare optional offline runtime materials. |
| `ci/` | Prepare desktop sidecars and bundled automation tools, then build release artifacts. |
| `release/` | Desktop signing checks, updater manifests, and Windows signing/installer/update integration. |
| `with-console-build-lock.py` | Serialize shared Console staging during desktop and APK builds. |

## Release verification

`tests/` contains release-gate checks, CI integration tests, and their dependencies:

- `release-gate.sh`: aggregate release checks; `--android-apk APK` also validates a built APK.
- `clippy-touched.sh` and `cargo-test-rua-server.sh`: affected-crate Clippy and test execution; missing Git history or an unavailable comparison falls back to all workspace members plus the standalone Desktop crate.
- `check-feature-flags.sh`, `check-config-roundtrip.sh`, and `check-threat-model-coverage.sh`: feature, configuration, and security documentation checks.
- `check-android-release-contract.py`: SDK-free APK packaging contracts.
- `check-desktop-release-contract.py` and `check-desktop-automation-staging.py`: desktop packaging contracts.
- `verify-macos-desktop-bundle.sh` and `verify-windows-desktop-installer.ps1`: platform artifact acceptance.
- `android-native-runtime-e2e.py`, its model/ACP/Skill/recovery fixtures, and `prepare-android-webview.py`: the Android CI emulator matrix.

Contributors run `scripts/tests/release-gate.sh` before submitting changes. The
release workflow also runs it on pull requests and main pushes. Console and
Android E2E workflows retain their platform prerequisites and test fixtures.

Product tests remain in the crate, Console, and mobile source directories.

## Upstream catalogs

The QwenPaw provider and channel snapshots record the upstream revision and
per-source SHA-256 hashes. Provider snapshots live in
`crates/rua-provider/data/qwenpaw.json` and
`crates/rua-provider/src/provider_templates.rs`; Rua-specific adaptations live in
`crates/rua-provider/data/local_templates.json`. Channel snapshots live in
`crates/rua-config/data/qwenpaw-channels.json`; Rua-specific differences live in
`crates/rua-config/data/local-channels.json`.

Review the upstream revision, source hashes, and snapshot diffs when updating
these catalogs. Transport and request-adapter behavior is implemented separately in Rust.
See [ADR-0006](../docs/adr/0006-model-knowledge-catalog-and-reasoning-control.md),
[ADR-0016](../docs/adr/0016-channel-traits-implementation-isolation-and-feature-gates.md),
and [third-party attribution](../THIRD_PARTY_NOTICES.md).
