import { act, renderHook } from "@testing-library/react";
import type { TouchEvent, UIEvent, WheelEvent } from "react";
import { describe, expect, it, vi } from "vitest";
import { usePullPastBottom } from "./usePullPastBottom";

function scrollElement({
  scrollTop = 0,
  scrollHeight = 100,
  clientHeight = 100,
} = {}) {
  return { scrollTop, scrollHeight, clientHeight } as HTMLElement;
}

function wheelEvent(element: HTMLElement, deltaY: number) {
  return {
    currentTarget: element,
    deltaY,
    deltaMode: window.WheelEvent.DOM_DELTA_PIXEL,
  } as WheelEvent<HTMLElement>;
}

function touchEvent(element: HTMLElement, clientY?: number) {
  return {
    currentTarget: element,
    touches: clientY === undefined ? [] : [{ clientY }],
  } as unknown as TouchEvent<HTMLElement>;
}

describe("usePullPastBottom", () => {
  it("loads after continued downward wheel input when the list is shorter than its viewport", () => {
    const onLoadMore = vi.fn();
    const { result } = renderHook(() =>
      usePullPastBottom({ enabled: true, onLoadMore }),
    );
    const element = scrollElement();

    act(() => result.current.onWheel(wheelEvent(element, 24)));
    expect(onLoadMore).not.toHaveBeenCalled();
    act(() => result.current.onWheel(wheelEvent(element, 24)));

    expect(onLoadMore).toHaveBeenCalledTimes(1);
  });

  it("does not load before a scrollable list reaches the bottom", () => {
    const onLoadMore = vi.fn();
    const { result } = renderHook(() =>
      usePullPastBottom({ enabled: true, onLoadMore }),
    );
    const element = scrollElement({
      scrollTop: 100,
      scrollHeight: 400,
      clientHeight: 200,
    });

    act(() => result.current.onWheel(wheelEvent(element, 80)));

    expect(onLoadMore).not.toHaveBeenCalled();
  });

  it("triggers only once during one wheel gesture", () => {
    const onLoadMore = vi.fn();
    const { result } = renderHook(() =>
      usePullPastBottom({ enabled: true, onLoadMore }),
    );
    const element = scrollElement();

    act(() => {
      result.current.onWheel(wheelEvent(element, 60));
      result.current.onWheel(wheelEvent(element, 60));
    });

    expect(onLoadMore).toHaveBeenCalledTimes(1);
  });

  it("does not trigger while loading or when there are no more records", () => {
    const onLoadMore = vi.fn();
    const { result } = renderHook(() =>
      usePullPastBottom({ enabled: false, onLoadMore }),
    );

    act(() => result.current.onWheel(wheelEvent(scrollElement(), 60)));

    expect(onLoadMore).not.toHaveBeenCalled();
  });

  it("loads after an upward touch pull at the bottom", () => {
    const onLoadMore = vi.fn();
    const { result } = renderHook(() =>
      usePullPastBottom({ enabled: true, onLoadMore }),
    );
    const element = scrollElement();

    act(() => {
      result.current.onTouchStart(touchEvent(element, 100));
      result.current.onTouchMove(touchEvent(element, 70));
      result.current.onTouchMove(touchEvent(element, 50));
    });

    expect(onLoadMore).toHaveBeenCalledTimes(1);
  });

  it("allows another load on the next touch gesture", () => {
    const onLoadMore = vi.fn();
    const { result } = renderHook(() =>
      usePullPastBottom({ enabled: true, onLoadMore }),
    );
    const element = scrollElement();

    act(() => {
      result.current.onTouchStart(touchEvent(element, 100));
      result.current.onTouchMove(touchEvent(element, 40));
      result.current.onTouchEnd();
      result.current.onTouchStart(touchEvent(element, 100));
      result.current.onTouchMove(touchEvent(element, 40));
    });

    expect(onLoadMore).toHaveBeenCalledTimes(2);
  });

  it("clears partial progress after scrolling away from the bottom", () => {
    const onLoadMore = vi.fn();
    const { result } = renderHook(() =>
      usePullPastBottom({ enabled: true, onLoadMore }),
    );
    const bottom = scrollElement();
    const aboveBottom = scrollElement({
      scrollTop: 90,
      scrollHeight: 300,
      clientHeight: 200,
    });

    act(() => result.current.onWheel(wheelEvent(bottom, 30)));
    act(() =>
      result.current.onScroll({
        currentTarget: aboveBottom,
      } as UIEvent<HTMLElement>),
    );
    act(() => result.current.onWheel(wheelEvent(bottom, 20)));

    expect(onLoadMore).not.toHaveBeenCalled();
  });
});
