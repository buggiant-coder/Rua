import { useCallback, useEffect, useRef } from "react";
import type {
  TouchEvent as ReactTouchEvent,
  UIEvent as ReactUIEvent,
  WheelEvent as ReactWheelEvent,
} from "react";

const BOTTOM_TOLERANCE_PX = 2;
const PULL_THRESHOLD_PX = 48;
const WHEEL_GESTURE_IDLE_MS = 250;

type PullPastBottomOptions = {
  enabled: boolean;
  onLoadMore: () => void;
};

function isAtBottom(element: HTMLElement) {
  return (
    element.scrollHeight - element.scrollTop - element.clientHeight <=
    BOTTOM_TOLERANCE_PX
  );
}

function wheelDistance(event: ReactWheelEvent<HTMLElement>) {
  if (event.deltaMode === WheelEvent.DOM_DELTA_LINE) return event.deltaY * 16;
  if (event.deltaMode === WheelEvent.DOM_DELTA_PAGE) {
    return event.deltaY * event.currentTarget.clientHeight;
  }
  return event.deltaY;
}

export function usePullPastBottom({
  enabled,
  onLoadMore,
}: PullPastBottomOptions) {
  const distanceRef = useRef(0);
  const triggeredRef = useRef(false);
  const touchYRef = useRef<number | null>(null);
  const wheelIdleTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const resetGesture = useCallback((allowAnotherTrigger = true) => {
    distanceRef.current = 0;
    if (allowAnotherTrigger) triggeredRef.current = false;
  }, []);

  const addDistance = useCallback(
    (element: HTMLElement, distance: number) => {
      if (!enabled || triggeredRef.current) return;
      if (!isAtBottom(element) || distance <= 0) {
        resetGesture();
        return;
      }

      distanceRef.current += distance;
      if (distanceRef.current < PULL_THRESHOLD_PX) return;

      triggeredRef.current = true;
      distanceRef.current = 0;
      onLoadMore();
    },
    [enabled, onLoadMore, resetGesture],
  );

  const onWheel = useCallback(
    (event: ReactWheelEvent<HTMLElement>) => {
      const distance = wheelDistance(event);
      if (wheelIdleTimerRef.current) clearTimeout(wheelIdleTimerRef.current);
      wheelIdleTimerRef.current = setTimeout(
        () => resetGesture(),
        WHEEL_GESTURE_IDLE_MS,
      );
      addDistance(event.currentTarget, distance);
    },
    [addDistance, resetGesture],
  );

  const onTouchStart = useCallback(
    (event: ReactTouchEvent<HTMLElement>) => {
      touchYRef.current = event.touches[0]?.clientY ?? null;
      resetGesture();
    },
    [resetGesture],
  );

  const onTouchMove = useCallback(
    (event: ReactTouchEvent<HTMLElement>) => {
      const nextY = event.touches[0]?.clientY;
      if (nextY === undefined || touchYRef.current === null) return;
      const distance = touchYRef.current - nextY;
      touchYRef.current = nextY;
      addDistance(event.currentTarget, distance);
    },
    [addDistance],
  );

  const onTouchEnd = useCallback(() => {
    touchYRef.current = null;
    resetGesture();
  }, [resetGesture]);

  const onScroll = useCallback(
    (event: ReactUIEvent<HTMLElement>) => {
      if (!isAtBottom(event.currentTarget)) resetGesture();
    },
    [resetGesture],
  );

  useEffect(() => {
    if (!enabled) resetGesture(false);
  }, [enabled, resetGesture]);

  useEffect(
    () => () => {
      if (wheelIdleTimerRef.current) clearTimeout(wheelIdleTimerRef.current);
    },
    [],
  );

  return { onWheel, onTouchStart, onTouchMove, onTouchEnd, onScroll };
}
