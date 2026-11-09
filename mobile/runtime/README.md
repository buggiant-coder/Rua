# Android runtime inputs

The APK carries Rua, WebUI, PRoot and its Android ELF dependencies. Debian and
its toolchains are separate, verified installation inputs. No Docker daemon or
local Docker image build is required.

## Pinned materials

`runtime-manifest.json` is the trust root shipped inside the APK. Every offline
ZIP member and downloaded artifact must match its size and SHA-256. A mirror
cannot supply its own trusted checksums. Architecture is ARM64, Debian trixie.

The base is the official Debian 13.7 ARM64 image published on 2026-10-27 by
[debuerreotype](https://github.com/debuerreotype/docker-debian-artifacts/tree/e36653f9967231373aeba7cff2102fd004ac4f4a/trixie).
[Debian's official checksum page](https://docker.debian.net/) identifies its OCI
manifest. The manifest digest pins the compressed rootfs layer digest recorded
here. The installer uses the archive as files, not as a running container.

The 64 Python/support `.deb` artifacts were resolved with Debian APT against the
base's Debian archive keyring. APT verified the archive InRelease signatures and
package indexes. Each package's complete version, architecture, archive path,
size and SHA-256 is frozen in the APK manifest. These are upstream packages, not
a private reconstructed rootfs. Runtime installation unpacks the complete local
set with dpkg, then requires dependency configuration, an empty dpkg audit,
Python SSL/SQLite imports and venv creation before activation. No repository
access is needed to install imported materials.

`native-dependencies.json` separately pins three upstream Android packages from
[Termux's package repository](https://packages.termux.dev/apt/termux-main/).
Only PRoot, its loader, talloc and android-shmem ELF files are packaged. There is
no Termux bootstrap, shell, prefix tree or Termux service in the native launch
path. The staging script adjusts talloc's ELF SONAME for Android APK library
filenames and verifies original package bytes first.

The supply chain is upstream PRoot/talloc/android-shmem source → Termux Android
package builds → pinned official `.deb` downloads → selected ELF files in the
APK. This follows the same component sourcing as DSHBox, whose checked-in
`jniLibs` contain prebuilt PRoot dependencies. Rua downloads and verifies the
original packages directly rather than depending on a DSHBox checkout.
`SourceURL` and `SourceSHA256` record the upstream source archives;
`LibraryLicense` distinguishes talloc's LGPL library license from its package
metadata. Keep the corresponding build recipes and patches with binary-release
source materials; upstream archives alone are not the complete build inputs.

## Offline material preparation

From the repository root:

```sh
python3 scripts/android/prepare-runtime-offline.py
```

This fetches and verifies the public upstream inputs, then creates
`target/debian-runtime-offline.zip`. `--cache` reuses verified downloads;
`--out` chooses the resulting ZIP. Neither downloads nor the ZIP are embedded
in the APK. The Android import action uses the same manifest and installation
transaction as online preparation.

A manifest update must include refreshed dependency resolution, upstream trust
evidence and device installation/probe acceptance. Do not edit hashes to make
an unexpected download pass. Preserve the previous working environment until
its replacement has passed its own probes.

## Automatic preparation and mirrors

The native resident and environment preparation start together. The foreground
Service owns one preparation operation; Activity recreation only observes it.
A verified ready guest is not reinstalled. Offline startup waits for validated
connectivity. Transient failures have at most three attempts per service start;
explicit cancellation persists and requires a user retry. Offline import remains
available through the environment status strip.

Debian's OCI manifest is fetched by its fixed digest from Docker Hub or DaoCloud,
using anonymous, repository-scoped pull tokens. No Docker engine is used. Tokens
are never forwarded to a cross-host blob redirect. Package candidates are Debian's
CDN, TUNA, USTC and a dated Debian Snapshot fallback. Paths retain the pinned full
version and architecture from the APK manifest.

Each exact artifact is ranked independently by monotonic elapsed time for HTTPS
HEAD metadata. HTTP failures and unexpected content lengths are excluded. This
measures request latency, not sustained throughput. Missing files at an otherwise
reachable host do not qualify. Metadata eligibility is not a trust decision:
size and APK-pinned SHA-256 are checked over downloaded bytes before cache
publication, and a bad download tries the next eligible mirror. There are at most
three probe workers, an eight-second selection window, four-second connection/read
timeouts and six redirects. Registry metadata preparation has its own eight-second
window. Node independently ranks its official distribution endpoint and npmmirror endpoints; uv uses the pinned Astral release.

Controlled HTTP tests are restricted to loopback and explicitly enabled only by
test construction; production requests require HTTPS throughout redirects. Device
acceptance downloads actual upstream materials under the Android app UID and runs
the installed Debian Python/venv probes.

## Installation ownership and recovery

`installation.json` records the unique operation ID and its exact staging directory.
Recovery removes only that recorded directory; unrelated directories and canonical
workspace/home data stay intact. A complete ready marker plus directory rename is
the commit point. Restart after rename reconciles the journal without reinstalling.
Invalid cache entries are quarantined. Unverified partial HTTP downloads restart
from the beginning; there is no optimistic Range resume without content validation.

The small `process-group.c` launcher is compiled with the Android NDK into the APK.
It supervises an isolated installer group, kills the whole group on TERM or parent
exit, and reaps its leader. Holding the leader unreaped during cleanup prevents PID
reuse. No downloaded host executable or Termux userland is involved.

Recovery acceptance requires process death during download, extraction, probing,
and both sides of activation. Device UI tests exercise the actual cancel/retry
buttons while dpkg runs and check descendant cleanup. Run destructive acceptance
on dedicated test devices.

## Language toolchains

The manifest pins Linux ARM64 Node 24.18.0 and musl uv 0.13.7 by byte count and
SHA-256. They are downloaded on first launch, after Debian becomes available.
Offline ZIPs contain the same artifacts. Neither archive is included in the APK.
Each archive extracts into an owned staging directory, then moves atomically to
`/usr/local/lib/rua/toolchains/<sha256>`. Relative `/usr/local/bin` links expose
commands; existing unrelated commands are preserved. A version marker is committed
only after the real guest commands run successfully. Runtime Inventory reads that
marker through the command's actual target; replacing a command does not retain an
unrelated pinned version. Retry preserves Debian and reuses verified downloads.

Guest uv uses `UV_LINK_MODE=copy`: PRoot's hard-link emulation can otherwise create
absolute native cache links that guest Python cannot resolve. npm's global prefix
is `/root/.local`, whose bin directory is on guest PATH. Guest home and canonical
workspace are persistent binds; language tool updates do not replace user packages.

Skill acceptance imports deterministic Python wheel and npm package fixtures
through the normal Skill API and executes them through the actual Agent. Coverage
includes dependency installation, shebangs, nested processes, private-environment
checks and cross-tool workspace reads.

## Independent environment updates

A compatible installed Debian 13 / Python 3.13 / Node 24 environment remains usable
when the APK changes. Startup does not download a new toolchain merely because its
pinned patch version differs. The native environment status strip offers an explicit
update action; first installation remains automatic. The APK manifest declares the
supported combination and target artifacts, rather than following floating latest.

All Android external process plans enter through `librua-runtime-gate.so`. PRoot
holds a shared lease for the traced process lifetime. An update takes an exclusive,
nonblocking lease before copying any data. Active tools, MCP, ACP, Plugins or terminal
sessions therefore reject the update with an explicit retry instruction. Closing the
last terminal also closes its otherwise idle Node state bridge. The lock coordinates
process lifecycle inside the app UID; it is not an isolation boundary.

`RuntimeUpdate` clones Debian and guest HOME, keeps canonical Rua HOME/workspaces
separate, then applies the verified recipe to the private candidate. Existing newer
system packages remain installed. Required cached packages are unpacked/configured
with dpkg and checked with APT; additional base upgrades use exact requested versions
from signed Debian Snapshot repositories at the manifest date. No removals, downgrades,
rootfs overlay, or Python/Node major ABI change is accepted. Incompatible dependencies
fail the candidate instead of silently replacing the user's environment.

A durable journal and IDs in both directories describe the activation. Admission
stays closed throughout their renames. A kill before the new rootfs commit restores
the owned backups; a kill after both directories commit completes the transaction.
`update.pending` prevents new guest execution until recovery restores a consistent
pair. APK resources use their own content-addressed activation and never share this
journal. `runtime/update.log` retains the last failed candidate's diagnostic tail.

Update acceptance covers process kills/restarts during both guest transactions
and APK resource extraction on dedicated test devices. The same-process
`runtime_info` probe checks that the installed version is refreshed following
activation, without restarting the resident.
