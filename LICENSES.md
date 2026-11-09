# License scope

Rua-owned code and documentation use Apache-2.0 unless explicitly stated
otherwise. This is a multi-license repository; the root license does not
relicense third-party code, fonts, skills, or bundled executables.

| Scope | License / authoritative text |
| --- | --- |
| Rua-owned Rust, Python SDK, documentation, web console, and Android host source | [Apache-2.0](LICENSE) |
| Retained QwenPaw-derived source and catalog snapshots | Apache-2.0; attribution in [NOTICE](NOTICE) and [console/NOTICE](console/NOTICE) |
| `vendor/iroh/`, `libs/iroh-relay/` | MIT OR Apache-2.0; additional Tailscale BSD-3-Clause portions, with all three texts in each subtree |
| `vendor/iroh-blobs/`, `vendor/iroh-docs/` | MIT OR Apache-2.0; texts in each subtree |
| `vendor/swarm-discovery/` | [Apache-2.0](vendor/swarm-discovery/LICENSE.Apache_2.0) |
| Cua Driver and copied host SDK portions | [MIT](console/src-tauri/automation/cua-driver-LICENSE); exact scope and changes in [automation/NOTICE.md](console/src-tauri/automation/NOTICE.md) |
| Bundled agent-browser and Rua's adapter patch | [Apache-2.0](console/src-tauri/automation/agent-browser-LICENSE); upstream and patch pinned in [manifest.json](console/src-tauri/automation/manifest.json) |
| Android PRoot, loader, talloc, and android-shmem | Separate GPL / LGPL / BSD components; see [native notices](mobile/runtime/licenses/NOTICE.md) and [package/source pins](mobile/runtime/native-dependencies.json) |
| JetBrains Mono font | [SIL Open Font License 1.1](console/src/assets/fonts/JetBrainsMono-OFL.txt) |
| Built-in `docx`, `pdf`, `pptx`, `xlsx` materials | Apache-2.0 public Anthropic versions with Rua adaptations; full text and `NOTICE.md` in each skill tree, provenance in `crates/rua-skills/document-skills-upstream.json` |
| Other dependencies and separately acquired runtimes | Their own upstream licenses, determined for each distribution |

Retain applicable file and subtree notices. Neither this summary nor a package
manifest overrides the rights granted by the relevant copyright holders.
See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for provenance and the
[distribution requirements](docs/open-source-licensing.md) for release requirements and
outstanding blockers. This table is not a complete transitive dependency audit.
