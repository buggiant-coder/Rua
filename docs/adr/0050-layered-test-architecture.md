# ADR-0050: Layered test architecture

> **Status:** Accepted

## Context

The workspace contains many Rust crates, a TypeScript Console, Android code,
Tauri packaging, scripts, and cross-process protocols. Running every check
after every small edit is slow, while relying only on narrow unit tests misses
wire compatibility, subsystem composition, binary startup, and browser-visible
regressions.

Rust also pays compile and link cost per integration-test binary. A root
language-agnostic test tree or one binary per source file would obscure crate
ownership and make targeted verification expensive. The architecture needs
clear test intent and verification that expands with blast radius. Whether a
build is ready to publish is a separate decision owned by ADR-0051.

## Decision

### D1 - Every test has one logical layer

Tests are classified by the failure signal they provide:

- Unit tests exercise one module or crate interface with local fakes.
- Contract tests verify public Rust interfaces, adapter families, serialized
  DTOs, errors, streams, and compatibility rules.
- Integration tests exercise multiple Rua subsystems in an isolated workspace
  without external credentials.
- E2E tests launch the real binary, packaged runtime boundary, or browser and
  drive it as a client would.

Physical placement does not determine the layer by itself.

### D2 - Rust tests remain crate-local and theme-aggregated

Rust source tests live under the owning crate or library `tests/` directory.
For suites with many files, `autotests = false` and explicit `[[test]]` targets
aggregate related files by layer and functional theme, such as `unit_runtime`,
`contract_sync`, or `integration_server`.

Small crates may use one target per layer. Large crates split by coherent
behavior theme, not alphabet, file count, or one target per case.

The repository is still transitioning: several manifests retain multiple
standalone explicit test targets, and `rua-desktop` still permits Cargo's
automatic test discovery alongside an explicit notification test. Existing
layout must not be described as uniformly aggregated until those manifests are
actually migrated.

### D3 - Contract tests are first-class

Traits prove method shape but not behavioral compatibility. Shared adapters and
public protocols require table-driven contract suites for normalization,
serialization, error semantics, state transitions, capability negotiation, and
frontend-visible frames.

Contract fixtures must make each implementation discoverable in failure output.

### D4 - Verification expands with change risk

During implementation, run focused tests for the changed behavior and use
`scripts/tests/clippy-touched.sh` for touched crates; its optional `--deps`
mode expands to reverse dependencies. Required CI may run broader workspace
checks, but local iteration should remain proportional to the change.

Documentation-only changes use structural and reference checks unless they
alter executable examples or verification policy.

### D5 - E2E is reserved for assembled boundaries

E2E coverage is used when correctness depends on a real server process,
served Console assets, Playwright browser behavior, CLI-to-server interaction,
cross-process streams, Android runtime, or packaged desktop artifacts.

In-process router tests remain Integration or Contract tests. E2E does not
replace cheaper lower-layer coverage.

### D6 - PR E2E is small and path-gated

The default Console E2E lane runs only for paths that can break the real
server/Console boundary. It builds and starts an isolated server, waits for
readiness, runs a bounded Playwright baseline, and uploads server and browser
artifacts.

Heavier, credentialed, multi-Device, Android, or packaging flows begin in
nightly, manual, or dedicated workflows until deterministic and fast enough for
promotion.

### D7 - Workflow policy and reusable mechanics are separate

Reusable jobs or scripts own environment setup, server lifecycle, browser
installation, artifact capture, and deterministic command execution. Entry
workflows own triggers, path filters, schedules, concurrency, target matrices,
and secret availability.

Android and desktop verification use dedicated product-path scripts because
their environments and artifacts differ from the Console baseline.

### D8 - Priority reflects user impact

Where priority labels are used, `p0`, `p1`, and `p2` describe user impact and
test placement, not code complexity. Filtering should use existing test runner
capabilities or script-owned lists rather than multiplying Cargo test binaries.

## Hard invariants

- Every test has one logical layer.
- Rust tests remain owned by the crate whose boundary they verify.
- New Rust test files join an existing coherent aggregator when possible.
- Contract coverage is not replaced by E2E.
- In-process HTTP tests are not mislabeled E2E.
- Local agents do not run workspace-wide checks by default.
- The touched-crate helper can expand to reverse dependencies.
- Real external credentials are absent from default PR test lanes.
- Long-running tests produce non-interactive logs and failure artifacts.
- A test listed in an ADR is evidence only when its required features are
  enabled by the command being cited.

## Authority, persistence, and failure boundaries

Crate manifests own Rust test aggregation. Test source owns behavioral
assertions. Scripts own repeatable local test mechanics. GitHub workflows own
remote trigger and matrix policy.

A lower-layer failure blocks promotion to broader test layers. An E2E
infrastructure failure must retain logs and browser artifacts so it can be
distinguished from a product failure. External-service tests do not silently
enter required PR lanes.

## Consequences

- Developers receive fast, specific feedback for local changes.
- Shared contracts fail closer to the owning adapter or protocol.
- E2E remains small enough to be actionable.
- Test organization scales without hundreds of unnecessary linked binaries.
- Release readiness can evolve independently under ADR-0051.

## Rejected alternatives

- A repository-root `tests/unit|contract|integration|e2e` tree for Rust.
- One Cargo test binary per source file.
- Naming every crate-local test "unit".
- Running all E2E specifications on every pull request.
- Keeping all E2E manual-only.
- Replacing contract tests with browser tests.
- Duplicating environment setup across workflow files.

## Implementation and verification

- crate-local `tests/` directories
- explicit `[[test]]` targets in crate and library `Cargo.toml` files
- `.config/nextest.toml`
- `scripts/tests/clippy-touched.sh`
- `scripts/tests/cargo-test-rua-server.sh`
- `.github/workflows/_console-e2e-job.yml`
- `.github/workflows/e2e-integration.yml`
- `.github/workflows/android-native-runtime-e2e.yml`

Verification of this decision is the test layout and focused execution of the
affected suites. Feature-gated targets must be reported separately when the
invoking command does not enable their required features.

## Related decisions

- ADR-0007 defines process lifecycle exercised by real-process tests.
- ADR-0043 defines Android lifecycle and background residency acceptance.
- ADR-0048 defines parser and renderer contract coverage.
- ADR-0049 defines desktop packaging and updater artifacts.
- ADR-0051 owns release verification gates.
