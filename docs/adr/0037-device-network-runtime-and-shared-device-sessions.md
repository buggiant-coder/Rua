# ADR-0037: Device Network Runtime and shared Device Sessions

> **Status:** Accepted
> **Amended:** 2026-11-05 — manager-owned connection lifecycle; cold-start acceptance verified 2026-11-06.

## Context

Cross-device Chat control, Terminal viewing, notifications, file transfer, and
durable Sync all need peer reachability. Giving each feature its own endpoint,
address resolver, handshake, retry loop, and connectivity state creates
duplicate connections and contradictory answers about whether a Device is
reachable.

The transport owner must also survive feature growth. Pairing establishes
trust, but it should not eagerly connect every paired Device. Sync needs its own
iroh-docs and blob protocols, while interactive commands and streams need a
shared authenticated peer session. These are related consumers of one network
runtime, not one combined Sync protocol.

Cold-start diagnosis exposed two different ownership failures. A per-peer dial
lock serializes callers but does not give the manager ownership of the dial:
a short-lived request can still cancel establishment for everyone. Separately,
Console subscriptions and remote waits can exhaust the WebView's HTTP/1.1 pool
and delay local reads. This ADR owns Device-to-Device sessions; ADR-0052 owns
the independent Console-to-resident transport. Fixing either layer alone does
not establish the other layer's responsiveness.

## Decision

### D1 - One process owns one Device Network Runtime

`DeviceNetworkRuntime` is the process-wide owner of Device Identity, Bound
Device Network membership, the iroh Endpoint and Router, mDNS, Home Relay,
Gossip, peer address resolution, Device Session management, connectivity
projection, diagnostics, and shutdown.

Business modules receive narrow session or transfer access. They do not receive
raw Endpoint ownership and do not create feature-local network runtimes.

Peers are addressed by Endpoint ID. The shared runtime uses iroh's concurrent
DNS and HTTPS Pkarr discovery alongside mDNS, and lets iroh select relay/direct
paths and perform hole punching. A cached DNS relay address is a candidate,
not an exclusive route: HTTPS discovery must remain able to supply the peer's
current signed record. In particular, iroh 1.0.0-rc.0's N0 preset needs an
explicit PkarrResolver (upstream added it to N0 in
[PR #4412](https://github.com/n0-computer/iroh/pull/4412)). Consumers must not
introduce manual-IP requirements or their own address-refresh/dial loops.

Public discovery publishes the Home Relay and automatically observed IPv6
candidates in the global-unicast range, excluding documentation ranges. It
never publishes IPv4, loopback, link-local, or unique-local interface addresses.
This allows an Endpoint-ID lookup to supply direct candidates before an initial
relay handshake. Iroh still validates candidates and falls back to relaying;
publication is not proof that a route is reachable or fresh after restart.

This is an explicit address-privacy trade-off accepted on 2026-11-05: anyone
who obtains an Endpoint ID can resolve its published IPv6 address. Endpoint IDs
are public identities, not access secrets. Admission must reject an unpaired
public key even when the caller knows the correct destination ID and address;
paired peers must additionally prove Bound Device Network membership. This
policy does not weaken those checks or make local HTTP services public.

A path selected by an older transport connection must not pin a new handshake
after a peer silently restarts. New handshakes try current discovery candidates
as they arrive, without waiting for an obsolete path's retransmission timer.
The pinned iroh dependency carries a bounded pre-connection handshake patch
for this behavior (see `vendor/iroh/RUA-PATCHES.md` and upstream
[issue #4600](https://github.com/n0-computer/iroh/issues/4600)). This does not
replay business commands or change established-stream path selection. Remove
the patch after an upstream replacement passes the same restart and delayed
discovery contract tests.

### D2 - Sync is a consumer, not the network owner

Sync attaches iroh-docs and iroh-blobs behavior to Device Network Runtime and
uses its peer route knowledge. Sync Runtime owns replication, Sync Cycles,
publication, and inbound apply. It does not own general Device presence,
Gossip, mDNS, Home Relay, or Device Sessions.

Sync's Docs/Blobs protocols and Gossip retain their protocol connections on
the shared Endpoint. They do not become Device Session streams or borrow an
interactive session connection. Interactive Bulk Transfer remains a Device
Session stream under D5.

### D3 - One shared logical Device Session exists per active peer

The Device Session Manager lazily establishes at most one active logical
session for a remote Device. Pairing does not create an eager full mesh.
Simultaneous dials converge deterministically on one winning connection
generation while the losing generation drains for a bounded interval.

The logical session survives replaceable physical connections. A transport
failure, failed network-change probe, or winning path replacement retires the
stale generation; a single logical operation timing out does not establish
that the transport is stale. Stream-specific cursors or offsets decide how
each operation resumes.

"One session" means one admitted authenticated connection generation, not a
requirement to merge Docs/Blobs/Gossip protocols into that connection. A bounded
candidate/draining overlap remains necessary for simultaneous incoming dials
and connection replacement.

### D4 - The session authenticates transport once

Session establishment verifies the remote endpoint identity, maps it to the
paired Device, proves shared Bound Device Network membership, and negotiates
protocol version and capabilities. Logical streams reuse that authenticated
session.

Conversation epochs, submission ids, Terminal input leases, transfer targets,
and other business invariants remain owned and checked by their domains.

### D5 - Logical stream classes remain independent

The `rua-device-session/1` protocol carries independent Command, Live
Activity, and Bulk Transfer streams. Stream failure is isolated: a rejected
command does not close live viewing, and a failed transfer does not rebuild the
session.

Interactive command and live traffic take priority over bulk transfer. Bulk
transfer uses bounded chunks, acknowledged offsets, and bounded resume.

### D6 - Recovery is bounded and non-durable

A user operation may wait through a short reconnect and retry an idempotent
request. If no usable session is established, it fails explicitly. Device
Session does not persist an offline command queue and never executes stale
foreground intent after a Device later reconnects.

Each caller has an explicit finite operation deadline, including its wait for
session admission. The manager's discovery, transport, and authentication
bounds are separate from that deadline and do not silently extend it. Joining
an attempt, queuing for stream admission, or retrying does not reset the
caller's deadline. An expired or cancelled caller is removed before dispatch;
finishing shared establishment later cannot execute its abandoned intent.

A broken connection after dispatch does not prove that a mutation failed.
Only domain-defined idempotency/reconciliation permits replay; transport does
not promise exactly-once execution. Notification delivery retains ADR-0047's
one-shot best-effort behavior even while another consumer reconnects.

Protocol incompatibility fails with an upgrade-required error; there is no
legacy per-feature transport fallback.

### D7 - Connectivity is a shared transport projection

Device settings and cross-device features consume one Session-owned
connectivity projection and diagnostics. The current public state is
`connected`, `connecting`, or `not_connected`; protocol-version incompatibility
is retained as a diagnostic/error while the peer remains `not_connected`.
Diagnostics also expose path class and RTT when known. None of these values
proves Conversation Authority, history completeness, queue state, or Sync
convergence.

### D8 - The manager owns one establishment attempt per peer

All session consumers acquire through Device Session Manager. For one paired
peer and current identity/membership generation, at most one outgoing
establishment attempt performs resolution, dialing, and authentication at a
time. Concurrent callers observe that attempt's progress and outcome instead
of becoming successive owners of a request-scoped dial future.

The attempt is a manager-owned cancellable task. Cancelling or timing out one
waiter only detaches that waiter while other demand remains. A usable incoming
session completes admission for all waiters. Waking waiters is separate from
retiring competing candidates: simultaneous cross-dials use the same endpoint-ID
ordering at both ends, preferring the connection initiated by the lower endpoint
ID. An authenticated candidate participating in that arbitration may finish its
bounded handshake even after waiters have acquired the incoming session. Losing
connections continue serving already-dispatched peer streams for the bounded
drain interval, then close; a pre-dispatch generation change may reacquire, but
an ambiguously delivered command is never replayed. Late candidate failure must
not downgrade the selected session. Before installing a connection,
the manager rechecks pairing, identity/membership generation, and shutdown.

No global session-registry lock is held across network I/O. Slow establishment
for one peer cannot block local operations or another peer. Admission queues,
concurrent streams, and buffered bytes have finite bounds; overload fails or
applies backpressure without opening parallel replacement connections.

### D9 - Demand and host lifecycle bound recovery

An outstanding operation or a live remote-view/transfer consumer expresses
interest in a peer. A visible remote viewer may retain interest while an
individual snapshot query times out, so recovery does not depend on a
reconnect button or on repeated short queries becoming new dial owners.
The manager alone schedules retries with bounded attempts and capped backoff;
concurrent consumer failures cannot each start another retry loop. Retryable
transport failures may recover while interest remains; trust or version
failures wait for the relevant state to change.

When the last consumer releases interest, cancel pending establishment and
retry work. The only exception is the bounded candidate arbitration above when
an authenticated incoming session is already serving the peer; it is not a new
retry or a mobile residency lease. An established idle connection may be reused only within a bounded
idle-retention policy; it does not itself retain a mobile Foreground Operation
lease. Pairing and background Sync do not manufacture interactive-session
interest. Runtime acquisition observes the host-owned readiness transition:
consumers wait for initialization rather than treating an empty runtime cell as
a permanent connection failure. Retained views observe runtime replacement as
well as session connectivity; a view that failed before a manager existed must
reattach when the resident runtime becomes available. Host suspension, disabling networking, unpairing, and shutdown cancel
the affected task subtree and fence late results, following ADR-0043.

Diagnostics distinguish caller wait expiry from attempt failure. Bounded
production logs identify the peer, attempt/connection generation, stage,
elapsed time, selected direct/Relay path, and retry/cancellation reason. They
exclude credentials and business payloads and do not require packet-level
tracing to explain a cold-start failure.

## Hard invariants

- One process has at most one Device Network Runtime.
- One active logical Device Session exists per local/remote Device pair.
- One manager-owned outgoing establishment attempt exists per peer generation.
- A caller's timeout cannot cancel establishment still needed by other consumers.
- Obsolete attempts cannot install sessions after revocation or shutdown.
- Pairing never implies an eagerly connected full mesh.
- Sync does not own Device network presence.
- Sync is not carried as a Device Session logical stream.
- Business modules do not dial raw iroh endpoints.
- Session authentication never replaces domain-specific authorization.
- Reconnect and draining are bounded; no operation becomes a durable outbox.
- Failure in one logical stream does not invalidate unrelated streams.
- Version incompatibility must fail explicitly and must not be reported as a
  successful connection.

## Authority, persistence, and failure boundaries

Device Network Runtime is the authority for transport lifecycle and shared peer
connectivity. Device Session Manager is the authority for connection
generation, authentication, capability negotiation, and stream admission.
Feature authorities remain responsible for command meaning and recovery.

Sessions, connection generations, RTT, and live connectivity are runtime-only.
Device Identity and paired-device metadata persist through their owning stores,
but an open session is never restored as durable state. A network failure
produces bounded operation failure or delayed Sync; it does not create a new
business queue or elect a different authority.

## Consequences

- Cross-device features share connection setup and route observations.
- Sync remains independently recoverable through its replica and inbox model.
- Mobile suspension can stop one complete network runtime.
- Diagnostics become consistent across Device settings and interactive uses.
- Session protocol evolution requires explicit capability and version tests.

## Rejected alternatives

- One endpoint and retry loop per feature.
- A shared dial mutex whose in-flight connection is owned by a business request.
- Fixing cancellation ownership only by increasing every request timeout.
- Treating a single physical connection for all iroh protocols as the objective.
- Eager sessions to every paired Device.
- Routing commands or bulk data through Gossip.
- Carrying durable Sync as a Device Session stream.
- Exposing raw Endpoint handles to Chat, Terminal, or notification code.
- Persisting commands for delivery after an offline Device returns.
- Keeping legacy feature-specific ALPN protocols as fallback.

## Implementation and verification

- `crates/rua-device-network/src/runtime.rs`
- `crates/rua-device-network/src/session.rs`
- `crates/rua-device-network/src/peer_directory.rs`
- `crates/rua-device-network/src/gossip.rs`
- `crates/rua-server/src/services/device_session_access.rs`
- `crates/rua-server/src/sync_peer_directory.rs`
- `crates/rua-server/src/services/sync_transfer_tools.rs`
- `crates/rua-device-network/tests/runtime.rs`
- `crates/rua-device-network/tests/contract_session.rs`
- `crates/rua-sync/tests/contract_device_network/`

The listed suites cover the runtime and session contracts at different layers.
`contract_device_network` requires the `test-util` feature and is not exercised
by the default workspace PR/release test command, so it must not be cited as a
default-gate guarantee for every shared-session behavior.

### Conformance gaps and required acceptance

The 2026-11-05 amendment is normative, not an implementation-completion claim.
The implementation now moves establishment/recovery into manager-owned tasks,
retains viewer demand across short attach deadlines, and preserves caller total
deadlines. Contract tests cover shared admission, cancellation, incoming wakeup,
and simultaneous-dial arbitration. The earlier preparation/operation budget
experiment is removed. Physical-device acceptance is recorded separately; unit
or contract tests alone do not establish cold-start acceptance.

Required verification includes:

- Delayed real transport/authentication with concurrent Command, Live Activity,
  and Bulk consumers: one physical outgoing attempt; short waiter expiry and
  cancellation of the first caller leave the other consumers able to connect.
- Incoming-session admission wakes all waiters; a later obsolete dial failure
  cannot downgrade it. No expired command is sent after the session becomes ready.
- Last-interest release, unpairing, identity changes, host suspension, and
  shutdown stop attempts/retries and prevent late installation.
- Persistent viewer recovery, bounded queues/backoff, one slow peer alongside
  a healthy peer, mutation delivery uncertainty, and one-shot notifications.
- Repeated native APK cold starts on different networks with normal page
  activity, correlated logs from both peers, and separate local-read acceptance
  from ADR-0052. Success after minutes of failed attempts is not a passing run.

## Related decisions

- ADR-0036 defines Device identity, membership, Device List, and pairing trust.
- ADR-0038 defines Sync operations without making Sync the network owner.
- ADR-0043 defines lifecycle-driven Device Network presence.
- ADR-0044 uses Device Session for Live, checkpoint, and authority-adoption
  traffic while keeping durable hydration on Sync.
- ADR-0047 uses the shared session for remote notifications.
- ADR-0052 owns Console transport multiplexing and local-first read isolation.
