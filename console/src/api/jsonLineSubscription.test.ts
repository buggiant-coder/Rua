import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { subscribeJsonLines } from "./jsonLineSubscription";

vi.mock("./config", () => ({ getApiUrl: (path: string) => `/api${path}` }));
vi.mock("./authHeaders", () => ({ buildAuthHeaders: () => ({}) }));

function stream() {
  let controller!: ReadableStreamDefaultController<Uint8Array>;
  const cancel = vi.fn();
  const response = new Response(
    new ReadableStream<Uint8Array>({
      start(value) {
        controller = value;
      },
      cancel,
    }),
  );
  return {
    response,
    cancel,
    send: (bytes: Uint8Array) => controller.enqueue(bytes),
    close: () => controller.close(),
  };
}

describe("JSON line subscription recovery", () => {
  const stops: Array<() => void> = [];
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => {
    stops.splice(0).forEach((stop) => stop());
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("backs off network and HTTP failures to a cap, resetting only after a stable connection", async () => {
    const stable = stream();
    const fetcher = vi
      .fn()
      .mockRejectedValueOnce(new Error("offline"))
      .mockImplementation(async () => new Response(null, { status: 503 }));
    vi.stubGlobal("fetch", fetcher);
    const sub = subscribeJsonLines({ path: "/activity", onMessage: vi.fn() });
    stops.push(sub.stop);
    await vi.advanceTimersByTimeAsync(0);
    for (const delay of [1000, 2000, 4000, 8000, 10000, 10000]) {
      const calls = fetcher.mock.calls.length;
      await vi.advanceTimersByTimeAsync(delay - 1);
      expect(fetcher).toHaveBeenCalledTimes(calls);
      await vi.advanceTimersByTimeAsync(1);
      expect(fetcher).toHaveBeenCalledTimes(calls + 1);
    }
    fetcher.mockResolvedValueOnce(stable.response);
    await vi.advanceTimersByTimeAsync(10000);
    await vi.advanceTimersByTimeAsync(10000);
    stable.close();
    await vi.advanceTimersByTimeAsync(0);
    const calls = fetcher.mock.calls.length;
    await vi.advanceTimersByTimeAsync(999);
    expect(fetcher).toHaveBeenCalledTimes(calls);
    await vi.advanceTimersByTimeAsync(1);
    expect(fetcher).toHaveBeenCalledTimes(calls + 1);
    sub.stop();
    await vi.advanceTimersByTimeAsync(30000);
    expect(fetcher).toHaveBeenCalledTimes(calls + 1);
  });

  it("consumes split UTF-8, skips malformed frames, and flushes the final line on EOF", async () => {
    const source = stream();
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(source.response));
    const onMessage = vi.fn();
    stops.push(subscribeJsonLines({ path: "/activity", onMessage }).stop);
    await vi.advanceTimersByTimeAsync(0);
    const bytes = new TextEncoder().encode(
      '{"text":"中文"}\n{broken}\n{"done":true}',
    );
    source.send(bytes.slice(0, 10));
    source.send(bytes.slice(10));
    source.close();
    await vi.advanceTimersByTimeAsync(0);
    expect(onMessage.mock.calls).toEqual([
      [{ text: "中文" }],
      [{ done: true }],
    ]);
  });

  it("discards a late response from a replaced connection and stops all retries", async () => {
    const old = stream();
    const current = stream();
    let resolve!: (response: Response) => void;
    const fetcher = vi
      .fn()
      .mockImplementationOnce(
        () =>
          new Promise<Response>((r) => {
            resolve = r;
          }),
      )
      .mockResolvedValueOnce(current.response);
    vi.stubGlobal("fetch", fetcher);
    const onMessage = vi.fn();
    const onConnected = vi.fn();
    const sub = subscribeJsonLines({
      path: "/activity",
      onMessage,
      onConnected,
    });
    stops.push(sub.stop);
    sub.reconnect();
    old.send(new TextEncoder().encode('{"stale":true}\n'));
    resolve(old.response);
    await vi.advanceTimersByTimeAsync(0);
    expect(fetcher.mock.calls[0][1].signal.aborted).toBe(true);
    expect(old.cancel).toHaveBeenCalledTimes(1);
    expect(onConnected.mock.calls).toEqual([[true]]);
    expect(onMessage).not.toHaveBeenCalled();
    sub.stop();
    expect(current.cancel).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(30000);
    expect(fetcher).toHaveBeenCalledTimes(2);
  });
});

vi.mock("./consoleTransport", () => ({ consoleFetch: (path: string, options?: RequestInit) => fetch(`/api${path}`, options) }));
