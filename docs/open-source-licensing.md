# Distribution licensing requirements

Rua's own code uses **Apache-2.0**. The repository and application distributions
also contain third-party material under other licenses. These requirements must
be met before describing a release as cleared for public redistribution;
this document does not grant rights to unresolved third-party material.

## License design

Keep the existing unmodified Apache-2.0 text in the root `LICENSE`. Apply it
to Rua-owned Rust, Android host, web console, Python SDK, documentation, and
original skills unless a specific file says otherwise. It permits commercial
use, modification, and redistribution, and includes a contributor patent
grant. Preserve applicable attribution and modification notices.

Keep the current simple structure:

| File | Purpose |
| --- | --- |
| `LICENSE` | Full Apache-2.0 grant for Rua-owned material |
| `LICENSES.md` | Directory/component license boundaries, including unresolved exceptions |
| `NOTICE` | Concise applicable copyright and attribution notices |
| `THIRD_PARTY_NOTICES.md` | Source provenance, versions, local modifications, and locations of full texts |
| Component-local license files | Full upstream terms and copyright notices |
| Release inventories and source archives | Evidence for the actual shipped target/features and source obligations |

Use the Apache contribution terms in section 5 for contributions intentionally
submitted for inclusion, unless explicitly stated otherwise. Contributors
retain copyright; no separate copyright assignment or CLA is introduced by
this guide. Imported third-party material must identify its own source and
license rather than being labeled as original Rua code.

Apache section 6 does not grant general trademark rights. Keep branding
questions separate from permission to use the software; do not append
noncommercial, no-competition, or mandatory display clauses to Apache-2.0.
This guide does not change existing artwork copyright licenses.

For dependencies offered under `MIT OR Apache-2.0`, Rua may use the Apache
option while retaining both upstream texts in vendored source. Additional
BSD notices still apply to their covered portions. Treat `OR` and `AND`
expressions correctly; a permissive alternative is not permission to drop
unrelated third-party notices.

## What the code and build actually contain

| Component | Verified evidence | Treatment |
| --- | --- | --- |
| QwenPaw catalogs | `crates/rua-provider/data/qwenpaw.json`, generated `provider_templates.rs`, and `crates/rua-config/data/qwenpaw-channels.json` contain pinned upstream/source metadata | Retain Apache attribution for these materials |
| Console origins | `console/NOTICE` records an initial source import | Retain the historical notice; determine any remaining derived files from source/history, not API compatibility alone |
| Android host | The Termux App-derived DocumentsProvider and its manifest entry were removed; native payload staging extracts selected components rather than installing Termux App | Current host uses Apache-2.0; historical revisions retain their original licenses |
| PRoot and loader | APK staging carries independent executables from pinned Termux packages | Separate GPL components; deliver matching source and build materials |
| talloc | PRoot dynamically depends on the bundled talloc library; Rua's resident executable does not link it | Library source is LGPL-3.0-or-later; keep its source and replacement rights |
| Cua | Both a separate driver and copied host SDK files are present | MIT notice covers both scopes; source/patch hashes are already pinned in the automation manifest |
| agent-browser | Separately built executable plus Rua adapter patch | Apache-2.0, full text and modification notice already staged with automation resources |
| Vendored networking | iroh, iroh-relay, iroh-blobs, iroh-docs, swarm-discovery, including relay under `libs/` | Preserve their upstream licenses independently of the root default |
| Document skills | `crates/rua-skills/src/builtin.rs` embeds the entire `builtin_skills` tree via `include_dir!` | The four current trees now use pinned public Apache-2.0 sources with embedded license/attribution texts |
| Font | `console/src/styles/theme.css` loads JetBrains Mono | Include its OFL text when shipping the font/WebUI |

DSHBox is an architecture and supply-chain reference. It is not a license
supplier for the Termux-built native packages. If implementation code is copied
from DSHBox, review that code under its own applicable license before inclusion.
Likewise, matching QwenPaw behavior is not by itself evidence of copied source;
retain attribution where actual source/data provenance supports it. Rewriting
code in another language does not by itself prove independence either.

## Distribution requirements

### 1. Preserve document skill provenance

The four built-in document skills use pinned public Apache-2.0 sources with Rua
adaptations. `crates/rua-skills/document-skills-upstream.json` records the upstream
commit and archive; each skill carries its license and modification notice.
For updates, compare against those sources, document local adaptations, and run
the focused builtin tests and validate the updated document workflows.

Review all history included in a source distribution: replacing current files
does not remove restricted materials from older commits.

### 2. Complete the Android native source-delivery chain

`mobile/runtime/native-dependencies.json` pins package URLs, binary hashes,
upstream source archives, source hashes, and a Termux build-recipe snapshot.
The preview workflow archives the three matching-version upstream sources plus
the complete recipe snapshot (including patches and common build scripts) in
`rua-android-native-sources.tar.gz`. See
[Android native source delivery](android-native-source-delivery.md) for the
version/hash checks, rebuild/replacement procedure, and reproduction limits.

The package metadata says PRoot `GPL-2.0`, but the pinned main program and
loader headers explicitly permit GPL-2.0-or-later. talloc's library source
permits LGPL-3.0-or-later even though its package metadata says GPL-3.0.
Record these distinctions. For the PRoot/talloc linked distribution, use
compatible GPL v3 terms after verifying the full matching source/patch set;
do not describe this combination as GPL-2.0-only plus LGPL v3.

Keep Rua's independently implemented host and runtime separate from these
components. The current subprocess/dynamic-library architecture supports
separate component licensing; process boundaries alone are not a universal
license exemption and do not waive native distribution obligations.

Before shipping each APK, publish a matching native source archive beside it:
upstream sources, matching packaging recipes and patches, build configuration,
Rua's SONAME/NEEDED transformation script, and a manifest connecting them to
the exact shipped binary hashes. Include the GPL/LGPL/BSD texts and upstream
copyright notices. Use direct downloadable source accompanying the binary
release rather than relying solely on a third-party homepage or generic source
URL. Address library replacement/relinking and any applicable installation
information requirements; validate the supported replacement procedure.

Debian/rootfs, Node, and uv are separately acquired runtime artifacts, not
covered by Rua's root license. Preserve their notices on download. If Rua
redistributes an offline ZIP or mirrors those binaries, include them in its
own source-delivery and dependency review as well.

### 3. Deliver notices and dependency sources for each real artifact

The existing Rust SBOM helper is useful but not complete release clearance.
Cargo metadata includes optional/build/dev packages and is not a list of
necessarily shipped code. Inventory the workspace and the separate desktop
Cargo workspace using the release target and feature selection; also cover
npm/WebUI, Gradle, the independent automation programs, and native payloads.
Retain required build-tool notices when redistributing those tools or source.

The current metadata review found MPL-2.0 candidates including `option-ext`,
`attohttpc`, UniFFI, and desktop CSS/selector crates. Determine which actually
ship, then include their license texts and exact source locations or matching
source archives, including modifications. MPL does not require relicensing
independently written Rua files; its covered source must remain available.
For dual-licensed dependencies such as `self_cell` or DOMPurify, record the
chosen permissive option rather than treating the alternative as mandatory.

For each independently downloadable artifact, include or accompany it with:
root `LICENSE`, `NOTICE`, scope and third-party notices, applicable complete
third-party texts/copyright notices, a target-specific dependency inventory,
and required source-access information. Keep namespaced paths for component
texts rather than flattening files all named `LICENSE`.

The preview workflow accompanies each installer with a target-specific notices
archive generated by `scripts/release/package-notices.py`. It contains Cargo/npm
license texts, the font notice, matching MPL crate sources, and a conservative
resolved dependency inventory; Android also includes its selected Maven runtime
metadata, distributed license files, and sources where the publisher provides them. These companion archives supplement the automation/native notices
inside the installed application. Keep the companion materials available beside
each installer. Archive generation is not a blanket legal-clearance assertion;
inspect the actual target inventory and assembled contents for each release.

## Authoritative references

- [Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0.txt), sections 3–6.
- [Pinned public Anthropic skills README](https://github.com/anthropics/skills/blob/2c5958ecac7cee2b583967a2c4981d0f4a75c579/README.md)
  and each public skill's `LICENSE.txt`.
- [Pinned PRoot source](https://github.com/termux/proot/tree/v5.1.107.102),
  especially `src/cli/proot.c`, `src/loader/loader.c`, and `COPYING`;
  the native manifest pins the talloc source archive.
- [GPL v3](https://www.gnu.org/licenses/gpl-3.0.html) and
  [LGPL v3](https://www.gnu.org/licenses/lgpl-3.0.html).
- [Mozilla MPL FAQ](https://www.mozilla.org/en-US/MPL/2.0/FAQ/), questions 8–12.
