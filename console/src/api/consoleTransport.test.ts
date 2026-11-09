import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ConsoleTransport } from "./consoleTransport";

vi.mock("./config", () => ({
  getApiUrl: (p: string) => `/api${p}`,
  getApiToken: () => "test-secret",
}));
vi.mock("./authHeaders", () => ({ buildAuthHeaders: () => ({}) }));
class Socket {
  static instances: Socket[] = [];
  onopen?: () => void;
  onmessage?: (e: { data: string }) => void;
  onclose?: () => void;
  bufferedAmount = 0;
  frames: Array<Record<string, unknown>> = [];
  constructor(readonly url: string) {
    Socket.instances.push(this);
  }
  send(raw: string) {
    this.frames.push(JSON.parse(raw));
  }
  close() {
    this.onclose?.();
  }
  emit(frame: Record<string, unknown>) {
    this.onmessage?.({ data: JSON.stringify(frame) });
  }
  authenticate() {
    this.onopen?.();
    this.emit({ type: "ready", version: 1 });
  }
}
let transport: ConsoleTransport;
beforeEach(() => {
  vi.useFakeTimers();
  Socket.instances = [];
  vi.stubGlobal("WebSocket", Socket);
  transport = new ConsoleTransport(
    "ws://localhost/api/console/transport",
    "test-secret",
  );
});
afterEach(() => {
  transport.close();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe("Console shared transport", () => {
  it("multiplexes persistent streams and blocked remote reads without consuming fetch slots", async () => {
    const fetch = vi.fn();
    vi.stubGlobal("fetch", fetch);
    const pending = Array.from({ length: 6 }, (_, i) =>
      transport
        .fetch(`/agents/a/chats/${i}/console/activity`)
        .catch(() => undefined),
    );
    const snapshot = transport.fetch("/agents/a/chats/c/acp/permissions");
    expect(Socket.instances).toHaveLength(1);
    const socket = Socket.instances[0];
    expect(socket.url).not.toContain("secret");
    socket.authenticate();
    expect(socket.frames.filter((f) => f.type === "open")).toHaveLength(7);
    const id = socket.frames.at(-1)!.id;
    socket.emit({
      type: "head",
      id,
      status: 200,
      content_type: "application/json",
    });
    socket.emit({ type: "data", id, data: btoa('{"requests":[]}') });
    socket.emit({ type: "end", id });
    expect(await (await snapshot).json()).toEqual({ requests: [] });
    expect(fetch).not.toHaveBeenCalled();
    transport.close();
    await Promise.all(pending);
  });

  it("detaches one cancelled channel and ignores its late reply", async () => {
    const abort = new AbortController();
    const cancelled = transport
      .fetch("/agents/a/chats/c/console/activity", { signal: abort.signal })
      .catch((e: Error) => e);
    const remaining = transport.fetch("/agents/a/chats/d/acp/permissions");
    const socket = Socket.instances[0];
    socket.authenticate();
    abort.abort();
    expect((await cancelled).name).toBe("AbortError");
    socket.emit({ type: "head", id: "1", status: 200 });
    socket.emit({ type: "data", id: "1", data: btoa("obsolete") });
    socket.emit({ type: "head", id: "2", status: 200 });
    socket.emit({ type: "data", id: "2", data: btoa("current") });
    socket.emit({ type: "end", id: "2" });
    expect(await (await remaining).text()).toBe("current");
    expect(socket.frames).toContainEqual({ type: "cancel", id: "1" });
    expect(Socket.instances).toHaveLength(1);
  });

  it("bounds request admission and never dispatches an expired intent later", async () => {
    const result = transport
      .fetch("/agents/a/chats/c/acp/permissions")
      .catch((e: Error) => e);
    await vi.advanceTimersByTimeAsync(30001);
    expect((await result) instanceof Error).toBe(true);
    const socket = Socket.instances.at(-1)!;
    socket.authenticate();
    expect(socket.frames.filter((f) => f.type === "open")).toHaveLength(0);
  });

  it("does not replay a dispatched operation when the socket reconnects", async () => {
    const first = transport
      .fetch("/agents/a/chats/c/acp/permissions")
      .catch((e: Error) => e);
    const socket = Socket.instances[0];
    socket.authenticate();
    socket.close();
    expect((await first) instanceof Error).toBe(true);
    const second = transport
      .fetch("/agents/a/chats/d/acp/permissions")
      .catch(() => undefined);
    await vi.advanceTimersByTimeAsync(1000);
    const replacement = Socket.instances[1];
    replacement.authenticate();
    expect(
      replacement.frames.filter((f) => f.type === "open").map((f) => f.path),
    ).toEqual(["/agents/a/chats/d/acp/permissions"]);
    // An obsolete socket cannot inject data into the current generation.
    socket.emit({ type: "fatal", detail: "obsolete" });
    transport.close();
    await second;
  });

  it("isolates a slow channel with an explicit overflow error", async () => {
    const response = transport.fetch("/agents/a/chats/c/console/activity");
    const socket = Socket.instances[0];
    socket.authenticate();
    socket.emit({ type: "head", id: "1", status: 200 });
    const body = (await response).body!;
    for (let i = 0; i < 17; i++)
      socket.emit({ type: "data", id: "1", data: btoa("x".repeat(16 * 1024)) });
    await expect(body.getReader().read()).rejects.toThrow("overflow");
    expect(socket.frames).toContainEqual({ type: "cancel", id: "1" });
  });

  it("treats version/auth rejection as terminal instead of falling back to streams", async () => {
    const result = transport
      .fetch("/sync/devices/activity")
      .catch((e: Error) => e);
    const socket = Socket.instances[0];
    socket.onopen?.();
    socket.emit({ type: "ready", version: 2 });
    expect((await result) instanceof Error).toBe(true);
    await vi.advanceTimersByTimeAsync(60000);
    expect(Socket.instances).toHaveLength(1);
  });
});
