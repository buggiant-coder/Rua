# ADR-0051: Release verification gates

> **Status:** Proposed

## Context

Layered tests provide evidence about components and assembled boundaries, but
they do not by themselves prove that a release is publishable. Release
verification must also check configuration compatibility, feature combinations,
security contracts, version agreement, signing inputs, packaged artifacts, and
installer behavior.

The repository has scripts and workflows for these checks, but two current
helpers can return success without validating their advertised contract. The
configuration round-trip script logs a failed Rust test and missing required
fixtures without failing. The feature-flag script reports missing required
features as informational and can label failed required builds as skipped. An
aggregate script can therefore print `release ready` after a gate silently did
less than its name promises.

## Decision

### D1 - Release readiness is separate from test architecture

ADR-0050 defines test layers and placement. This record defines the promotion
decision for a release candidate. Passing a subset of tests is evidence for a
gate, not a substitute for an explicit release contract.

### D2 - Required gates fail closed

Every mandatory gate has a documented command, prerequisites, expected
artifacts, and non-zero failure status. A missing required script, fixture,
feature, tool, secret, or artifact is failure unless the release policy
explicitly marks that gate optional for the selected target.

`INFO`, `WARNING`, and `SKIP` output must not convert an unmet mandatory check
into success.

### D3 - Configuration round trips are executable contracts

The configuration gate must fail when its Rust round-trip test fails, when a
required fixture is missing, when fixtures are invalid, or when secrets are
silently redacted or lost. Fixture requirements must be versioned with the
configuration contract rather than described as future work while returning
zero.

### D4 - Feature matrices distinguish optional from required

The feature gate must derive or declare the required feature set and build
matrix for each release target. Missing required features and failed required
build combinations are failures. Unsupported optional combinations may be
skipped only when the target policy names them as optional.

### D5 - The aggregate gate reports only executed success

`release-gate.sh` may report release readiness only when every mandatory child
gate executed and returned success. Its report records command, status, and any
explicitly optional omissions. A green aggregate result must not be inferred
from warning text or the existence of a helper script.

### D6 - Artifact workflows verify what is published

Tag-triggered workflows add product-artifact checks: Console resources,
unified versions, packaged unified binaries, supported target archives,
signatures, updater metadata, notarization/signing, and installer acceptance.
The release workflow must depend on successful verification jobs before
publication.

## Hard invariants

- A failed mandatory child check makes the aggregate release gate fail.
- A missing mandatory fixture, feature, secret, script, or artifact is not a
  pass.
- Required feature-build failures are never reported as skipped.
- Configuration round-trip test failure is propagated to the caller.
- Release readiness is reported only after all selected mandatory gates run.
- Packaged artifacts are verified as shipped, not inferred from a debug build.
- Browser E2E and product packaging gates may run in separate workflows, but
  release publication depends on the required set for that release target.

## Authority, persistence, and failure boundaries

Release policy owns which gates are mandatory for each target. Individual
scripts own one repeatable check and its exit status. CI workflows own triggers,
matrices, credentials, artifact retention, and publication dependencies.

A gate may distinguish product failure from infrastructure failure in its
report, but both block release when the gate is mandatory. Optional gates must
be explicit policy, not inferred from a failed command.

## Current implementation gap

`scripts/tests/check-config-roundtrip.sh` propagates Rust round-trip failures
but treats missing unknown-field and secret fixtures as informational. `scripts/tests/check-feature-flags.sh`
currently treats missing required features as informational and, under
`--build`, reports failed feature combinations as skipped without failing.

Because `scripts/tests/release-gate.sh` trusts child exit codes, those scripts
can silently succeed and allow the aggregate to report success. This
record remains Proposed until mandatory checks fail closed and automated tests
exercise their failure paths.

## Consequences

- Release readiness becomes an explicit, auditable promotion decision.
- Optional target support remains possible without weakening mandatory gates.
- Script output and exit status convey the same result.
- A green release workflow means the selected artifact contracts actually ran.

## Rejected alternatives

- Treating all workspace tests as sufficient proof of release readiness.
- Logging failed mandatory checks while returning success.
- Treating missing required fixtures or features as future informational work.
- Marking failed required builds as skipped.
- Publishing artifacts after an independent verification job fails.

## Implementation and verification

- `scripts/tests/release-gate.sh`
- `scripts/tests/check-feature-flags.sh`
- `scripts/tests/check-config-roundtrip.sh`
- `scripts/tests/check-threat-model-coverage.sh`
- `scripts/release/check-desktop-signing-config.sh`
- `scripts/tests/check-desktop-release-contract.py`
- `.github/workflows/release.yml`

The current files are implementation evidence, not proof that this Proposed
decision is complete. Acceptance requires tests that force each mandatory child
gate to fail and verify that both the child and aggregate commands return
non-zero without printing a release-ready result.

## Related decisions

- ADR-0049 defines desktop packaging and updater artifacts.
- ADR-0050 defines layered test architecture and evidence placement.
