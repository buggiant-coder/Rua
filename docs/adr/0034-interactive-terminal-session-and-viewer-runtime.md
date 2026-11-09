# ADR 0034: Interactive Terminal Session and Viewer Runtime

**Status:** Accepted

## Context

An interactive terminal is a long-lived PTY owned by a Device runtime. It can
have several viewers, survive route changes, and continue producing output while
a browser is hidden. It is not a Chat Turn, Agent action, ACP session, or durable
conversation transcript.

Local and remote terminals need identical product semantics. Direct endpoint
PTY spawning or a separate terminal network stack would create different
lifecycle, recovery, and authorization behavior depending on where the browser
is connected.

The current backend provides terminal authority and placement services, while
the Console provides a runtime registry and per-viewer state machine around
xterm.js.

## Decision

### D1 - Terminal Session is a Device-owned runtime aggregate

An Interactive Terminal Session has a stable terminal session ID and one
immutable Terminal Execution Device. That Device hosts the Terminal Session
Authority for the lifetime of the PTY.

Terminal input and output are not Conversation State, Message Log records,
Memory, Task output, or Stable Conversation Facts. Agent Workspaces may be cwd
choices, but they do not own the session.

On Android, the native Rua resident owns the PTY and starts PRoot with the
Linux Compatibility Runtime's shell. Runtime Authority supplies the launch plan;
Runtime Layout supplies cwd, HOME, binds, and environment. The terminal defaults
to the Device's terminal home as mapped by that layout. Desktop/headless use
their platform shell policy. Missing Android guest runtime or required toolchain
prevents session creation with diagnostics, without falling back to a host shell.

PRoot supplies Linux compatibility; the native PTY supplies terminal I/O,
foreground process control, Ctrl+C, and resize. The browser's xterm.js provides
the visible terminal. These responsibilities do not require a separate terminal
application or a packaged Android shell userland.

### D2 - Local and remote operations share one placement surface

Open, list, attach, take lease, send input, resize, detach, rename, and kill go
through one terminal service and placement contract. Local placement invokes the
authority adapter directly and must work without Sync; remote placement uses
Terminal Device Commands and Terminal Session Activity over the shared Device
Session.

No endpoint bypass may spawn a local PTY outside this authority model. Remote
failure never falls back to the controller Device.

### D3 - Terminal Session Authority owns canonical state

The authority owns PTY lifetime, process control, canonical rows and columns,
session-wide scrollback capacity, input lease, ordered byte positions, activity
journal, terminal emulator state, snapshot serialization, viewer registration,
detach timeout, and cleanup.

One authority-side xterm-headless instance consumes the same ordered PTY bytes
and geometry changes as the session. It is the canonical reconstructable screen
state and sole source of terminal-emulator protocol replies to the PTY.
On Android its Node bridge runs inside the Linux Compatibility Runtime, using
the APK's matching bridge asset and the installed Linux Node toolchain. The
bridge is bundled with the Console's pinned xterm dependencies, extracted into
APK-owned immutable resources outside the guest, and exposed by Runtime Layout.
It is resolved lazily when a session is created, so automatic installation may
finish after resident startup. Android offers one Debian profile; stale host
profile preferences do not select an Android shell.

The resident closes the Terminal Session Authority before exiting. Each Android
PTY owns a Unix session; cleanup includes its foreground and background process
groups and reaps its PRoot child. The shared Node bridge has a separate owned
process group and is stopped and reaped after sessions close. PTY descriptors are
close-on-exec so another child cannot keep an unrelated session open.

### D4 - Terminal Session Activity is one positioned stream

All output and control facts share a monotonically increasing activity sequence.
Output frames additionally identify a half-open range of original PTY bytes:

```text
{ activity_sequence, start_offset, end_offset, data }
```

Offsets count bytes, not text characters, lines, rendered cells, SSE frames, or
snapshot length. Non-duplicate output ranges are contiguous. Transport retry
preserves original positions.

### D5 - Input uses a takeable lease

Exactly one viewer holds the Terminal Input Lease. Only that viewer may write
input, publish PTY geometry, or perform lease-protected process control. Another
viewer may atomically take the lease; the prior holder becomes read-only when it
observes the ordered lease fact.

Viewer identity is a browser-local UUID plus the paired source Device identity
for remote access. The browser UUID survives refresh so reattach can preserve a
still-held lease; local-only sessions do not require a fabricated paired Device
identity.

Terminal input is byte-oriented and has no Conversation queue, passive
interjection, active interjection, or Send Now behavior.

### D6 - Terminal Viewer Runtime owns browser execution state

Each attached `{execution_device, terminal_session_id, viewer_id}` has one
long-lived Terminal Viewer Runtime. It owns browser xterm, addons, ordered
writer, applied cursor, transport subscription, recovery state, staging xterm,
and viewer-local focus, selection, scroll intent, and viewport projection.

A routed page or React component is only a view binding. Hiding, moving, or
remounting that binding does not recreate the runtime, reattach, or replay
terminal history.

Terminal activity subscriptions carry Console authentication in request headers.
The Viewer Runtime serializes input requests in typed order; it drops unsent
bytes on lease loss, recovery, disposal, or a failed send. A failed input request
is never automatically replayed because the shell may already have consumed it.

### D7 - Attach and recovery use atomic replacement state

Cold attach and recovery return one coherent state containing session metadata,
a serialized authority snapshot with geometry and cursor, an ordered tail, and
the resulting live cursor.

The browser applies snapshot and tail to an invisible staging xterm, waits for
all writes, verifies the cursor, and only then reveals or swaps it. Snapshot or
recovery bytes never enter the writer of an already visible xterm.

If a valid snapshot cannot be produced, attach or recovery fails while the old
visible terminal remains unchanged. Full retained output is not replayed through
the visible terminal as fallback.

### D8 - Live output has one acknowledged writer

After becoming live, all PTY output enters one ordered writer. The writer
accepts positioned frames after the applied cursor, coalesces bounded adjacent
output, keeps control facts ordered, and advances the cursor only after the
xterm write callback.

Byte-based watermarks detect a lagging viewer. That viewer enters snapshot
resync independently; it cannot pause PTY reading or affect other viewers.

### D9 - Viewer lifecycle does not own PTY lifecycle

Route hide or session selection may detach the DOM view while the runtime and
subscription continue. Browser suspension invalidates delayed delivery. Resume reopens the activity
stream from the retained applied cursor and preserves the existing terminal
surface and history. A cursor gap or failed transport triggers atomic snapshot
resync; visibility changes alone do not reattach a healthy session.

Explicit viewer detach does not kill the PTY. With no viewers, the authority
keeps the live session for a bounded detached TTL, whose MVP default is 10
minutes. Process exit removes it immediately; server restart does not promise
durable resume.

Explicit kill, detached TTL expiry, and resident shutdown must clean up the
session's shell, PRoot process, bridge, and owned child processes. Android shell
job control may create several process groups within one Unix session; killing
only the outer PRoot PID or its initial process group is insufficient. Cleanup
must cover those groups, reap owned children, and preserve other terminal
sessions. This lifecycle contract does not claim containment of arbitrary code
that deliberately escapes its session.

### D10 - Geometry and viewer state remain separate

Only the input lease holder's stable visible viewport controls PTY geometry.
Passive viewers retain authority rows and columns and adapt their local viewport
without resizing the shared PTY.

Focus, scroll intent, selection, accessibility mode, IME handling, and viewport
position are viewer-local. They do not trigger attach, hydration, reset, or
authority state changes. Scrollback capacity is not viewer-local; it configures
the shared authority-side emulator for the whole session.

## Hard invariants

- One terminal session has one immutable execution Device.
- A terminal session cannot switch execution Device after creation.
- One authority owns PTY, emulator, stream positions, lease, and geometry.
- Terminal data never enters Conversation State by default.
- Local and remote placement expose the same contract.
- Exactly one viewer holds input and geometry authority.
- Visible xterm receives live positioned output only.
- Snapshot and recovery tail are applied before reveal.
- A slow or disconnected viewer cannot pause the PTY.
- Local terminal operation does not require Sync or paired-device identity.

## Authority, persistence, concurrency, and failure boundaries

**Authority.** Terminal Session Authority owns process and canonical terminal
state. Placement routes commands. Viewer Runtime owns one browser projection;
view bindings own only DOM attachment and visibility.

**Persistence.** Session registry, activity journal, emulator state, snapshots,
leases, and viewers are runtime-only. Full terminal transcripts are not
persisted or synchronized. Shell-level persistence requires tools such as tmux
inside the terminal.

**Concurrency.** PTY bytes and controls enter one ordered authority stream.
Each viewer has one writer and cursor. Lease takeover serializes writers of
input and geometry without recreating the terminal.

**Failure.** Missing Device, process exit, cursor gap, invalid frame, writer
overflow, or document suspension produces explicit detach, terminal exit, or
snapshot resync. Recovery replaces state atomically and never guesses by text
overlap or visible replay.

## Consequences

- Terminal lifecycle remains independent from Chat and route lifecycle.
- Browser components become bindings over a reusable runtime service.
- Authority-side emulation provides coherent snapshots for every viewer.
- Multiple viewers can observe one PTY without conflicting resize or input.
- Recovery favors current coherent state over visual playback of backlog.

## Rejected alternatives

- Model terminal sessions as Chat or Agent state.
- Spawn local PTYs directly in HTTP handlers.
- Add a terminal-specific peer protocol outside shared Device Sessions.
- Persist full terminal transcripts by default.
- Let every viewer write input or resize the PTY.
- Hydrate or replay into an already visible xterm.
- Pause the shared PTY for a slow viewer.
- Treat route changes, focus, or lease changes as recovery events.

## Implementation and verification

Current implementation anchors include:

- `crates/rua-server/src/services/terminal.rs`;
- `crates/rua-server/src/endpoints/terminal.rs`;
- `crates/rua-server/src/services/device_command_bus.rs`;
- `console/src/terminal/runtime/viewerRuntime.ts`;
- `console/src/terminal/runtime/registry.ts`;
- `console/src/pages/Terminal/index.tsx`.

Verification must cover local and remote parity, PTY byte ordering, snapshot
cursor and tail atomicity, one writer, lag resync, browser suspension, route
hide without reattach, lease takeover, passive-viewer geometry, detach TTL,
process exit, and no Conversation writes. Android acceptance must additionally
cover guest input/output, Ctrl+C preserving the shell, `stty`-verified resize,
detach/reattach with the same shell and restored snapshot, child cleanup across
process groups, and a second terminal surviving the first terminal's closure.
Browser/WebView, soft keyboard, and IME acceptance remain separate from PTY
protocol verification. Relevant suites include
`crates/rua-server/tests/contract_terminal_http.rs`,
`crates/rua-server/tests/infra/terminal_authority.rs`,
`console/src/terminal/runtime/viewerRuntime.test.ts`, and
`console/tests/terminal-canary/xterm-compatibility.spec.ts`.

### Current implementation gap

`attach_with_scrollback` and the remote attach path currently apply the
request's `scrollback_lines` through `set_scrollback_lines` before registering
the viewer. Any viewer allowed to attach can therefore change the shared
authority-side emulator capacity, affecting every viewer's future snapshot.
The product contract must either fix scrollback at session creation or treat a
later change as a lease-protected session control; it cannot be implemented as
an independent per-viewer preference.

## Related decisions

- ADR 0019: Project Resource Root distinction.
- ADR 0002: Runtime Layout and Agent Path Contract.
- ADR 0003: Runtime Authority and external process environment.
- ADR 0007: Process lifecycle and bounded phased shutdown.
- ADR 0024: Stop, interjection, and Viewer Disconnect.
- ADR 0032: Task Runner, Watch, Control, and authority routing.
- ADR 0035: Chat CLI single-request adapter and resident server boundary.
- ADR 0037: Device Network Runtime and shared Device Sessions.
- ADR 0049: Tauri Desktop Shell and local resident server.
