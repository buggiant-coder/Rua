# Bundled desktop automation

agent-browser 0.39.4 (Apache-2.0), upstream commit
`04ba5264dade678062c3cdcce0f4f293e748249f`.
Copyright the agent-browser contributors. See agent-browser-LICENSE.

Rua modifications, 2026-09-20: `cli/src/connection.rs`, `cli/src/main.rs`,
`cli/src/native/daemon.rs`, `cli/src/native/cdp/chrome.rs`.
The accompanying agent-browser-rua.patch requires a live observed session
for input, propagates owner/Stop cancellation,
and closes the owned Chromium process group after a Unix daemon crash.
The upstream source archive and patch digests are pinned in manifest.json.
The program is built independently with Cargo.lock; no upstream workspace is
merged into Rua. Windows retains the upstream owned Job Object.

Cua Driver 0.38.0 (MIT), upstream commit
`85e2b1318bd77ccf3cfa88ccdf875a4bb7eaed19`.
Copyright the Cua contributors. See cua-driver-LICENSE.
The platform release binaries are unchanged before platform signing. The host
uses the same pinned SDK commit. Rua vendors only `embedded.rs` lifecycle
and the permission probes from `lib.rs`, with UniFFI exports and inline upstream
tests omitted. Rustfmt is applied without changing lifecycle behavior. Their
upstream and local source hashes are recorded under `cua_driver.host_sources`
in manifest.json and checked by the resident Runtime build. The vendored files
live in crates/rua-server/src/desktop_automation/cua_host.rs and
crates/rua-server/src/desktop_automation/cua_host/embedded.rs. Cua core/contract remain pinned
to the same commit. The full in-process SDK is excluded so ScreenCaptureKit and
Swift Concurrency are not loaded by the resident host on macOS 11/12. Native capture
remains in the independent daemon. No Node addon or Python runtime is bundled.

`release-set.json` is generated at build time and records the target, product
version, source/patch hashes and hashes of the programs before platform signing.
Platform signing changes executable bytes; the updater verifies the signed
complete package, and the platform bundle checks verify its signatures. The
Shell checks the compiled receipt identity and the running driver contracts.
Windows receipts also record a PE image digest excluding only Authenticode's
checksum/certificate fields and padding; signed component content is compared
with that digest. macOS checks the App's sealed signature. The pinned Cua
macOS executable requires macOS 13 or newer.
Browser software and mutable profiles are not part of this directory.
