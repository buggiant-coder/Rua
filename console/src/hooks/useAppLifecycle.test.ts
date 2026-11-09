import { renderHook } from "@testing-library/react";
import { vi, describe, it, expect, beforeEach, afterEach } from "vitest";

vi.mock("../api/config", () => ({
  getApiUrl: (path: string) => `http://test${path}`,
}));
vi.mock("../api/authHeaders", () => ({
  buildAuthHeaders: () => ({ Authorization: "Bearer test-token" }),
}));

import { useAppLifecycle } from "./useAppLifecycle";

describe("useAppLifecycle", () => {
  let fetchSpy: ReturnType<typeof vi.fn>;
  let listeners: Record<string, EventListener>;

  beforeEach(() => {
    fetchSpy = vi.fn().mockResolvedValue(new Response());
    vi.stubGlobal("fetch", fetchSpy);
    listeners = {};
    vi.spyOn(document, "addEventListener").mockImplementation(
      (event, handler) => {
        listeners[event] = handler as EventListener;
      },
    );
    vi.spyOn(document, "removeEventListener").mockImplementation((event) => {
      delete listeners[event];
    });
    Object.defineProperty(document, "visibilityState", {
      value: "visible",
      writable: true,
      configurable: true,
    });
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("fires POST on mount (initial foreground)", () => {
    renderHook(() => useAppLifecycle());

    expect(fetchSpy).toHaveBeenCalledTimes(1);
    const [url, opts] = fetchSpy.mock.calls[0];
    expect(url).toBe("http://test/platform/lifecycle");
    expect(opts.method).toBe("POST");
    expect(JSON.parse(opts.body)).toEqual({ event: "foreground", scope: "document" });
  });

  it("fires POST when visibilitychange -> visible", () => {
    renderHook(() => useAppLifecycle());
    fetchSpy.mockClear();

    listeners["visibilitychange"]?.(new Event("visibilitychange"));

    expect(fetchSpy).toHaveBeenCalledTimes(1);
  });

  it("does NOT fire POST when visibilitychange -> hidden", () => {
    renderHook(() => useAppLifecycle());
    fetchSpy.mockClear();

    Object.defineProperty(document, "visibilityState", {
      value: "hidden",
      writable: true,
      configurable: true,
    });
    listeners["visibilitychange"]?.(new Event("visibilitychange"));

    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it("mount visible + visibilitychange visible = 2 total calls", () => {
    renderHook(() => useAppLifecycle());
    listeners["visibilitychange"]?.(new Event("visibilitychange"));

    expect(fetchSpy).toHaveBeenCalledTimes(2);
  });

  it("cleans up listener on unmount", () => {
    const { unmount } = renderHook(() => useAppLifecycle());
    unmount();

    expect(listeners["visibilitychange"]).toBeUndefined();
  });
});
