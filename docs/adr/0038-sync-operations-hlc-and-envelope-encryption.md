# ADR-0038: Sync operations, HLC ordering, and envelope encryption

> **Status:** Accepted

## Context

Rua synchronizes portable domain state through untrusted local replica stores,
peer transports, and public relay infrastructure. Transport encryption protects
connections, but it does not keep data opaque once bytes are stored by
iroh-docs or iroh-blobs. Concurrent Devices also need deterministic ordering
without relying on perfectly synchronized wall clocks.

The replicated unit must carry enough information for a domain adapter to
validate and apply a change, while keeping canonical business state in domain
stores. It must also support retry, duplicate delivery, record replacement,
diagnostics, and tombstones without turning the replica log into another
business database.

## Decision

### D1 - SyncChange is the Sync operation envelope

The current wire/application unit is `SyncChange`, with schema version,
operation id, domain, record id, kind, Device id, HLC, optional merge base,
optional source timing, and a domain payload. “Sync operation” refers to this
immutable published fact, not to a mutable queue item or a filesystem event.

The `op_id` identifies one publication for idempotency and cross-stage
correlation. The pair `(domain, record_id)` identifies the logical replicated
record. For each iroh-docs author, a newer publication for that pair replaces
that author's current replica entry; publications from different Device authors
remain independently addressable until domain merge selects the canonical
result.

### D2 - Replica keys hide business identifiers

The iroh-docs key is
`HMAC-SHA256(SyncKey, domain || NUL || record_id)`. Domain and record
identifiers are not placed on the wire as plaintext paths. The encrypted value
contains the complete `SyncChange`, including its fresh `op_id`. Older
`HMAC(..., op_id)` descriptions do not match the current implementation.

`CONTEXT.md` still contains the older `HMAC(SyncKey, op_id)` formula. That is a
shared-document consistency gap to be corrected by the task that owns
`CONTEXT.md`; this ADR records the implemented formula and does not redefine it
locally.

Each replica entry contains one change. Batch APIs may append several entries
with bounded concurrency, but they do not pack unrelated records into one
opaque value.

### D3 - HLC orders competing versions

Every SyncChange carries an `HlcTimestamp` consisting of wall time, a logical
counter, and Device identity. Within one process lifetime, the current clock
keeps locally created values monotonic when wall time stalls or moves backward.
The fixed-width encoding provides deterministic total comparison, including
ties between Devices.

The implementation does not merge an observed remote clock into the local
clock, so this timestamp must not be described as a complete causal clock. It
is a merge-ordering mechanism, not proof of authority, delivery, or causality
for Conversation execution. Domain adapters still define whether a record uses
whole-record LWW, field merge, union, three-way text merge, or a terminal delete
rule.

### D4 - NetworkSecret derives SyncKey

The active NetworkSecret derives one 32-byte SyncKey with HKDF-SHA256. SyncKey
is held in memory and is not persisted as a separate file. DeviceKey remains a
different local key for device-local secret envelopes.

### D5 - Envelope Cipher protects stored replication content

Before a SyncChange or sync-owned blob enters iroh-docs or iroh-blobs, it is
encrypted with ChaCha20-Poly1305 under SyncKey and a fresh nonce. Decryption and
schema validation happen before routing to a domain adapter.

This application envelope is mandatory even though iroh connections are
already encrypted. QUIC/Noise protects transport; Envelope Cipher protects
replicated content at rest in transport-managed stores.

### D6 - Large domain payloads use the same trust boundary

Adapters may represent large file content with encrypted blob references
instead of inline bytes. Blob payloads use the same SyncKey-derived envelope
boundary. A hash proves content identity, not authorization to read plaintext.

Current encrypted replica entries are also durable GC roots for referenced
media and skill-file blobs. Each GC cycle reconstructs those references from
the current namespace entries, alongside the existing Docs content roots;
process-local temporary tags alone do not establish that lifetime. If an
entry needed to establish reachability cannot be read or decrypted, that GC
cycle aborts before sweeping. Replaced and orphaned payloads remain eligible
for collection after ordinary temporary protections expire.

### D7 - Local canonical state remains outside the log

The opaque replica is a replication journal and current-record exchange
surface. Domain files and databases remain canonical. Inbound application
becomes authoritative only after the owning adapter commits canonical state and
the sidecar transaction records the operation and cursor.

## Current implementation gap

The accepted design requires one Device's locally generated HLC values to stay
monotonic across process restarts. The server currently initializes its HLC
clock from `None` and does not persist or resume the last emitted timestamp.
After a restart combined with wall-clock rollback, the next local timestamp may
sort before a timestamp emitted by the same Device before shutdown.

## Hard invariants

- Every published SyncChange has a schema version and unique `op_id`.
- Every logical record has a stable `(domain, record_id)` identity.
- Local HLC generation is monotonic within a running process. Cross-restart
  monotonicity remains an implementation gap; comparison still includes Device
  identity as a deterministic tie-breaker.
- SyncKey is derived from NetworkSecret and is not DeviceKey.
- SyncChange and sync-owned blob plaintext never enter iroh stores unencrypted.
- Replica keys do not contain plaintext domain paths or record ids.
- One replica frame contains exactly one SyncChange; batch APIs never pack
  unrelated records into one frame.
- A Device author's current entry key is stable for one `(domain, record_id)`,
  so a newer publication replaces that author's older value at that key.
- Duplicate delivery of the same `op_id` is idempotent.
- Transport success alone does not mean canonical domain apply succeeded.

## Authority, persistence, and failure boundaries

`rua-sync` owns SyncChange encoding, HLC behavior, key derivation, envelope
cryptography, and opaque replica storage. Domain adapters own payload meaning,
merge validation, and canonical persistence. The Sync Sidecar Store owns local
idempotency and version cursors.

An encryption, decryption, schema, or content-read failure stops that inbound
operation before domain mutation. A domain write failure leaves its cursor
unadvanced so the inbox can retry. Loss of NetworkSecret makes encrypted
network content unreadable; possession of Device Identity or Device List data
does not recover it.

## Consequences

- Relay and storage providers cannot inspect portable state.
- Record replacement bounds active replica entries while `op_id` preserves
  application idempotency and diagnostics.
- Clock skew does not make merge ordering nondeterministic.
- Domains can choose merge semantics without changing transport encryption.
- Key loss and network rotation are explicit product-level events.

## Rejected alternatives

- Relying only on iroh transport encryption.
- A per-recipient encrypted envelope and recipient list for every change.
- Plaintext path-shaped replica keys.
- Bare wall-clock timestamps as versions.
- Treating the replica log as canonical domain storage.
- Packing unrelated records into one entry for throughput.
- Reusing DeviceKey as the network SyncKey.

## Implementation and verification

- `crates/rua-sync/src/opaque_replica_log.rs`
- `crates/rua-sync/src/hlc.rs`
- `crates/rua-sync/src/crypto.rs`
- `crates/rua-sync/src/identity.rs`
- `crates/rua-sync/src/merge_store.rs`
- `crates/rua-sync/tests/hlc/`
- `crates/rua-sync/tests/contract_entity_roundtrip/opaque_replica_log.rs`
- `crates/rua-sync/tests/sync_benchmarks.rs`
- `crates/rua-server/tests/contract_sync_network.rs`

Existing tests cover process-local HLC monotonicity and ordering, ciphertext
round trips, and selected replica-log contracts. They do not cover HLC resume
after process restart. `contract_entity_roundtrip` and `sync_benchmarks` require
the `test-util` feature and are not run by the default workspace PR/release test
command.

## Related decisions

- ADR-0036 defines NetworkSecret and Device trust.
- ADR-0039 defines domain adapters and publication.
- ADR-0040 defines idempotency and cursor persistence.
- ADR-0042 defines incremental inbound processing and repair.
