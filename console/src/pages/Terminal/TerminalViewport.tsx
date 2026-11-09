import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ArrowDownToLine,
  ArrowUpToLine,
  ChevronDown,
  ChevronUp,
  Copy,
} from "lucide-react";
import type { TerminalAttachResponse } from "../../api/modules/terminal";
import type {
  TerminalStreamCursor,
  TerminalStreamFrame,
} from "../../terminal/stream";
import { recordTerminalDiagnostic } from "../../terminal/diagnostics";
import { EventSourceActivityTransport } from "../../terminal/runtime/eventSourceActivityTransport";
import { terminalRuntimeRegistry } from "../../terminal/runtime/registry";
import type { TerminalResyncReason } from "../../terminal/runtime/viewerRuntime";
import { isMobile } from "../../utils/isMobile";
import {
  observeTerminalDisplayPalette,
  type TerminalDisplayPalette,
  XtermTerminalSurfaceFactory,
} from "../../terminal/runtime/xtermSurface";
import "@xterm/xterm/css/xterm.css";

const TOUCH_RESIZE_STABLE_MS = 200;
const SCROLL_CONTROLS_HIDE_DELAY_MS = 2500;
const MOBILE_KEYBOARD_MIN_HEIGHT_PX = 96;

interface TerminalViewportProps {
  active?: boolean;
  sessionKey: string;
  inputEnabled?: boolean;
  lockedGeometry?: { cols: number; rows: number } | null;
  screenReaderMode?: boolean;
  scrollbackLines: number;
  onInput?: (data: string) => void | Promise<void>;
  onGeometryChange?: (
    sessionId: string,
    geometry: { cols: number; rows: number },
  ) => void;
  onPaletteChange?: (palette: TerminalDisplayPalette) => void;
  activity: {
    browserViewerId: string;
    executionDeviceQueryValue: string;
    placement: "local" | "remote";
    sessionId: string;
    onFrame: (frame: TerminalStreamFrame, cursor: TerminalStreamCursor) => void;
    onError: (error: unknown) => void;
    onRecoveryComplete: (reason: TerminalResyncReason) => void;
    onRecoveryError: (reason: TerminalResyncReason, error: unknown) => void;
    requestAttachment: (
      reason?: TerminalResyncReason,
    ) => Promise<TerminalAttachResponse>;
  };
}

export default function TerminalViewport({
  active = true,
  sessionKey,
  inputEnabled = false,
  lockedGeometry = null,
  screenReaderMode = false,
  scrollbackLines,
  onInput,
  onGeometryChange,
  onPaletteChange,
  activity,
}: TerminalViewportProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const hideScrollControlsTimerRef = useRef<number | null>(null);
  const [scrollControlsVisible, setScrollControlsVisible] = useState(false);
  const [selectionAvailable, setSelectionAvailable] = useState(false);
  const [mobileKeyboardGeometry, setMobileKeyboardGeometry] = useState<{
    cols: number;
    rows: number;
  } | null>(null);
  const [runtime] = useState(() =>
    terminalRuntimeRegistry.getOrCreate({
      key: sessionKey,
      sessionId: activity.sessionId,
      viewerId: activity.browserViewerId,
      placement: activity.placement,
      scrollbackLines,
      surfaceFactory: new XtermTerminalSurfaceFactory(),
      onInput,
    }),
  );
  const retainAuthority = activity.placement === "local" || active;
  const transport = useMemo(
    () =>
      new EventSourceActivityTransport({
        browserViewerId: activity.browserViewerId,
        executionDeviceQueryValue: activity.executionDeviceQueryValue,
        placement: activity.placement,
        retainAuthority,
        sessionId: activity.sessionId,
        sessionKey,
      }),
    [
      activity.browserViewerId,
      activity.executionDeviceQueryValue,
      activity.placement,
      activity.sessionId,
      retainAuthority,
      sessionKey,
    ],
  );
  const geometryTimerRef = useRef<number | null>(null);
  const lockedScrollTimerRef = useRef<number | null>(null);
  const initialAttachStartedRef = useRef(false);
  const effectiveVisibilityRef = useRef<boolean | null>(null);
  const lastReportedGeometryRef = useRef<{
    cols: number;
    rows: number;
  } | null>(null);
  const lastStableGeometryRef = useRef<{
    cols: number;
    rows: number;
  } | null>(null);
  const mobileKeyboardGeometryRef = useRef<{
    cols: number;
    rows: number;
  } | null>(null);
  const mobileViewportBaselineRef = useRef<{
    width: number;
    height: number;
  } | null>(null);
  const effectiveLockedGeometry = lockedGeometry ?? mobileKeyboardGeometry;
  const geometryLockedRef = useRef(Boolean(effectiveLockedGeometry));
  const previousGeometryLockedRef = useRef(Boolean(effectiveLockedGeometry));
  geometryLockedRef.current = Boolean(effectiveLockedGeometry);

  runtime.setEventHandlers({
    onFrame: activity.onFrame,
    onTransportError: activity.onError,
    onRecoveryComplete: activity.onRecoveryComplete,
    onRecoveryError: activity.onRecoveryError,
  });
  runtime.setRecoveryProvider((reason) => activity.requestAttachment(reason));

  const showScrollControlsTemporarily = useCallback(() => {
    if (hideScrollControlsTimerRef.current !== null) {
      window.clearTimeout(hideScrollControlsTimerRef.current);
    }
    setScrollControlsVisible(true);
    hideScrollControlsTimerRef.current = window.setTimeout(() => {
      hideScrollControlsTimerRef.current = null;
      setScrollControlsVisible(false);
    }, SCROLL_CONTROLS_HIDE_DELAY_MS);
  }, []);

  const scrollTerminal = useCallback(
    (position: "top" | "up" | "down" | "bottom") => {
      showScrollControlsTemporarily();
      if (position === "top") {
        runtime.scrollToTop();
      } else if (position === "bottom") {
        runtime.scrollToBottom();
      } else {
        runtime.scrollPages(position === "up" ? -1 : 1);
      }
    },
    [runtime, showScrollControlsTemporarily],
  );

  const scheduleLockedScrollToBottom = useCallback(() => {
    if (lockedScrollTimerRef.current !== null) {
      window.clearTimeout(lockedScrollTimerRef.current);
    }
    lockedScrollTimerRef.current = window.setTimeout(() => {
      lockedScrollTimerRef.current = null;
      runtime.scrollToBottom();
    }, TOUCH_RESIZE_STABLE_MS);
  }, [runtime]);

  const reportGeometry = useCallback(() => {
    if (!active || !inputEnabled || geometryLockedRef.current) return;
    const geometry = runtime.fit();
    if (!geometry) return;
    lastStableGeometryRef.current = geometry;
    const previous = lastReportedGeometryRef.current;
    if (previous?.cols === geometry.cols && previous.rows === geometry.rows) {
      return;
    }
    lastReportedGeometryRef.current = geometry;
    recordTerminalDiagnostic({
      event: "viewport_fit",
      session_key: sessionKey,
      session_id: activity.sessionId,
      viewer_id: activity.browserViewerId,
      placement: activity.placement,
      viewport: geometry,
    });
    onGeometryChange?.(activity.sessionId, geometry);
  }, [
    active,
    activity.browserViewerId,
    activity.placement,
    activity.sessionId,
    inputEnabled,
    onGeometryChange,
    runtime,
    sessionKey,
  ]);

  const scheduleGeometry = useCallback(() => {
    if (geometryTimerRef.current !== null) {
      window.clearTimeout(geometryTimerRef.current);
    }
    geometryTimerRef.current = window.setTimeout(() => {
      geometryTimerRef.current = null;
      reportGeometry();
    }, TOUCH_RESIZE_STABLE_MS);
  }, [reportGeometry]);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    void runtime.bind(host);
    return () => runtime.unbind(host);
  }, [runtime]);

  useEffect(() => {
    if (initialAttachStartedRef.current) return;
    initialAttachStartedRef.current = true;
    void runtime
      .ensureAttached(() => activity.requestAttachment(), transport)
      .then(scheduleGeometry)
      .catch((error) => {
        initialAttachStartedRef.current = false;
        activity.onError(error);
      });
  }, [activity, runtime, scheduleGeometry, transport]);

  useEffect(() => {
    runtime.replaceActivityTransport(transport);
  }, [runtime, transport]);

  useEffect(() => {
    runtime.setInputEnabled(inputEnabled, onInput);
  }, [inputEnabled, onInput, runtime]);

  useEffect(() => {
    if (!active || !inputEnabled || !onPaletteChange) return;
    return observeTerminalDisplayPalette(onPaletteChange).dispose;
  }, [active, inputEnabled, onPaletteChange]);

  useEffect(() => {
    runtime.setScreenReaderMode(screenReaderMode);
  }, [runtime, screenReaderMode]);

  useEffect(() => {
    runtime.setSelectionListener((hasSelection) => {
      setSelectionAvailable(hasSelection);
      if (hasSelection) showScrollControlsTemporarily();
    });
    return () => runtime.setSelectionListener(undefined);
  }, [runtime, showScrollControlsTemporarily]);

  useEffect(
    () => () => {
      if (hideScrollControlsTimerRef.current !== null) {
        window.clearTimeout(hideScrollControlsTimerRef.current);
      }
    },
    [],
  );

  useEffect(() => {
    const geometryLocked = Boolean(effectiveLockedGeometry);
    const wasGeometryLocked = previousGeometryLockedRef.current;
    previousGeometryLockedRef.current = geometryLocked;
    runtime.setLockedGeometry(geometryLocked);
    if (effectiveLockedGeometry) {
      lastStableGeometryRef.current = effectiveLockedGeometry;
      runtime.resize(
        effectiveLockedGeometry.cols,
        effectiveLockedGeometry.rows,
      );
      scheduleLockedScrollToBottom();
      recordTerminalDiagnostic({
        event: "viewport_fit",
        session_key: sessionKey,
        session_id: activity.sessionId,
        viewer_id: activity.browserViewerId,
        placement: activity.placement,
        viewport: effectiveLockedGeometry,
      });
      return;
    }
    if (wasGeometryLocked) {
      lastReportedGeometryRef.current = null;
    }
    scheduleGeometry();
  }, [
    activity.browserViewerId,
    activity.placement,
    activity.sessionId,
    effectiveLockedGeometry,
    runtime,
    scheduleGeometry,
    scheduleLockedScrollToBottom,
    sessionKey,
  ]);

  useEffect(() => {
    const viewport = window.visualViewport;
    if (
      !isMobile() ||
      !viewport ||
      !active ||
      !inputEnabled ||
      lockedGeometry
    ) {
      mobileViewportBaselineRef.current = null;
      mobileKeyboardGeometryRef.current = null;
      setMobileKeyboardGeometry(null);
      return;
    }
    mobileViewportBaselineRef.current = {
      width: viewport.width,
      height: viewport.height,
    };

    const clearMobileKeyboardGeometry = () => {
      if (!mobileKeyboardGeometryRef.current) return;
      mobileKeyboardGeometryRef.current = null;
      geometryLockedRef.current = false;
      setMobileKeyboardGeometry(null);
    };
    const handleViewportResize = () => {
      const baseline = mobileViewportBaselineRef.current;
      if (!baseline) return;
      const widthChanged = Math.abs(viewport.width - baseline.width) >= 1;
      if (widthChanged) {
        mobileViewportBaselineRef.current = {
          width: viewport.width,
          height: viewport.height,
        };
        clearMobileKeyboardGeometry();
        scheduleGeometry();
        return;
      }

      const activeElement = document.activeElement;
      const terminalInputFocused =
        activeElement instanceof HTMLTextAreaElement &&
        hostRef.current?.contains(activeElement);
      const coveredHeight = baseline.height - viewport.height;
      if (
        terminalInputFocused &&
        coveredHeight >= MOBILE_KEYBOARD_MIN_HEIGHT_PX
      ) {
        const geometry = lastStableGeometryRef.current;
        if (!geometry) return;
        mobileKeyboardGeometryRef.current = geometry;
        geometryLockedRef.current = true;
        setMobileKeyboardGeometry((current) =>
          current?.cols === geometry.cols && current.rows === geometry.rows
            ? current
            : geometry,
        );
        scheduleLockedScrollToBottom();
        return;
      }

      if (viewport.height >= baseline.height - MOBILE_KEYBOARD_MIN_HEIGHT_PX) {
        clearMobileKeyboardGeometry();
      } else if (!mobileKeyboardGeometryRef.current) {
        mobileViewportBaselineRef.current = {
          width: viewport.width,
          height: viewport.height,
        };
      }
      scheduleGeometry();
    };

    viewport.addEventListener("resize", handleViewportResize);
    return () => {
      viewport.removeEventListener("resize", handleViewportResize);
    };
  }, [
    active,
    inputEnabled,
    lockedGeometry,
    scheduleGeometry,
    scheduleLockedScrollToBottom,
  ]);

  useEffect(
    () => () => {
      if (lockedScrollTimerRef.current !== null) {
        window.clearTimeout(lockedScrollTimerRef.current);
      }
    },
    [],
  );

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const observer =
      typeof ResizeObserver === "undefined"
        ? null
        : new ResizeObserver(scheduleGeometry);
    observer?.observe(host);
    window.addEventListener("orientationchange", scheduleGeometry);
    document.fonts?.ready.then(scheduleGeometry).catch(() => undefined);
    return () => {
      observer?.disconnect();
      window.removeEventListener("orientationchange", scheduleGeometry);
      if (geometryTimerRef.current !== null) {
        window.clearTimeout(geometryTimerRef.current);
        geometryTimerRef.current = null;
      }
    };
  }, [scheduleGeometry]);

  useEffect(() => {
    const applyVisibility = () => {
      const visible = !document.hidden;
      const previous = effectiveVisibilityRef.current;
      effectiveVisibilityRef.current = visible;
      if (previous === visible) return;
      if (!visible) {
        runtime.suspend();
      } else if (previous === false) {
        void runtime.resume().catch(activity.onError);
      }
    };
    const suspendForHostLifecycle = () => {
      effectiveVisibilityRef.current = false;
      runtime.suspend();
    };
    const resumeForHostLifecycle = () => {
      if (document.hidden) return;
      const wasVisible = effectiveVisibilityRef.current;
      effectiveVisibilityRef.current = true;
      runtime.refresh();
      if (wasVisible === false) {
        void runtime.resume().catch(activity.onError);
      }
    };
    applyVisibility();
    document.addEventListener("visibilitychange", applyVisibility);
    window.addEventListener("pagehide", suspendForHostLifecycle);
    window.addEventListener("pageshow", resumeForHostLifecycle);
    window.addEventListener("rua:background", suspendForHostLifecycle);
    window.addEventListener("rua:foreground", resumeForHostLifecycle);
    return () => {
      document.removeEventListener("visibilitychange", applyVisibility);
      window.removeEventListener("pagehide", suspendForHostLifecycle);
      window.removeEventListener("pageshow", resumeForHostLifecycle);
      window.removeEventListener("rua:background", suspendForHostLifecycle);
      window.removeEventListener("rua:foreground", resumeForHostLifecycle);
    };
  }, [activity.onError, runtime]);

  useEffect(
    () => () => {
      runtime.suspend();
    },
    [runtime],
  );

  useEffect(() => {
    recordTerminalDiagnostic({
      event: "visibility_change",
      session_key: sessionKey,
      session_id: activity.sessionId,
      viewer_id: activity.browserViewerId,
      placement: activity.placement,
      visibility_scope: "session",
      visible: active,
    });
  }, [
    active,
    activity.browserViewerId,
    activity.placement,
    activity.sessionId,
    sessionKey,
  ]);

  return (
    <div
      ref={hostRef}
      className="rua-terminal-xterm"
      hidden={!active}
      onKeyDownCapture={(event) => {
        if (
          [
            "PageUp",
            "PageDown",
            "Home",
            "End",
            "ArrowUp",
            "ArrowDown",
          ].includes(event.key)
        ) {
          showScrollControlsTemporarily();
        }
      }}
      onPointerDown={(event) => {
        if (event.pointerType !== "touch") runtime.focus();
      }}
      onTouchMoveCapture={showScrollControlsTemporarily}
      onWheelCapture={showScrollControlsTemporarily}
    >
      <div
        className={`rua-terminal-scroll-controls${
          scrollControlsVisible ? " is-visible" : ""
        }`}
        role="toolbar"
        aria-label="终端滚动导航"
        aria-hidden={!scrollControlsVisible}
        onMouseDown={(event) => event.stopPropagation()}
        onPointerDown={(event) => event.stopPropagation()}
        onClick={(event) => event.stopPropagation()}
      >
        {selectionAvailable && (
          <button
            type="button"
            className="rua-terminal-scroll-control"
            aria-label="复制终端选区"
            title="复制终端选区"
            tabIndex={scrollControlsVisible ? 0 : -1}
            onClick={() => {
              const selection = runtime.getSelection();
              if (selection && navigator.clipboard) {
                void navigator.clipboard.writeText(selection);
              }
            }}
          >
            <Copy size={16} aria-hidden="true" />
          </button>
        )}
        <button
          type="button"
          className="rua-terminal-scroll-control"
          aria-label="滚动到最上方"
          title="滚动到最上方"
          tabIndex={scrollControlsVisible ? 0 : -1}
          onClick={() => scrollTerminal("top")}
        >
          <ArrowUpToLine size={16} aria-hidden="true" />
        </button>
        <button
          type="button"
          className="rua-terminal-scroll-control"
          aria-label="往前滚一屏"
          title="往前滚一屏"
          tabIndex={scrollControlsVisible ? 0 : -1}
          onClick={() => scrollTerminal("up")}
        >
          <ChevronUp size={16} aria-hidden="true" />
        </button>
        <button
          type="button"
          className="rua-terminal-scroll-control"
          aria-label="往下滚一屏"
          title="往下滚一屏"
          tabIndex={scrollControlsVisible ? 0 : -1}
          onClick={() => scrollTerminal("down")}
        >
          <ChevronDown size={16} aria-hidden="true" />
        </button>
        <button
          type="button"
          className="rua-terminal-scroll-control"
          aria-label="滚动到最下方"
          title="滚动到最下方"
          tabIndex={scrollControlsVisible ? 0 : -1}
          onClick={() => scrollTerminal("bottom")}
        >
          <ArrowDownToLine size={16} aria-hidden="true" />
        </button>
      </div>
    </div>
  );
}
