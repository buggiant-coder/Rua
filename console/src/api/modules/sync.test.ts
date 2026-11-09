import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../config", () => ({
  getApiUrl: (path: string) => `http://test/api${path}`,
  getApiToken: () => "test-token",
}));

import { devicesApi, syncApi } from "./sync";

describe("devicesApi connectivity watch", () => {
  let stop: (() => void) | undefined;
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => {
    stop?.();
    stop = undefined;
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("refreshes on logical attach and events, and cancels its channel", async () => {
    let stream!: ReadableStreamDefaultController<Uint8Array>;
    const cancel = vi.fn();
    const body = new ReadableStream<Uint8Array>({
      start: (c) => {
        stream = c;
      },
      cancel,
    });
    const fetchMock = vi.fn().mockResolvedValue(new Response(body));
    vi.stubGlobal("fetch", fetchMock);
    const changed = vi.fn();
    stop = devicesApi.watchChanges(changed);
    await vi.advanceTimersByTimeAsync(0);
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/sync/devices/activity",
      expect.objectContaining({
        signal: expect.any(AbortSignal),
      }),
    );
    expect(changed).toHaveBeenCalledTimes(1);
    stream.enqueue(
      new TextEncoder().encode(
        'event: device_connectivity\ndata: {"revision":1}\n\n',
      ),
    );
    await vi.advanceTimersByTimeAsync(0);
    expect(changed).toHaveBeenCalledTimes(2);
    stop();
    await vi.advanceTimersByTimeAsync(5000);
    expect(cancel).toHaveBeenCalled();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(fetchMock.mock.calls[0][1].signal.aborted).toBe(true);
  });

  it("reconnects after failure and EOF, refreshing changes missed while disconnected", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(new Response(null, { status: 503 }))
      .mockResolvedValueOnce(new Response(""))
      .mockResolvedValue(new Response(new ReadableStream()));
    vi.stubGlobal("fetch", fetchMock);
    const changed = vi.fn();
    stop = devicesApi.watchChanges(changed);
    await vi.advanceTimersByTimeAsync(0);
    expect(changed).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1000);
    expect(changed).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(2000);
    expect(changed).toHaveBeenCalledTimes(2);
  });

  it("cancels pending retries", async () => {
    const fetchMock = vi.fn().mockRejectedValue(new TypeError("offline"));
    vi.stubGlobal("fetch", fetchMock);
    stop = devicesApi.watchChanges(vi.fn());
    await vi.advanceTimersByTimeAsync(0);
    stop();
    await vi.advanceTimersByTimeAsync(60000);
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("ignores a connection that finishes after unsubscribe", async () => {
    let resolve!: (response: Response) => void;
    const fetchMock = vi.fn().mockReturnValue(
      new Promise<Response>((r) => {
        resolve = r;
      }),
    );
    vi.stubGlobal("fetch", fetchMock);
    const changed = vi.fn();
    const cancel = vi.fn();
    stop = devicesApi.watchChanges(changed);
    stop();
    resolve(new Response(new ReadableStream({ cancel })));
    await vi.advanceTimersByTimeAsync(60000);
    expect(changed).not.toHaveBeenCalled();
    expect(cancel).toHaveBeenCalledOnce();
    expect(fetchMock).toHaveBeenCalledOnce();
  });
});

describe("syncApi power policy", () => {
  let fetchSpy: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    fetchSpy = vi.fn();
    vi.stubGlobal("fetch", fetchSpy);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("gets the device-local sync enabled config without runtime status", async () => {
    const body = { enabled: true };
    fetchSpy.mockResolvedValueOnce(
      new Response(JSON.stringify(body), { status: 200 }),
    );

    await expect(syncApi.getConfig()).resolves.toEqual(body);

    expect(fetchSpy).toHaveBeenCalledWith("http://test/api/sync/config", {
      headers: {
        "Content-Type": "application/json",
        Authorization: "Bearer test-token",
      },
    });
  });

  it("gets the device-local sync power policy", async () => {
    const body = {
      host_profile: "mobile-apk",
      stored_policy: null,
      effective_policy: {
        mode: "power_saver",
        periodic_interval_secs: 600,
        background_flush_grace_secs: 60,
        allow_background_runtime: false,
        allow_background_scheduled_sync: false,
      },
    };
    fetchSpy.mockResolvedValueOnce(
      new Response(JSON.stringify(body), { status: 200 }),
    );

    await expect(syncApi.getPowerPolicy()).resolves.toEqual(body);

    expect(fetchSpy).toHaveBeenCalledWith("http://test/api/sync/power-policy", {
      headers: {
        "Content-Type": "application/json",
        Authorization: "Bearer test-token",
      },
    });
  });

  it("updates the power policy and keeps the server-normalized response", async () => {
    const body = {
      host_profile: "mobile-apk",
      stored_policy: {
        mode: "balanced",
        periodic_interval_secs: 300,
      },
      effective_policy: {
        mode: "balanced",
        periodic_interval_secs: 300,
        background_flush_grace_secs: 60,
        allow_background_runtime: false,
        allow_background_scheduled_sync: true,
      },
    };
    fetchSpy.mockResolvedValueOnce(
      new Response(JSON.stringify(body), { status: 200 }),
    );

    await expect(
      syncApi.updatePowerPolicy({
        mode: "balanced",
        periodic_interval_secs: 300,
      }),
    ).resolves.toEqual(body);

    expect(fetchSpy).toHaveBeenCalledWith("http://test/api/sync/power-policy", {
      method: "PUT",
      headers: {
        "Content-Type": "application/json",
        Authorization: "Bearer test-token",
      },
      body: JSON.stringify({ mode: "balanced", periodic_interval_secs: 300 }),
    });
  });

  it("surfaces backend errors when saving an invalid policy", async () => {
    fetchSpy.mockResolvedValueOnce(
      new Response(
        JSON.stringify({
          detail: "custom mode requires periodic_interval_secs",
        }),
        {
          status: 400,
        },
      ),
    );

    await expect(syncApi.updatePowerPolicy({ mode: "custom" })).rejects.toThrow(
      "custom mode requires periodic_interval_secs",
    );
  });
});

vi.mock("../consoleTransport", () => ({
  consoleFetch: (path: string, options?: RequestInit) =>
    fetch(`/api${path}`, options),
}));
