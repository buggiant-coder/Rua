import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useHistoryDismiss } from "./useHistoryDismiss";

describe("useHistoryDismiss", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    window.history.replaceState({ route: "chat" }, "", "/");
  });

  it("dismisses only the top native layer without navigating history", () => {
    vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
      "Android Mobile/0.1.0",
    );
    const pushSpy = vi.spyOn(window.history, "pushState");
    const backSpy = vi.spyOn(window.history, "back");
    const outerClose = vi.fn();
    const innerClose = vi.fn();
    const outer = renderHook(() => useHistoryDismiss(true, outerClose));
    const inner = renderHook(() => useHistoryDismiss(true, innerClose));

    act(() => window.dispatchEvent(new Event("rua:back")));
    expect(innerClose).toHaveBeenCalledTimes(1);
    expect(outerClose).not.toHaveBeenCalled();
    inner.unmount();

    act(() => window.dispatchEvent(new Event("rua:back")));
    expect(outerClose).toHaveBeenCalledTimes(1);
    outer.unmount();
    act(() => window.dispatchEvent(new Event("rua:back")));
    expect(outerClose).toHaveBeenCalledTimes(1);
    expect(pushSpy).not.toHaveBeenCalled();
    expect(backSpy).not.toHaveBeenCalled();
    expect(window.history.state).toEqual({ route: "chat" });
  });

  it("does not pop native history when a layer closes through its button", () => {
    vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
      "Android Mobile/0.1.0",
    );
    const pushSpy = vi.spyOn(window.history, "pushState");
    const backSpy = vi.spyOn(window.history, "back");
    const onClose = vi.fn();
    const layer = renderHook(({ open }) => useHistoryDismiss(open, onClose), {
      initialProps: { open: true },
    });
    layer.rerender({ open: false });
    act(() => window.dispatchEvent(new Event("rua:back")));
    expect(onClose).not.toHaveBeenCalled();
    expect(pushSpy).not.toHaveBeenCalled();
    expect(backSpy).not.toHaveBeenCalled();
    layer.unmount();
  });

  it("keeps browser history dismissal for iPhone Safari", () => {
    vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
      "Mozilla/5.0 (iPhone) AppleWebKit/605.1.15 Mobile/15E148 Safari/604.1",
    );
    const pushSpy = vi.spyOn(window.history, "pushState");
    const onClose = vi.fn();
    const layer = renderHook(() => useHistoryDismiss(true, onClose));
    expect(pushSpy).toHaveBeenCalledTimes(1);
    act(() =>
      window.dispatchEvent(new PopStateEvent("popstate", { state: {} })),
    );
    expect(onClose).toHaveBeenCalledTimes(1);
    layer.unmount();
  });

  it("does nothing while the layer is closed", () => {
    const pushSpy = vi.spyOn(window.history, "pushState");
    const backSpy = vi.spyOn(window.history, "back");

    const { unmount } = renderHook(() => useHistoryDismiss(false, vi.fn()));
    unmount();

    expect(pushSpy).not.toHaveBeenCalled();
    expect(backSpy).not.toHaveBeenCalled();
  });

  it("pushes a layer sentinel and closes on back/popstate without cleanup back", () => {
    const onClose = vi.fn();
    const pushSpy = vi.spyOn(window.history, "pushState");
    const backSpy = vi.spyOn(window.history, "back");

    const { unmount } = renderHook(() => useHistoryDismiss(true, onClose));

    expect(pushSpy).toHaveBeenCalledTimes(1);
    const pushedState = pushSpy.mock.calls[0][0] as Record<string, unknown>;
    expect(pushedState.route).toBe("chat");
    expect(typeof pushedState.__layerKey).toBe("string");

    act(() => {
      window.dispatchEvent(new PopStateEvent("popstate", { state: {} }));
    });
    expect(onClose).toHaveBeenCalledTimes(1);

    unmount();
    expect(backSpy).not.toHaveBeenCalled();
  });

  it("programmatic close pops its own sentinel when it is still on top", () => {
    const backSpy = vi
      .spyOn(window.history, "back")
      .mockImplementation(vi.fn());

    const { rerender } = renderHook(
      ({ open }) => useHistoryDismiss(open, vi.fn()),
      { initialProps: { open: true } },
    );

    expect(window.history.state.__layerKey).toEqual(expect.any(String));

    rerender({ open: false });

    expect(backSpy).toHaveBeenCalledTimes(1);
  });

  it("uses the latest onClose callback without pushing another sentinel", () => {
    const firstClose = vi.fn();
    const secondClose = vi.fn();
    const pushSpy = vi.spyOn(window.history, "pushState");

    const { rerender } = renderHook(
      ({ onClose }) => useHistoryDismiss(true, onClose),
      { initialProps: { onClose: firstClose } },
    );
    rerender({ onClose: secondClose });

    act(() => {
      window.dispatchEvent(new PopStateEvent("popstate", { state: {} }));
    });

    expect(pushSpy).toHaveBeenCalledTimes(1);
    expect(firstClose).not.toHaveBeenCalled();
    expect(secondClose).toHaveBeenCalledTimes(1);
  });

  it("only closes the topmost layer on back/popstate", () => {
    const outerClose = vi.fn();
    const innerClose = vi.fn();

    const outer = renderHook(() => useHistoryDismiss(true, outerClose));
    const inner = renderHook(() => useHistoryDismiss(true, innerClose));

    act(() => {
      window.dispatchEvent(new PopStateEvent("popstate", { state: {} }));
    });

    expect(innerClose).toHaveBeenCalledTimes(1);
    expect(outerClose).not.toHaveBeenCalled();

    inner.unmount();
    outer.unmount();
  });

  it("does not close the lower layer when a top layer closes programmatically", async () => {
    const outerClose = vi.fn();
    vi.spyOn(window.history, "back").mockImplementation(vi.fn());

    const outer = renderHook(() => useHistoryDismiss(true, outerClose));
    const inner = renderHook(({ open }) => useHistoryDismiss(open, vi.fn()), {
      initialProps: { open: true },
    });

    inner.rerender({ open: false });

    act(() => {
      window.dispatchEvent(new PopStateEvent("popstate", { state: {} }));
    });
    await Promise.resolve();

    expect(outerClose).not.toHaveBeenCalled();

    inner.unmount();
    outer.unmount();
  });
});
