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
    // The client deadline is deliberately later than the server's 30s one, so
    // an expired server operation is reported by the server, not pre-empted.
    await vi.advanceTimersByTimeAsync(30001);
    expect(Socket.instances.at(-1)?.frames ?? []).toEqual([]);
    await vi.advanceTimersByTimeAsync(10000);
    const failure = await result;
    expect(failure instanceof Error).toBe(true);
    expect((failure as Error).message).toContain("[client-deadline]");
    expect((failure as Error).message).not.toContain("[server-deadline]");
    expect(failure).toMatchObject({ code: "client_request_deadline" });
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
    // Local buffer pressure is distinguishable from server-side frame drops.
    const reader = body.getReader();
    const failure = await reader.read().catch((e: Error) => e);
    expect(failure).toBeInstanceOf(Error);
    expect((failure as Error).message).toContain("[client-buffer]");
    expect((failure as Error).message).not.toContain("[server-backlog]");
    expect(failure).toMatchObject({ code: "client_channel_overflow" });
    expect(socket.frames).toContainEqual({ type: "cancel", id: "1" });
  });

  it("preserves the server-side backlog signal instead of the client buffer one", async () => {
    const response = transport.fetch("/agents/a/chats/c/console/activity");
    const socket = Socket.instances[0];
    socket.authenticate();
    socket.emit({ type: "head", id: "1", status: 200 });
    const body = (await response).body!;
    const reader = body.getReader();
    const pending = reader.read().catch(() => undefined);
    socket.emit({
      type: "gap",
      id: "1",
      code: "server_channel_backlog",
      detail:
        "Console channel overflow [server-backlog]; the server could not flush this channel",
    });
    const failure = await reader.read().catch((e: Error) => e);
    expect(failure).toBeInstanceOf(Error);
    expect((failure as Error).message).toContain("[server-backlog]");
    expect((failure as Error).message).not.toContain("[client-buffer]");
    expect(failure).toMatchObject({ code: "server_channel_backlog" });
    await pending;
  });

  it("delivers the server deadline before the client timer pre-empts it", async () => {
    const response = transport.fetch("/agents/a/chats/c/acp/permissions");
    const socket = Socket.instances[0];
    socket.authenticate();
    socket.emit({ type: "head", id: "1", status: 200 });
    const body = (await response).body!;
    const reader = body.getReader();
    const pending = reader.read().catch(() => undefined);
    // The server gives up first; the page must surface the server's signal.
    await vi.advanceTimersByTimeAsync(30001);
    socket.emit({
      type: "error",
      id: "1",
      code: "server_request_deadline",
      detail:
        "Console request deadline exceeded [server-deadline]; the server gave up on this operation",
    });
    const failure = await reader.read().catch((e: Error) => e);
    expect(failure).toBeInstanceOf(Error);
    expect((failure as Error).message).toContain("[server-deadline]");
    expect((failure as Error).message).not.toContain("[client-deadline]");
    expect(failure).toMatchObject({ code: "server_request_deadline" });
    await pending;
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

  it("treats server-sent event endpoints as subscriptions, not bounded reads", async () => {
    // `/events` is how the Runtime publishes user notifications. It has no
    // natural end, so the bounded-read deadline must not apply: otherwise the
    // page tears the subscription down 40s after it opens and notifications
    // silently stop arriving.
    const pending = transport
      .fetch("/platform/user-notifications/events")
      .catch(() => undefined);
    const socket = Socket.instances[0];
    socket.authenticate();
    socket.emit({ type: "head", id: "1", status: 200 });
    await vi.advanceTimersByTimeAsync(120_000);
    expect(
      socket.frames.filter(
        (frame) => frame.type === "cancel" && frame.id === "1",
      ),
    ).toEqual([]);
    transport.close();
    await pending;
  });
});
