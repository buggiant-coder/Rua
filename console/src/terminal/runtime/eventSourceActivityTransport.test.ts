import { afterEach, describe, expect, it, vi } from "vitest";
import { EventSourceActivityTransport } from "./eventSourceActivityTransport";

vi.mock("../../api/config", () => ({
  getApiUrl: (path: string) => `/api${path}`,
}));
vi.mock("../../api/authHeaders", () => ({
  buildAuthHeaders: () => ({ Authorization: "Bearer fixture" }),
}));

const options = {
  browserViewerId: "viewer",
  executionDeviceQueryValue: "local",
  placement: "local" as const,
  retainAuthority: true,
  sessionId: "session",
  sessionKey: "key",
};
const cursor = { next_activity_sequence: 4, next_output_offset: 9 };
afterEach(() => vi.unstubAllGlobals());

describe("authenticated terminal activity", () => {
  it("carries authorization and positioned cursor while parsing split SSE frames", async () => {
    const cancel = vi.fn();
    let body!: ReadableStreamDefaultController<Uint8Array>;
    const stream = new ReadableStream<Uint8Array>({
      start(controller) {
        body = controller;
      },
      cancel,
    });
    const fetch = vi.fn().mockResolvedValue(new Response(stream));
    vi.stubGlobal("fetch", fetch);
    const onFrame = vi.fn();
    const onError = vi.fn();
    const connection = new EventSourceActivityTransport(options).open(cursor, {
      onFrame,
      onError,
    });
    const frame = {
      type: "output",
      activity_sequence: 4,
      start_offset: 9,
      end_offset: 12,
      data_base64: "YWJj",
    };
    const bytes = new TextEncoder().encode(
      `: keepalive\n\nevent: terminal_activity\ndata: ${JSON.stringify(
        frame,
      )}\n\n`,
    );
    body.enqueue(bytes.slice(0, 47));
    body.enqueue(bytes.slice(47));
    await vi.waitFor(() => expect(onFrame).toHaveBeenCalledWith(frame));
    expect(fetch).toHaveBeenCalledTimes(1);
    const [url, init] = fetch.mock.calls[0];
    expect(url).toContain("next_activity_sequence=4&next_output_offset=9");
    expect(url).not.toContain("fixture");
    expect(init.headers.Authorization).toBe("Bearer fixture");
    connection.close();
    connection.close();
    body.close();
    expect(init.signal.aborted).toBe(true);
    expect(onError).not.toHaveBeenCalled();
  });

  it("reports denied authentication without replaying or silently reconnecting", async () => {
    const fetch = vi
      .fn()
      .mockResolvedValue(
        new Response('{"detail":"Unauthorized"}', { status: 401 }),
      );
    vi.stubGlobal("fetch", fetch);
    const onError = vi.fn();
    const connection = new EventSourceActivityTransport(options).open(cursor, {
      onFrame: vi.fn(),
      onError,
    });
    await vi.waitFor(() =>
      expect(onError).toHaveBeenCalledWith(
        expect.objectContaining({ message: expect.stringContaining("401") }),
      ),
    );
    expect(fetch).toHaveBeenCalledTimes(1);
    connection.close();
  });

  it("closing an in-flight subscription aborts it without reporting a viewer error", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(
        (_url, init) =>
          new Promise((_resolve, reject) => {
            init.signal.addEventListener("abort", () =>
              reject(new DOMException("Aborted", "AbortError")),
            );
          }),
      ),
    );
    const onError = vi.fn();
    const connection = new EventSourceActivityTransport(options).open(cursor, {
      onFrame: vi.fn(),
      onError,
    });
    connection.close();
    await Promise.resolve();
    await Promise.resolve();
    expect(onError).not.toHaveBeenCalled();
  });
});

vi.mock("../../api/consoleTransport", () => ({ consoleFetch: (path: string, options?: RequestInit) => fetch(`/api${path}`, options) }));
