# Third-party notices

This document records the principal source-code origins and license boundaries
in the Rua repository. Dependency lockfiles, package manifests, vendored
license files, and release SBOMs provide the detailed dependency inventory.

## QwenPaw

- Upstream: <https://github.com/agentscope-ai/QwenPaw>
- License: Apache License 2.0
- Copyright: Copyright 2025 The QwenPaw Authors
- Scope in Rua: portions of the early web console, provider/model and channel
  catalogs, discovery policies, and selected compatibility implementations

Rua began as an independent repository and its backend is a Rust
implementation. Early console work incorporated QwenPaw source, after which
the console was extensively modified. Selected Rust modules also preserve
behavior compatible with QwenPaw APIs or channel formatting.

The provider snapshot in `crates/rua-provider/data/qwenpaw.json` and generated
`provider_templates.rs` are adapted from upstream commit
`5bd10453e055c9d7eed0afc914bf94ecae81763c`. Provider discovery, filtering, and
request normalization in `crates/rua-provider` and `libs/llm-multiplex` include
Rust adaptations of QwenPaw behavior. The snapshot records per-source hashes;
see [catalog update workflow](scripts/INDEX.md#upstream-catalogs).

The channel snapshot in `crates/rua-config/data/qwenpaw-channels.json` is
adapted from the same upstream commit. It records the inherited channel
configuration declarations, registry, and channel source hashes. Shared
pipeline behavior and selected transports in `crates/rua-channel`, voice
handling in `crates/rua-voice` / `crates/rua-server`, and workspace
composition include Rust adaptations of QwenPaw behavior; see
[catalog update workflow](scripts/INDEX.md#upstream-catalogs).

Files incorporating QwenPaw source remain under Apache License 2.0. Modified
files are identified by source comments where practical, and the repository
`NOTICE` and `console/NOTICE` preserve the upstream attribution.

## lark-oapi

- Protocol reference: [lark-oapi 1.5.3 on PyPI](https://pypi.org/project/lark-oapi/1.5.3/)
- License: MIT
- Copyright: Copyright (c) 2023 Lark Technologies Pte. Ltd.
- Scope in Rua: handshake, heartbeat, fragment assembly, and ACK behavior in
  `crates/rua-channel/src/feishu/ws.rs`, adapted to Rust.

The reference is the SDK used by QwenPaw's Feishu implementation. Its license
as distributed in the 1.5.3 wheel follows:

```text
MIT License

Copyright (c) 2023 Lark Technologies Pte. Ltd.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Android host and native dependencies

The current `mobile/mobile-shell/` host source is Apache-2.0. The Termux App
host, terminal modules, and derived DocumentsProvider have been removed;
launcher and notification artwork use Rua branding. Earlier host versions
retain their original license terms.

PRoot and its native dependencies are separate bundled components and retain
their upstream licenses. Their Android binaries currently come from the
Termux package repository; this does not include Termux App or its userland.
See `mobile/runtime/native-dependencies.json` and `mobile/runtime/licenses/`
for versions, binary hashes, and notices.

## Vendored networking libraries

| Local scope | Upstream | Version | License |
| --- | --- | --- | --- |
| `vendor/iroh/` | <https://github.com/n0-computer/iroh> | 1.0.0-rc.0 | MIT OR Apache-2.0, with BSD-3-Clause portions |
| `libs/iroh-relay/` | <https://github.com/n0-computer/iroh> | 1.0.0-rc.0 | MIT OR Apache-2.0, with BSD-3-Clause portions |
| `vendor/iroh-blobs/` | <https://github.com/n0-computer/iroh-blobs> | 0.101.0 | MIT OR Apache-2.0 |
| `vendor/iroh-docs/` | <https://github.com/n0-computer/iroh-docs> | 0.99.0 | MIT OR Apache-2.0 |
| `vendor/swarm-discovery/` | <https://github.com/rkuhn/swarm-discovery> | 0.6.2 | Apache-2.0 |

The iroh-family license texts preserve N0, INC. copyright notices. The
additional `LICENSE-BSD3` files in iroh and iroh-relay preserve the Tailscale
Inc & AUTHORS notices; they do not replace the MIT/Apache licenses.

The iroh and iroh-relay crates record upstream commit
`96b4b0e80de1f46f4eff2cfd40d42797535d7582` in `.cargo_vcs_info.json`.
Their MIT and Apache texts were restored verbatim from that commit's repository
root. Rua's iroh modifications are described in `vendor/iroh/RUA-PATCHES.md`.
The iroh-docs crate records upstream commit
`db8d73b8beb5d7cb592538ec75b11ffcc5677087`. Each vendored subtree retains its
own license texts; local modifications do not change the upstream grant.

## Desktop automation

- Cua Driver 0.38.0: <https://github.com/trycua/cua>, MIT, copyright
  2025 Cua AI Inc. Rua also incorporates selected host SDK source in
  `crates/rua-server/src/desktop_automation/cua_host.rs` and
  `crates/rua-server/src/desktop_automation/cua_host/embedded.rs`.
- agent-browser 0.39.4: <https://github.com/vercel-labs/agent-browser>,
  Apache-2.0. Rua builds the independent program with a local adapter patch.

`console/src-tauri/automation/` contains the full license texts, modification
notices, exact upstream commits, source hashes, and patch. The desktop staging
script includes these materials in its automation resources. The component
licenses also apply to incorporated source; running other parts in separate
processes does not remove attribution obligations.

## JetBrains Mono

The console loads `console/src/assets/fonts/JetBrainsMono-Regular.woff2`
from its CSS. Copyright 2020 The JetBrains Mono Project Authors.
The font is licensed under SIL OFL-1.1; the full text is in
`console/src/assets/fonts/JetBrainsMono-OFL.txt`. Include that text with
redistributed WebUI/font assets.

## DSHBox implementation reference

DSHBox was consulted as an implementation reference for the Android PRoot
architecture and native payload supply chain. This acknowledgement does not
claim that Rua includes DSHBox host source or receives a license to its
components through DSHBox. Native payload provenance is the pinned upstream
package/source records above. Any copied DSHBox source would require a
separate provenance and license review.

## Built-in artifact skills

The built-in `docx`, `pdf`, `pptx`, and `xlsx` trees now derive from
Anthropic's public Apache-2.0 skills at commit
`2c5958ecac7cee2b583967a2c4981d0f4a75c579`:
<https://github.com/anthropics/skills/tree/2c5958ecac7cee2b583967a2c4981d0f4a75c579/skills>.

Each tree includes the upstream `LICENSE.txt` and Rua's `NOTICE.md`. The source
archive hash, builtin version, and modified/added file paths are recorded in
`crates/rua-skills/document-skills-upstream.json`. Rua's changes adapt dependency
and working-directory guidance, version metadata, case-sensitive reference
paths, LibreOffice platform discovery, and text whitespace. The old restricted
trees were replaced, including the 14 files absent from that public upstream;
their older licenses were not simply relabeled.

`crates/rua-skills/src/builtin.rs` embeds these trees with `include_dir!`, so
the full texts and attribution travel with skill extraction. This update
covers the current trees; historical revisions require their own review before
publishing the repository history. See `docs/open-source-licensing.md`.

## Rust and JavaScript dependencies

Rust and JavaScript dependencies remain under their respective upstream
licenses. `Cargo.lock`, Cargo manifests, `console/package-lock.json`, and
release SBOM output identify candidate dependency versions. They are not
by themselves a complete inventory of shipped code or a substitute for full
license texts and required source delivery. Inventory release targets and
features separately, including the desktop shell, npm bundles, Android native
payload, Gradle dependencies, and the independent automation executables.
