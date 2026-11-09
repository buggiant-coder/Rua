# ADR-0036: Master identity, Device List, and pairing trust

> **Status:** Proposed

## Context

Rua needs one durable answer to three questions that are easy to conflate:
which secret defines a sync network, which key identifies a physical Device,
and which peers should appear in product surfaces. Earlier designs named a
separate `MasterIdentity`, treated Device List membership as authority, or let
features reconstruct trust from persisted peer rows.

The current implementation has no standalone `MasterIdentity` object. The
network-wide secret is `NetworkSecret`, wrapping the iroh-docs namespace secret.
Each Device separately owns a persistent Ed25519 Device Identity. The Device
List is replicated metadata for names, public keys, timestamps, known
addresses, and peer selection. Removal is represented by omission from the
latest list rather than a per-row revocation state, and possession of a row
does not prove network membership.

Pairing must therefore establish cryptographic membership before runtime
features trust a peer. A stale or copied `device_list.json` cannot grant access
to encrypted data or authenticate a Device Session.

## Decision

### D1 - NetworkSecret is the master network capability

The architecture uses `NetworkSecret` as the root capability for one sync
network. It determines the iroh-docs namespace and derives the symmetric
`SyncKey`. “Master identity” is descriptive language only; no additional
`MasterIdentity` keypair, registry, or persistence format is introduced.

### D2 - Device Identity is local and stable

Each installation generates or loads one Ed25519 Device Identity from
`data_dir/device_identity.json`. It supplies the iroh endpoint identity and the
iroh-docs author identity. It is not synchronized and is not regenerated when
the sync directory is reset.

`DeviceKey` remains a separate device-local encryption key for local secret
envelopes. Neither Device Identity nor DeviceKey replaces NetworkSecret.

### D3 - Device List is a replicated projection

`device_list.json` records known Device public keys and presentation metadata.
It supports Device settings, peer address selection, rename, removal, and
connectivity projection. It is not an access-control list for decrypting the
replica log and is not sufficient evidence that a peer belongs to the active
network.

### D4 - Pairing transfers membership through an authenticated one-shot flow

Pairing uses the short-lived PIN and PAKE-protected rendezvous channel to
exchange Device entries and network state. The joining side receives the
selected NetworkSecret only inside that encrypted exchange. The completed
design requires pairing to succeed only after the secret and Device List are
durably stored, the runtime constructs a Bound Device Network and Bound Sync
Network, and the peer receives a final commit acknowledgement.

### D5 - Runtime trust is bound, not reconstructed

Device Session authentication and Sync startup consume Bound Device Network
membership. They verify the peer Device identity and shared network proof.
Feature modules do not load NetworkSecret directly, infer trust from Device
List rows, or invent feature-specific peer authentication.

### D6 - Removal is not retroactive cryptographic revocation

Removing a Device updates the Device List and disconnects its shared session on
Devices that apply the updated list. Those runtimes then reject new
Device-Session admission and stop selecting the removed peer. This is product
routing and admission control, not cryptographic revocation: it cannot erase
data or the NetworkSecret already held by that Device. Strong revocation
requires moving remaining Devices to a new NetworkSecret and republishing
current state.

## Current implementation gap

The exchange and basic persistence path exist, but the pairing commit protocol
is not transactional. The receiving side may save `network_secret.key` before
Device List persistence and runtime rebind complete. `PairingTransaction`
currently delegates only to `commit`; it has no rollback behavior, and the
sending peer does not wait for a final acknowledgement that the receiver made
the membership durable and successfully rebound its runtimes. A later failure
can therefore leave partially committed pairing state that startup may load.

Until rollback and final acknowledgement are implemented and covered, the
pairing success invariants in this record remain proposed.

## Hard invariants

- Exactly one NetworkSecret defines the active sync namespace.
- Device Identity is device-local, stable, and distinct from NetworkSecret.
- DeviceKey is device-local and never derives network membership.
- A Device List row is never accepted as cryptographic membership proof.
- Pairing success requires durable NetworkSecret persistence and successful
  bound-runtime construction.
- Device Session peers must prove identity and Bound Device Network membership.
- Resetting `sync/` must not silently rotate `device_identity.json`.
- Applying Device removal must close or invalidate that peer's shared session
  on the applying Device.

## Authority, persistence, and failure boundaries

Identity persistence is owned by `rua-sync` identity code. Pairing
orchestration owns network-secret exchange and deterministic network selection.
Device Network Runtime owns the in-memory bound membership used by transports.
Device List persistence owns only the replicated peer projection.

Failure to pair or bind leaves the Device unpaired even if peer metadata was
written. Failure to publish a later Device List update does not revoke local
identity, and a stale list must not be promoted into authority. Lost
NetworkSecret means loss of access to that sync network unless the user has an
explicit backup; there is no central recovery service.

## Consequences

- Sync and Device Sessions share one trust root without sharing business logic.
- Device names and connectivity can evolve without rotating cryptographic keys.
- UI removal has honest limits and does not claim retroactive data revocation.
- Pairing and startup must fail closed on identity or namespace mismatch.
- Tests must distinguish metadata presence from proven membership.

## Rejected alternatives

- A second custom `MasterIdentity` layered above the iroh namespace secret.
- Treating Device List membership as decryption or session authority.
- Deriving Device Identity from NetworkSecret.
- Regenerating Device Identity whenever sync is disabled or reset.
- Per-feature loading of secrets and independent trust checks.
- Claiming that Device removal revokes data already copied to that Device.

## Implementation and verification

- `crates/rua-sync/src/identity.rs`
- `crates/rua-sync/src/pairing.rs`
- `crates/rua-sync/src/pairing_orchestrator.rs`
- `crates/rua-sync/src/device_list.rs`
- `crates/rua-device-network/src/device_list.rs`
- `crates/rua-device-network/src/runtime.rs`
- `crates/rua-device-network/src/session.rs`
- `crates/rua-server/src/endpoints/pairing.rs`
- `crates/rua-server/src/endpoints/sync_devices.rs`
- `crates/rua-server/src/sync_network.rs`
- `crates/rua-sync/tests/pairing_e2e/happy_path.rs`
- `crates/rua-sync/tests/device_list_op/device_list_ops.rs`
- `crates/rua-sync/tests/revocation/removed_device_state.rs`
- `crates/rua-server/tests/contract_sync_network.rs`

Existing tests cover Device identity, PIN behavior, Device List operations,
removal state, and selected pairing exchanges. The `wormhole_handshake` and
`pairing_e2e` suites require the `test-util` feature, while the default
workspace PR/release test command does not enable it. They also do not prove
transaction rollback or a final durable-commit acknowledgement.

## Related decisions

- ADR-0037 owns Device Network Runtime and shared Device Sessions.
- ADR-0038 owns SyncChange ordering and envelope encryption.
- ADR-0041 owns the First Sync boundary after pairing.
