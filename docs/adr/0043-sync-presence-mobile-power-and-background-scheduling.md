# ADR-0043: Sync presence, mobile power, and background scheduling

> **Status:** Accepted

## Context

Mobile operating systems do not permit Rua to behave like an unrestricted
always-on server without explicit battery cost. Earlier policy focused on Sync
frequency while leaving the iroh endpoint, relay, Gossip, and feature sessions
resident. That distinction is artificial because all of those resources create
Device reachability and background work.

The system must distinguish a short Sync Cycle from the longer-lived Sync
Runtime and from the process-wide Device Network Runtime. Android lifecycle,
foreground work, and system scheduling must decide when those runtimes are
available without changing desktop or headless behavior.

## Decision

### D1 - Device Network Runtime owns presence

Externally reachable network presence consists of mDNS, Home Relay, Gossip,
Device Sessions, and the shared iroh endpoint/router. It belongs to Device
Network Runtime. Sync Runtime is a consumer that owns Docs/Blobs replication,
triggers, cycles, publication, and inbound apply.

Stopping only the Sync scheduler is not sufficient to suspend mobile presence.

### D2 - Host profile determines the default policy

`mobile-apk`, `desktop`, and `headless` are explicit host profiles. Mobile APK
defaults to Power Saver. Desktop and headless remain process-running unless
explicit configuration or platform power integration says otherwise.

WebUI visibility, browser tab state, and desktop window minimization are not
mobile lifecycle authorities.

### D3 - Android shell owns foreground/background state

The Android shell reports lifecycle transitions to the resident server. The
server does not infer foreground state from process ancestry, paths, ports,
User-Agent, or WebView request traffic. The APK-owned native service presents
one branded foreground notification and observes the authenticated operation
stream. It owns one wake/Wi-Fi lock pair and the discovery multicast lock,
releasing them on suspension, host loss, or service destruction. Activity
recreation does not own or restart the resident. A new resident generation
invalidates earlier lifecycle acknowledgements and receives the latest state.

Document visibility and page mounting are refresh hints, explicitly scoped as
`document` in lifecycle requests. They cannot report host foreground/background,
rebind the resident network, or reset its network paths. Native shell lifecycle
requests remain the authority for those transitions.

Entering foreground rebinds Device Network Runtime first, then restarts Sync
consumers, drains or repairs local inbound state as needed, and triggers an
app-resume Sync Cycle.

### D4 - Mobile policy is mode-first

The supported modes are Power Saver, Balanced, Realtime, and Custom. The
backend normalizes them into concrete values including periodic interval,
background flush grace, background runtime permission, and scheduled-sync
permission.

Power policy is Device-local sync bookkeeping. It is never replicated because
battery, lifecycle, and host constraints differ by Device.

### D5 - Background Flush Grace precedes suspension

After the Mobile APK enters background without eligible active work, pending
local publication receives a bounded grace window, defaulting to 60 seconds.
Power Saver and Balanced then stop the complete Device Network Runtime unless
the effective policy allows residency. This also applies before Sync pairing:
no pairing does not exempt Device Network Runtime or Android locks from the
grace deadline, and suspension does not create a paired Sync state.

The resulting Warm Suspend may retain local process state, but the Device is
not reachable through mDNS, Relay, Gossip, Device Session, periodic Sync, or
inbound apply.

Warm Suspend preserves durable network membership. Status and diagnostics use
the persisted namespace binding together with the matching NetworkSecret when
no live node exists; they must not present suspension as lost pairing or revive
networking to answer a read. Operations still require the live bound runtime.

### D6 - Foreground Operation leases protect active work

Reference-counted leases may retain background residency for active
Conversation execution, a foreground-visible remote Chat viewer, a
foreground-visible remote Terminal viewer, or active Airdrop transfer.

An idle detached terminal, an approval wait without a visible remote viewer, or
a hidden viewer does not retain a lease indefinitely. Releasing the final lease
while backgrounded starts a fresh grace period.

### D7 - Balanced scheduling uses Android system policy

Balanced mode may request low-frequency background Sync through the Android
scheduler only when `allow_background_scheduled_sync` is true. The worker first
reads effective policy and invokes the server's scheduled-sync endpoint.

A scheduled cycle does not imply permanent network residency. If policy
disallows the work, the job exits without overriding runtime suspension.

### D8 - Power Saver has no remote wake channel

After Device Network Suspension, another Device attempts the normal bounded
Device Session route and reports not connected when it fails. Rua does not
add FCM/GCM, a custom wake service, or a durable command queue. Users who need
an idle phone to remain remotely reachable select Realtime.

## Hard invariants

- Device Network presence and Sync scheduling are separate concepts.
- Mobile lifecycle comes from the Android shell.
- Power policy is Device-local and never synchronized.
- Power Saver and Balanced stop the complete network runtime after grace unless
  policy or an active lease permits residency.
- Foreground resume rebinds networking before feature reattachment.
- Scheduled Sync cannot override `allow_background_scheduled_sync`.
- A detached PTY alone cannot keep the Mobile APK resident.
- No out-of-band remote wake or offline command queue is introduced.
- Desktop/headless behavior is not inferred from WebUI visibility.

## Authority, persistence, and failure boundaries

Android shell owns lifecycle truth. `SyncRuntimeController` owns runtime state
transitions. Device Network Runtime owns actual presence. The foreground
operation registry owns temporary residency claims. Sync power policy storage
owns normalized Device-local configuration, while Android's scheduler owns when
a permitted background job is offered execution time.

Suspension is an intentional unavailable state, not proof that a Device is
powered off. Failed background work remains recoverable on foreground resume,
periodic retry, or manual Sync. A lifecycle acknowledgement must not wait for a
full Sync Cycle, but viewer recovery can wait for the runtime readiness
projection rather than using feature-local blind timers.

Explicitly disabling Sync drains its scheduler and inbound apply work, shuts
down the Device Network Runtime, and closes both Sync state database handles
before acknowledging completion. Closed database handles reject writes;
disabled diagnostics do not reopen storage. Re-enabling Sync opens the current
`sync/` directory again. Users may delete that directory after disabling
Sync and establish a new network without restarting the application. Device
Identity and canonical business data outside `sync/` remain intact.

## Consequences

- Default mobile behavior favors battery over continuous freshness.
- Realtime remains an explicit user choice for remote reachability.
- Active foreground work can finish without every feature managing its own
  Android service or wake lock.
- Desktop and server deployments retain predictable process-running semantics.
- Tests must cover lifecycle ordering, leases, grace, and scheduled-policy
  enforcement together.

## Rejected alternatives

- Treating Sync frequency as the entire power model.
- Leaving an idle iroh endpoint resident in Power Saver.
- Inferring Android lifecycle from WebView visibility.
- Letting each feature own foreground services or wake locks.
- Keeping a detached terminal alive as an implicit residency claim.
- Adding push wake specifically for Device Session commands.
- Replicating a phone's power policy to other Devices.

## Implementation and verification

- `crates/rua-sync/src/power_policy.rs`
- `crates/rua-server/src/sync_runtime_controller.rs`
- `crates/rua-server/src/services/foreground_operation.rs`
- `crates/rua-server/src/endpoints/platform.rs`
- `crates/rua-server/src/endpoints/sync.rs`
- `mobile/mobile-shell/app/src/main/kotlin/com/rua/mobile/SyncBackgroundWorker.kt`
- `mobile/mobile-shell/app/src/main/kotlin/com/rua/mobile/ForegroundOperationMonitor.kt`
- `mobile/mobile-shell/app/src/test/kotlin/com/rua/mobile/SyncBackgroundWorkerTest.kt`
- `mobile/mobile-shell/app/src/test/kotlin/com/rua/mobile/MainActivityLifecycleTest.kt`

Verification covers mode normalization, background grace, final-lease release,
scheduled-sync rejection, foreground rebind ordering, retained-session probing,
complete suspension, and desktop/headless isolation.

## Related decisions

- ADR-0037 assigns all Device presence to Device Network Runtime.
- ADR-0042 defines due Repair Scan behavior in permitted windows.
- ADR-0044 defines live viewer leases and foreground recovery.
- ADR-0047 remains best effort when a target Device is suspended.
