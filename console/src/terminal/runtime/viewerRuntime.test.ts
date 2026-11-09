import { afterEach, describe, expect, it, vi } from "vitest";
import type { TerminalAttachResponse } from "../../api/modules/terminal";
import type { TerminalStreamFrame } from "../stream";
import {
  TerminalViewerLagError,
  TerminalViewerRuntime,
  type TerminalActivityTransport,
  type TerminalSurface,
  type TerminalSurfaceFactory,
  type TerminalSurfaceOptions,
} from "./viewerRuntime";
import { TerminalRuntimeRegistry } from "./registry";

function base64(data: string) {
  const bytes = new TextEncoder().encode(data);
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return { bytes, encoded: btoa(binary) };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, reject, resolve };
}

function outputFrame(
  data: string,
  activitySequence: number,
  startOffset: number,
): TerminalStreamFrame {
  const encoded = base64(data);
  return {
    type: "output",
    activity_sequence: activitySequence,
    start_offset: startOffset,
    end_offset: startOffset + encoded.bytes.byteLength,
    data_base64: encoded.encoded,
  };
}

class FakeSurface implements TerminalSurface {
  readonly events: string[] = [];
  mounted = false;
  revealed = false;
  writeGate: Promise<void> | null = null;
  inputEnabled = false;
  onInput?: (data: string) => void;

  constructor(readonly options: TerminalSurfaceOptions) {}

  async mount() {
    this.events.push("mount");
    this.mounted = true;
  }

  reveal() {
    this.events.push("reveal");
    this.revealed = true;
  }

  unmount() {
    this.events.push("unmount");
    this.mounted = false;
    this.revealed = false;
  }

  refresh() {
    this.events.push("refresh");
  }

  async write(data: string | Uint8Array) {
    const text =
      typeof data === "string" ? data : new TextDecoder().decode(data);
    this.events.push(`write:${this.revealed ? "visible" : "staging"}:${text}`);
    if (this.writeGate) {
      const gate = this.writeGate;
      this.writeGate = null;
      await gate;
    }
  }

  resize(cols: number, rows: number) {
    this.events.push(`resize:${cols}x${rows}`);
  }

  fit() {
    this.events.push("fit");
    return { cols: this.options.cols, rows: this.options.rows };
  }

  setLockedGeometry(locked: boolean) {
    this.events.push(`locked:${locked}`);
  }

  setScreenReaderMode(enabled: boolean) {
    this.events.push(`screen-reader:${enabled}`);
  }

  setSelectionListener() {}

  getSelection() {
    return "";
  }

  setInputHandler(enabled: boolean, handler?: (data: string) => void) {
    this.inputEnabled = enabled;
    this.onInput = handler;
  }

  getViewportMetrics() {
    return {
      baseY: 0,
      cols: this.options.cols,
      rows: this.options.rows,
      viewportY: 0,
    };
  }

  focus() {
    this.events.push("focus");
  }

  scrollToTop() {
    this.events.push("scroll:top");
  }

  scrollPages(count: number) {
    this.events.push(`scroll:pages:${count}`);
  }

  scrollToBottom() {
    this.events.push("scroll:bottom");
  }

  emitInput(data: string) {
    if (this.inputEnabled) this.onInput?.(data);
  }

  dispose() {
    this.events.push("dispose");
  }
}

class FakeActivityTransport implements TerminalActivityTransport {
  openCount = 0;
  closeCount = 0;
  handlers:
    | {
        onFrame: (frame: TerminalStreamFrame) => void;
        onError: (error: unknown) => void;
      }
    | undefined;

  open(
    _cursor: { next_activity_sequence: number; next_output_offset: number },
    handlers: {
      onFrame: (frame: TerminalStreamFrame) => void;
      onError: (error: unknown) => void;
    },
  ) {
    this.openCount += 1;
    this.handlers = handlers;
    let closed = false;
    return {
      close: () => {
        if (closed) return;
        closed = true;
        this.closeCount += 1;
      },
    };
  }

  emit(frame: TerminalStreamFrame) {
    this.handlers?.onFrame(frame);
  }

  fail(error: unknown = new Error("transport failed")) {
    this.handlers?.onError(error);
  }
}

class FakeSurfaceFactory implements TerminalSurfaceFactory {
  surfaces: FakeSurface[] = [];
  nextWriteGate: Promise<void> | null = null;

  create(options: TerminalSurfaceOptions) {
    const surface = new FakeSurface(options);
    surface.writeGate = this.nextWriteGate;
    this.nextWriteGate = null;
    this.surfaces.push(surface);
    return surface;
  }
}

function attachment(): TerminalAttachResponse {
  const snapshotData = "snapshot";
  const snapshotBytes = new TextEncoder().encode(snapshotData).byteLength;
  const tailData = "-tail";
  const tailBytes = new TextEncoder().encode(tailData).byteLength;
  return {
    session: {
      session_id: "session-a",
      cwd: "/tmp",
      profile_id: "shell",
      profile_label: "Shell",
      viewer_count: 1,
      pty_geometry: { cols: 80, rows: 24 },
    },
    snapshot: {
      data: snapshotData,
      cols: 80,
      rows: 24,
      cursor: {
        next_activity_sequence: 1,
        next_output_offset: snapshotBytes,
      },
    },
    tail: [
      outputFrame(tailData, 1, snapshotBytes),
      {
        type: "pty_geometry_changed",
        activity_sequence: 2,
        geometry: { cols: 100, rows: 30 },
      },
    ],
    live_cursor: {
      next_activity_sequence: 3,
      next_output_offset: snapshotBytes + tailBytes,
    },
  };
}

afterEach(() => {
  vi.useRealTimers();
  window.localStorage.removeItem("rua-terminal-diagnostics");
  delete window.__ruaTerminalDiagnostics;
});

describe("TerminalViewerRuntime cold attach", () => {
  it("parses snapshot and the complete ordered tail before revealing", async () => {
    const factory = new FakeSurfaceFactory();
    const runtime = new TerminalViewerRuntime({
      key: "local:session-a:viewer-a",
      scrollbackLines: 5000,
      surfaceFactory: factory,
      writerBatchDelayMs: 0,
    });
    const host = document.createElement("div");
    await runtime.bind(host);

    await runtime.hydrate(attachment());

    expect(runtime.state).toBe("live");
    expect(runtime.cursor).toEqual(attachment().live_cursor);
    expect(factory.surfaces[0].events).toEqual([
      "screen-reader:false",
      "mount",
      "write:staging:snapshot",
      "write:staging:-tail",
      "resize:100x30",
      "reveal",
    ]);
  });

  it("applies a pending locked geometry state before revealing a new surface", async () => {
    const factory = new FakeSurfaceFactory();
    const runtime = new TerminalViewerRuntime({
      key: "local:session-passive:viewer-a",
      scrollbackLines: 5000,
      surfaceFactory: factory,
      writerBatchDelayMs: 0,
    });
    runtime.setLockedGeometry(true);
    await runtime.bind(document.createElement("div"));

    await runtime.hydrate(attachment());

    expect(factory.surfaces[0].events).toContain("locked:true");
    expect(factory.surfaces[0].events.indexOf("locked:true")).toBeLessThan(
      factory.surfaces[0].events.indexOf("mount"),
    );
  });

  it("advances the live cursor only after the visible xterm write callback", async () => {
    vi.useFakeTimers();
    const factory = new FakeSurfaceFactory();
    const runtime = new TerminalViewerRuntime({
      key: "local:session-a:viewer-a",
      scrollbackLines: 5000,
      surfaceFactory: factory,
      writerBatchDelayMs: 0,
    });
    await runtime.bind(document.createElement("div"));
    await runtime.hydrate(attachment());
    const surface = factory.surfaces[0];
    let releaseWrite = () => {};
    surface.writeGate = new Promise<void>((resolve) => {
      releaseWrite = resolve;
    });
    const before = runtime.cursor;
    const live = outputFrame(
      "-live",
      before!.next_activity_sequence,
      before!.next_output_offset,
    );

    const pending = runtime.applyLiveFrame(live);
    await vi.runAllTimersAsync();
    expect(runtime.cursor).toEqual(before);

    releaseWrite();
    await pending;
    expect(runtime.cursor).toEqual({
      next_activity_sequence: before!.next_activity_sequence + 1,
      next_output_offset: live.end_offset,
    });
    expect(surface.events.at(-1)).toBe("write:visible:-live");
  });

  it("rejects a tail that does not reach the declared live cursor", async () => {
    const factory = new FakeSurfaceFactory();
    const runtime = new TerminalViewerRuntime({
      key: "local:session-a:viewer-a",
      scrollbackLines: 5000,
      surfaceFactory: factory,
      writerBatchDelayMs: 0,
    });
    const invalid = attachment();
    invalid.live_cursor = {
      next_activity_sequence: 99,
      next_output_offset: 99,
    };

    await expect(runtime.hydrate(invalid)).rejects.toThrow(
      "did not reach the authority live cursor",
    );
    expect(runtime.state).toBe("new");
    expect(factory.surfaces[0].events).toContain("dispose");
  });
});

describe("TerminalViewerRuntime visibility lifecycle", () => {
  it("resumes from the retained cursor without reattaching terminal state", async () => {
    const factory = new FakeSurfaceFactory();
    const requestAttachment = vi.fn(async () => attachment());
    const runtime = new TerminalViewerRuntime({
      key: "remote:session-a:viewer-a",
      placement: "remote",
      scrollbackLines: 5000,
      surfaceFactory: factory,
    });
    runtime.setRecoveryProvider(requestAttachment);
    await runtime.hydrate(attachment());
    const transport = new FakeActivityTransport();
    runtime.startActivity(transport);
    const cursor = runtime.cursor;

    runtime.suspend();
    await runtime.resume();

    expect(requestAttachment).not.toHaveBeenCalled();
    expect(runtime.cursor).toEqual(cursor);
    expect(runtime.state).toBe("live");
    expect(factory.surfaces).toHaveLength(1);
    expect(transport.openCount).toBe(2);
    expect(runtime.lifecycleCounters.resyncCount).toBe(0);
  });

  it("records metadata-only lifecycle, cursor, surface, and document transitions", async () => {
    window.localStorage.setItem("rua-terminal-diagnostics", "1");
    const factory = new FakeSurfaceFactory();
    const runtime = new TerminalViewerRuntime({
      key: "local:session-a:viewer-a",
      scrollbackLines: 5000,
      surfaceFactory: factory,
      writerBatchDelayMs: 0,
    });
    runtime.setRecoveryProvider(async () => attachment());
    const host = document.createElement("div");
    await runtime.bind(host);
    await runtime.hydrate(attachment());
    const transport = new FakeActivityTransport();
    runtime.startActivity(transport);
    runtime.suspend();
    await runtime.resume();
    runtime.dispose();

    const diagnostics = window.__ruaTerminalDiagnostics ?? [];
    const transitions = diagnostics
      .filter((event) => event.event === "runtime_state")
      .map((event) => event.state_after);
    expect(transitions).toEqual(
      expect.arrayContaining([
        "attaching",
        "hydrating",
        "live",
        "suspended",
        "disposed",
      ]),
    );
    expect(diagnostics).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          event: "surface_change",
          surface_action: "create",
          snapshot_cursor: attachment().snapshot.cursor,
          live_cursor: attachment().live_cursor,
        }),
        expect.objectContaining({
          event: "visibility_change",
          visibility_scope: "document",
          visible: false,
        }),
      ]),
    );
    expect(JSON.stringify(diagnostics)).not.toMatch(
      /snapshot-tail|secret|data_base64|"input"|"command"/,
    );
  });

  it("keeps independent runtimes and subscriptions live across view switches", async () => {
    const registry = new TerminalRuntimeRegistry();
    const factoryA = new FakeSurfaceFactory();
    const factoryB = new FakeSurfaceFactory();
    const runtimeA = registry.getOrCreate({
      key: "local:session-a:viewer-a",
      scrollbackLines: 5000,
      surfaceFactory: factoryA,
      writerBatchDelayMs: 0,
    });
    const runtimeB = registry.getOrCreate({
      key: "local:session-b:viewer-a",
      scrollbackLines: 5000,
      surfaceFactory: factoryB,
      writerBatchDelayMs: 0,
    });
    const transportA = new FakeActivityTransport();
    const transportB = new FakeActivityTransport();
    const hostA = document.createElement("div");
    const hostB = document.createElement("div");

    await runtimeA.bind(hostA);
    await runtimeA.hydrate(attachment());
    runtimeA.startActivity(transportA);
    await runtimeB.bind(hostB);
    await runtimeB.hydrate({
      ...attachment(),
      session: { ...attachment().session, session_id: "session-b" },
    });
    runtimeB.startActivity(transportB);

    runtimeA.unbind(hostA);
    runtimeB.unbind(hostB);
    await runtimeA.bind(hostA);
    await runtimeB.bind(hostB);

    expect(runtimeA).not.toBe(runtimeB);
    expect(transportA.openCount).toBe(1);
    expect(transportB.openCount).toBe(1);
    expect(transportA.closeCount).toBe(0);
    expect(transportB.closeCount).toBe(0);
    expect(runtimeA.lifecycleCounters.hydrationCount).toBe(1);
    expect(runtimeB.lifecycleCounters.hydrationCount).toBe(1);
    expect(runtimeA.lifecycleCounters.resyncCount).toBe(0);
    expect(runtimeB.lifecycleCounters.resyncCount).toBe(0);
  });

  it("replaces an activity transport without rehydrating the terminal surface", async () => {
    const factory = new FakeSurfaceFactory();
    const runtime = new TerminalViewerRuntime({
      key: "remote:session-a:viewer-a",
      scrollbackLines: 5000,
      surfaceFactory: factory,
      writerBatchDelayMs: 0,
    });
    await runtime.hydrate(attachment());
    const activeTransport = new FakeActivityTransport();
    const passiveTransport = new FakeActivityTransport();
    runtime.startActivity(activeTransport);

    runtime.replaceActivityTransport(passiveTransport);

    expect(activeTransport.closeCount).toBe(1);
    expect(passiveTransport.openCount).toBe(1);
    expect(runtime.lifecycleCounters.hydrationCount).toBe(1);
    expect(runtime.lifecycleCounters.resyncCount).toBe(0);
    expect(factory.surfaces).toHaveLength(1);
  });

  it("applies hidden output without visible catch-up when rebound", async () => {
    vi.useFakeTimers();
    const factory = new FakeSurfaceFactory();
    const runtime = new TerminalViewerRuntime({
      key: "local:session-a:viewer-a",
      scrollbackLines: 5000,
      surfaceFactory: factory,
      writerBatchDelayMs: 0,
    });
    const transport = new FakeActivityTransport();
    const host = document.createElement("div");
    await runtime.bind(host);
    await runtime.hydrate(attachment());
    runtime.startActivity(transport);
    runtime.unbind(host);
    const before = runtime.cursor!;
    const hidden = outputFrame(
      "-hidden",
      before.next_activity_sequence,
      before.next_output_offset,
    );

    transport.emit(hidden);
    await vi.runAllTimersAsync();
    await runtime.bind(host);

    expect(runtime.cursor?.next_output_offset).toBe(hidden.end_offset);
    expect(factory.surfaces[0].events).toContain("write:staging:-hidden");
    expect(runtime.lifecycleCounters.hydrationCount).toBe(1);
    expect(runtime.lifecycleCounters.subscriptionOpenCount).toBe(1);
  });

  it("sends rapid input in order and drops queued bytes after a failed send", async () => {
    const first = deferred<void>();
    const second = deferred<void>();
    const onInput = vi
      .fn()
      .mockImplementationOnce(() => first.promise)
      .mockImplementationOnce(() => second.promise);
    const onTransportError = vi.fn();
    const factory = new FakeSurfaceFactory();
    const runtime = new TerminalViewerRuntime({
      key: "ordered-input",
      scrollbackLines: 100,
      surfaceFactory: factory,
      onInput,
      onTransportError,
    });
    await runtime.hydrate(attachment());
    runtime.setInputEnabled(true);
    for (const data of ["first", "second", "never-send"])
      factory.surfaces[0].emitInput(data);
    expect(onInput.mock.calls.map(([data]) => data)).toEqual(["first"]);
    first.resolve();
    await vi.waitFor(() => expect(onInput).toHaveBeenCalledTimes(2));
    expect(onInput.mock.calls.map(([data]) => data)).toEqual([
      "first",
      "second",
    ]);
    second.reject(new Error("input failed"));
    await vi.waitFor(() => expect(onTransportError).toHaveBeenCalled());
    expect(onInput).toHaveBeenCalledTimes(2);
    factory.surfaces[0].emitInput("new-input");
    expect(onInput).toHaveBeenLastCalledWith("new-input");
    runtime.dispose();
  });

  it("drops unsent input when its viewer loses the input lease", async () => {
    const first = deferred<void>();
    const onInput = vi.fn(() => first.promise);
    const factory = new FakeSurfaceFactory();
    const runtime = new TerminalViewerRuntime({
      key: "lost-input-lease",
      scrollbackLines: 100,
      surfaceFactory: factory,
      onInput,
    });
    await runtime.hydrate(attachment());
    runtime.setInputEnabled(true);
    factory.surfaces[0].emitInput("in-flight");
    factory.surfaces[0].emitInput("queued");
    runtime.setInputEnabled(false);
    first.resolve();
    await Promise.resolve();
    runtime.setInputEnabled(true);
    expect(onInput).toHaveBeenCalledTimes(1);
    runtime.dispose();
  });

  it("disables inactive input and disposes runtime resources exactly once", async () => {
    const onInput = vi.fn();
    const registry = new TerminalRuntimeRegistry();
    const factory = new FakeSurfaceFactory();
    const runtime = registry.getOrCreate({
      key: "local:session-a:viewer-a",
      scrollbackLines: 5000,
      surfaceFactory: factory,
      onInput,
      writerBatchDelayMs: 0,
    });
    const transport = new FakeActivityTransport();
    await runtime.hydrate(attachment());
    runtime.startActivity(transport);
    runtime.setInputEnabled(false);
    factory.surfaces[0].emitInput("blocked");
    runtime.setInputEnabled(true);
    factory.surfaces[0].emitInput("accepted");

    registry.dispose(runtime.key);
    registry.dispose(runtime.key);

    expect(onInput).toHaveBeenCalledTimes(1);
    expect(onInput).toHaveBeenCalledWith("accepted");
    expect(transport.closeCount).toBe(1);
    expect(runtime.lifecycleCounters.subscriptionCloseCount).toBe(1);
    expect(runtime.lifecycleCounters.surfaceDisposeCount).toBe(1);
    expect(registry.get(runtime.key)).toBeUndefined();
  });

  it("creates a fresh runtime and surface after explicit disposal", async () => {
    const registry = new TerminalRuntimeRegistry();
    const factory = new FakeSurfaceFactory();
    const options = {
      key: "local:session-a:viewer-a",
      scrollbackLines: 5000,
      surfaceFactory: factory,
      writerBatchDelayMs: 0,
    };
    const first = registry.getOrCreate(options);
    await first.hydrate(attachment());

    registry.dispose(first.key);
    const second = registry.getOrCreate(options);
    await second.hydrate(attachment());

    expect(second).not.toBe(first);
    expect(factory.surfaces).toHaveLength(2);
    expect(factory.surfaces[0].events).toContain("dispose");
    expect(factory.surfaces[1].events).not.toContain("dispose");
    expect(second.lifecycleCounters.hydrationCount).toBe(1);
  });

  it("applies lease and geometry facts without hydration or resync", async () => {
    vi.useFakeTimers();
    const onFrame = vi.fn();
    const factory = new FakeSurfaceFactory();
    const runtime = new TerminalViewerRuntime({
      key: "remote:device-b:session-a:viewer-a",
      scrollbackLines: 5000,
      surfaceFactory: factory,
      onFrame,
      writerBatchDelayMs: 0,
    });
    await runtime.hydrate(attachment());
    const transport = new FakeActivityTransport();
    runtime.startActivity(transport);
    const cursor = runtime.cursor!;

    transport.emit({
      type: "input_lease_changed",
      activity_sequence: cursor.next_activity_sequence,
      holder: { browser_id: "viewer-b" },
    });
    transport.emit({
      type: "pty_geometry_changed",
      activity_sequence: cursor.next_activity_sequence + 1,
      geometry: { cols: 132, rows: 44 },
    });
    await vi.runAllTimersAsync();

    expect(factory.surfaces[0].events).toContain("resize:132x44");
    expect(onFrame).toHaveBeenCalledTimes(2);
    expect(runtime.lifecycleCounters).toMatchObject({
      hydrationCount: 1,
      resyncCount: 0,
      surfaceCreateCount: 1,
    });
    expect(transport.openCount).toBe(1);
  });
});

describe("TerminalViewerRuntime acknowledged writer", () => {
  it("batches adjacent output and keeps control activity as a hard boundary", async () => {
    vi.useFakeTimers();
    window.localStorage.setItem("rua-terminal-diagnostics", "1");
    const factory = new FakeSurfaceFactory();
    const runtime = new TerminalViewerRuntime({
      key: "local:session-batching:viewer-a",
      scrollbackLines: 5000,
      surfaceFactory: factory,
      writerBatchDelayMs: 4,
      writerBatchMaxBytes: 64,
    });
    await runtime.hydrate(attachment());
    const cursor = runtime.cursor!;
    const first = outputFrame(
      "a",
      cursor.next_activity_sequence,
      cursor.next_output_offset,
    );
    const second = outputFrame(
      "b",
      cursor.next_activity_sequence + 1,
      first.end_offset,
    );
    const geometry: TerminalStreamFrame = {
      type: "pty_geometry_changed",
      activity_sequence: cursor.next_activity_sequence + 2,
      geometry: { cols: 120, rows: 40 },
    };
    const third = outputFrame(
      "c",
      cursor.next_activity_sequence + 3,
      second.end_offset,
    );

    const pending = [
      runtime.applyLiveFrame(first),
      runtime.applyLiveFrame(second),
      runtime.applyLiveFrame(geometry),
      runtime.applyLiveFrame(third),
    ];
    await vi.runAllTimersAsync();
    await Promise.all(pending);

    expect(factory.surfaces[0].events.slice(-3)).toEqual([
      "write:staging:ab",
      "resize:120x40",
      "write:staging:c",
    ]);
    expect(runtime.writerMetrics).toMatchObject({
      acknowledgedBytes: 3,
      batchCount: 2,
      inFlightBytes: 0,
      queuedBytes: 0,
    });
    expect(window.__ruaTerminalDiagnostics).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          event: "writer_complete",
          write_callback_completed: true,
          viewport: expect.objectContaining({
            base_y: 0,
            viewport_y: 0,
          }),
        }),
      ]),
    );
  });

  it("crossing the byte high watermark drops backlog and isolates a healthy viewer", async () => {
    vi.useFakeTimers();
    const lagError = vi.fn();
    const lagFactory = new FakeSurfaceFactory();
    const lagging = new TerminalViewerRuntime({
      key: "local:lagging:viewer-a",
      scrollbackLines: 5000,
      surfaceFactory: lagFactory,
      onTransportError: lagError,
      writerBatchDelayMs: 100,
      writerHighWatermarkBytes: 6,
      writerLowWatermarkBytes: 2,
    });
    const healthyFactory = new FakeSurfaceFactory();
    const healthy = new TerminalViewerRuntime({
      key: "local:healthy:viewer-b",
      scrollbackLines: 5000,
      surfaceFactory: healthyFactory,
      writerBatchDelayMs: 0,
      writerHighWatermarkBytes: 64,
      writerLowWatermarkBytes: 16,
    });
    await lagging.hydrate(attachment());
    await healthy.hydrate(attachment());
    const lagTransport = new FakeActivityTransport();
    const healthyTransport = new FakeActivityTransport();
    lagging.startActivity(lagTransport);
    healthy.startActivity(healthyTransport);
    const lagCursor = lagging.cursor!;
    const healthyCursor = healthy.cursor!;

    lagTransport.emit(
      outputFrame(
        "1234",
        lagCursor.next_activity_sequence,
        lagCursor.next_output_offset,
      ),
    );
    lagTransport.emit(
      outputFrame(
        "5678",
        lagCursor.next_activity_sequence + 1,
        lagCursor.next_output_offset + 4,
      ),
    );
    healthyTransport.emit(
      outputFrame(
        "ok",
        healthyCursor.next_activity_sequence,
        healthyCursor.next_output_offset,
      ),
    );
    await Promise.resolve();
    await vi.runAllTimersAsync();

    expect(lagTransport.closeCount).toBe(1);
    expect(lagError).toHaveBeenCalledTimes(1);
    expect(lagError.mock.calls[0][0]).toBeInstanceOf(TerminalViewerLagError);
    expect(lagFactory.surfaces[0].events).not.toContain("write:staging:1234");
    expect(lagging.writerMetrics).toMatchObject({
      highWatermarkBytes: 6,
      lowWatermarkBytes: 2,
      queuedBytes: 0,
    });
    expect(healthyTransport.closeCount).toBe(0);
    expect(healthyFactory.surfaces[0].events).toContain("write:staging:ok");
  });

  it("records byte queue metadata without terminal contents", async () => {
    vi.useFakeTimers();
    window.localStorage.setItem("rua-terminal-diagnostics", "1");
    const factory = new FakeSurfaceFactory();
    const runtime = new TerminalViewerRuntime({
      key: "local:diagnostics:viewer-a",
      scrollbackLines: 5000,
      surfaceFactory: factory,
      writerBatchDelayMs: 0,
    });
    await runtime.hydrate(attachment());
    const cursor = runtime.cursor!;
    const secret = "writer-secret-payload";

    const pending = runtime.applyLiveFrame(
      outputFrame(
        secret,
        cursor.next_activity_sequence,
        cursor.next_output_offset,
      ),
    );
    await vi.runAllTimersAsync();
    await pending;

    const encoded = JSON.stringify(window.__ruaTerminalDiagnostics);
    expect(encoded).toContain('"queued_bytes"');
    expect(encoded).toContain('"batch_bytes"');
    expect(encoded).toContain('"acknowledged_bytes"');
    expect(encoded).not.toContain(secret);
  });

  it("bounds sustained spinner output by batch bytes instead of frame count", async () => {
    vi.useFakeTimers();
    const factory = new FakeSurfaceFactory();
    const runtime = new TerminalViewerRuntime({
      key: "local:spinner:viewer-a",
      scrollbackLines: 5000,
      surfaceFactory: factory,
      writerBatchDelayMs: 0,
      writerBatchMaxBytes: 64,
      writerHighWatermarkBytes: 2048,
      writerLowWatermarkBytes: 512,
    });
    await runtime.hydrate(attachment());
    const cursor = runtime.cursor!;
    const pending: Array<Promise<"apply" | "duplicate">> = [];
    let offset = cursor.next_output_offset;
    for (let index = 0; index < 1000; index += 1) {
      const frame = outputFrame(
        ".",
        cursor.next_activity_sequence + index,
        offset,
      );
      offset = frame.end_offset;
      pending.push(runtime.applyLiveFrame(frame));
    }

    await vi.runAllTimersAsync();
    await Promise.all(pending);

    expect(runtime.writerMetrics).toMatchObject({
      acknowledgedBytes: 1000,
      batchCount: 16,
      inFlightBytes: 0,
      queuedBytes: 0,
      receivedBytes: 1000,
    });
    expect(
      factory.surfaces[0].events.filter((event) =>
        event.startsWith("write:staging:"),
      ),
    ).toHaveLength(18);
  });
});

describe("TerminalViewerRuntime atomic resync", () => {
  it("requests remote replacement directly without a readiness preflight", async () => {
    const factory = new FakeSurfaceFactory();
    const requestAttachment = vi.fn(async () => attachment());
    const runtime = new TerminalViewerRuntime({
      key: "remote:document-resume:viewer-a",
      placement: "remote",
      scrollbackLines: 5000,
      surfaceFactory: factory,
    });
    runtime.setRecoveryProvider(requestAttachment);
    await runtime.bind(document.createElement("div"));
    await runtime.hydrate(attachment());
    const transport = new FakeActivityTransport();
    runtime.startActivity(transport);
    const oldSurface = factory.surfaces[0];

    const recovery = runtime.requestResync("transport_failure");
    await recovery;

    expect(requestAttachment).toHaveBeenCalledTimes(1);
    expect(factory.surfaces[1].events).toContain("reveal");
    expect(oldSurface.events).toContain("dispose");
  });

  it("reports an attachment failure and allows a subsequent explicit recovery", async () => {
    const factory = new FakeSurfaceFactory();
    const recoveryError = vi.fn();
    const requestAttachment = vi
      .fn<() => Promise<TerminalAttachResponse>>()
      .mockRejectedValueOnce(
        new Error("Device Network Runtime is not initialized"),
      )
      .mockResolvedValueOnce(attachment());
    const runtime = new TerminalViewerRuntime({
      key: "remote:runtime-race:viewer-a",
      placement: "remote",
      scrollbackLines: 5000,
      surfaceFactory: factory,
      onRecoveryError: recoveryError,
    });
    runtime.setRecoveryProvider(requestAttachment);
    await runtime.hydrate(attachment());
    runtime.startActivity(new FakeActivityTransport());
    const oldSurface = factory.surfaces[0];

    await expect(runtime.requestResync("transport_failure")).rejects.toThrow(
      "Device Network Runtime is not initialized",
    );
    expect(requestAttachment).toHaveBeenCalledTimes(1);
    expect(recoveryError).toHaveBeenCalledTimes(1);
    expect(oldSurface.events).not.toContain("dispose");
    await runtime.requestResync("transport_failure");
    expect(requestAttachment).toHaveBeenCalledTimes(2);
    expect(recoveryError).toHaveBeenCalledTimes(1);
  });

  it("deduplicates gap recovery and preserves the old surface until reveal", async () => {
    vi.useFakeTimers();
    const factory = new FakeSurfaceFactory();
    const replacement = deferred<TerminalAttachResponse>();
    const requestAttachment = vi.fn(() => replacement.promise);
    const runtime = new TerminalViewerRuntime({
      key: "local:gap-recovery:viewer-a",
      scrollbackLines: 5000,
      surfaceFactory: factory,
      writerBatchDelayMs: 0,
    });
    runtime.setRecoveryProvider(requestAttachment);
    const host = document.createElement("div");
    await runtime.bind(host);
    await runtime.hydrate(attachment());
    const transport = new FakeActivityTransport();
    runtime.startActivity(transport);
    const oldSurface = factory.surfaces[0];
    const cursor = runtime.cursor!;
    const gap = outputFrame(
      "gap",
      cursor.next_activity_sequence + 1,
      cursor.next_output_offset,
    );

    transport.emit(gap);
    transport.emit(gap);
    await vi.runAllTimersAsync();
    const recovery = runtime.requestResync("stream_gap");
    const redundantColdAttach = vi.fn(async () => attachment());
    const bindingAttach = runtime.ensureAttached(
      redundantColdAttach,
      transport,
    );

    expect(requestAttachment).toHaveBeenCalledTimes(1);
    expect(redundantColdAttach).not.toHaveBeenCalled();
    expect(runtime.state).toBe("resyncing");
    expect(oldSurface.events).not.toContain("dispose");

    const hydrationGate = deferred<void>();
    factory.nextWriteGate = hydrationGate.promise;
    const next = attachment();
    next.snapshot.data = "replacement";
    replacement.resolve(next);
    await Promise.resolve();
    await Promise.resolve();

    expect(factory.surfaces).toHaveLength(2);
    expect(oldSurface.events).not.toContain("dispose");
    expect(factory.surfaces[1].events).not.toContain("reveal");

    hydrationGate.resolve();
    await Promise.all([recovery, bindingAttach]);

    expect(factory.surfaces[1].events).toContain("reveal");
    expect(oldSurface.events).toContain("dispose");
    expect(transport.openCount).toBe(2);
    expect(runtime.lifecycleCounters.resyncCount).toBe(1);
  });

  it("waits for an acknowledged live write before transport recovery", async () => {
    vi.useFakeTimers();
    const factory = new FakeSurfaceFactory();
    const requestAttachment = vi.fn(async () => attachment());
    const runtime = new TerminalViewerRuntime({
      key: "local:transport-recovery:viewer-a",
      scrollbackLines: 5000,
      surfaceFactory: factory,
      writerBatchDelayMs: 0,
    });
    runtime.setRecoveryProvider(requestAttachment);
    await runtime.hydrate(attachment());
    const transport = new FakeActivityTransport();
    runtime.startActivity(transport);
    const writeGate = deferred<void>();
    factory.surfaces[0].writeGate = writeGate.promise;
    const cursor = runtime.cursor!;
    transport.emit(
      outputFrame(
        "in-flight",
        cursor.next_activity_sequence,
        cursor.next_output_offset,
      ),
    );
    await vi.runAllTimersAsync();

    transport.fail();
    const recovery = runtime.requestResync("transport_failure");
    await Promise.resolve();
    expect(requestAttachment).not.toHaveBeenCalled();

    writeGate.resolve();
    await recovery;

    expect(requestAttachment).toHaveBeenCalledTimes(1);
    expect(transport.openCount).toBe(2);
  });

  it("discards input during transport recovery and re-enables it only after recovery", async () => {
    const onInput = vi.fn();
    const factory = new FakeSurfaceFactory();
    const replacement = deferred<TerminalAttachResponse>();
    const runtime = new TerminalViewerRuntime({
      key: "local:input-recovery:viewer-a",
      scrollbackLines: 5000,
      surfaceFactory: factory,
      onInput,
      writerBatchDelayMs: 0,
    });
    runtime.setRecoveryProvider(() => replacement.promise);
    await runtime.hydrate(attachment());
    runtime.setInputEnabled(true);
    const transport = new FakeActivityTransport();
    runtime.startActivity(transport);
    factory.surfaces[0].emitInput("before");

    transport.fail();
    const recovery = runtime.requestResync("transport_failure");
    factory.surfaces[0].emitInput("during");

    replacement.resolve(attachment());
    await recovery;
    factory.surfaces[1].emitInput("after");

    expect(onInput.mock.calls.map(([data]) => data)).toEqual([
      "before",
      "after",
    ]);
  });

  it("invalidates hidden callbacks and replays from the retained cursor", async () => {
    vi.useFakeTimers();
    const factory = new FakeSurfaceFactory();
    const requestAttachment = vi.fn(async () => attachment());
    const runtime = new TerminalViewerRuntime({
      key: "local:document-resume:viewer-a",
      scrollbackLines: 5000,
      surfaceFactory: factory,
      writerBatchDelayMs: 0,
    });
    runtime.setRecoveryProvider(requestAttachment);
    await runtime.hydrate(attachment());
    const transport = new FakeActivityTransport();
    runtime.startActivity(transport);
    const cursor = runtime.cursor!;

    runtime.suspend();
    transport.emit(
      outputFrame(
        "hidden-backlog",
        cursor.next_activity_sequence,
        cursor.next_output_offset,
      ),
    );
    await vi.runAllTimersAsync();
    await runtime.resume();
    transport.emit(
      outputFrame(
        "hidden-backlog",
        cursor.next_activity_sequence,
        cursor.next_output_offset,
      ),
    );
    await vi.runAllTimersAsync();

    expect(requestAttachment).not.toHaveBeenCalled();
    expect(transport.closeCount).toBe(1);
    expect(transport.openCount).toBe(2);
    expect(factory.surfaces).toHaveLength(1);
    expect(
      factory.surfaces[0].events.filter((event) =>
        event.includes("hidden-backlog"),
      ),
    ).toHaveLength(1);
  });

  it("repeated resume reopens activity without replacing terminal history", async () => {
    const factory = new FakeSurfaceFactory();
    const requestAttachment = vi.fn(async () => attachment());
    const runtime = new TerminalViewerRuntime({
      key: "local:repeat-resume:viewer-a",
      scrollbackLines: 5000,
      surfaceFactory: factory,
    });
    runtime.setRecoveryProvider(requestAttachment);
    await runtime.bind(document.createElement("div"));
    const initial = attachment();
    initial.snapshot.data = "history";
    await runtime.hydrate(initial);
    const transport = new FakeActivityTransport();
    runtime.startActivity(transport);

    runtime.suspend();
    await runtime.resume();
    runtime.suspend();
    await runtime.resume();

    expect(requestAttachment).not.toHaveBeenCalled();
    expect(factory.surfaces).toHaveLength(1);
    expect(factory.surfaces[0].events).toContain("write:staging:history");
    expect(factory.surfaces[0].events).not.toContain("dispose");
    expect(transport.openCount).toBe(3);
    expect(transport.closeCount).toBe(2);
    expect(runtime.lifecycleCounters.resyncCount).toBe(0);
  });

  it("keeps the current surface when replacement recovery fails", async () => {
    vi.useFakeTimers();
    const onInput = vi.fn();
    const recoveryError = vi.fn();
    const factory = new FakeSurfaceFactory();
    const runtime = new TerminalViewerRuntime({
      key: "local:failed-recovery:viewer-a",
      scrollbackLines: 5000,
      surfaceFactory: factory,
      onInput,
      onRecoveryError: recoveryError,
      writerBatchDelayMs: 0,
    });
    runtime.setRecoveryProvider(async () => {
      throw new Error("snapshot unavailable");
    });
    await runtime.hydrate(attachment());
    runtime.setInputEnabled(true);
    const oldSurface = factory.surfaces[0];

    await expect(runtime.requestResync("transport_failure")).rejects.toThrow(
      "snapshot unavailable",
    );

    expect(runtime.state).toBe("live");
    expect(oldSurface.events).not.toContain("dispose");
    expect(recoveryError).toHaveBeenCalledTimes(1);
    oldSurface.emitInput("blocked-after-failure");
    expect(onInput).not.toHaveBeenCalled();
  });

  it("routes viewer lag overflow through the same resync generation", async () => {
    vi.useFakeTimers();
    const factory = new FakeSurfaceFactory();
    const requestAttachment = vi.fn(async () => attachment());
    const runtime = new TerminalViewerRuntime({
      key: "local:lag-resync:viewer-a",
      scrollbackLines: 5000,
      surfaceFactory: factory,
      writerBatchDelayMs: 100,
      writerHighWatermarkBytes: 6,
      writerLowWatermarkBytes: 2,
    });
    runtime.setRecoveryProvider(requestAttachment);
    await runtime.hydrate(attachment());
    const transport = new FakeActivityTransport();
    runtime.startActivity(transport);
    const cursor = runtime.cursor!;

    transport.emit(
      outputFrame(
        "1234",
        cursor.next_activity_sequence,
        cursor.next_output_offset,
      ),
    );
    transport.emit(
      outputFrame(
        "5678",
        cursor.next_activity_sequence + 1,
        cursor.next_output_offset + 4,
      ),
    );
    await Promise.resolve();
    await runtime.requestResync("viewer_lag");

    expect(requestAttachment).toHaveBeenCalledTimes(1);
    expect(requestAttachment).toHaveBeenCalledWith("viewer_lag");
    expect(runtime.lifecycleCounters.resyncCount).toBe(1);
    expect(transport.openCount).toBe(2);
  });
});
