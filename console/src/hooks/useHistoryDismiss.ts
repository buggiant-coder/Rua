import { useEffect, useRef } from "react";

const activeLayerKeys: string[] = [];
let ignoredProgrammaticPopstates = 0;
let ignoreClearScheduled = false;
let ignoreResetTimer: number | null = null;

function removeLayerKey(layerKey: string): void {
  const index = activeLayerKeys.lastIndexOf(layerKey);
  if (index >= 0) activeLayerKeys.splice(index, 1);
}

function isTopLayer(layerKey: string): boolean {
  return activeLayerKeys[activeLayerKeys.length - 1] === layerKey;
}

function armIgnoredProgrammaticPopstate(): void {
  ignoredProgrammaticPopstates += 1;
  if (typeof window === "undefined") return;
  if (ignoreResetTimer != null) window.clearTimeout(ignoreResetTimer);
  ignoreResetTimer = window.setTimeout(() => {
    ignoredProgrammaticPopstates = 0;
    ignoreClearScheduled = false;
    ignoreResetTimer = null;
  }, 250);
}

function shouldIgnoreProgrammaticPopstate(): boolean {
  if (ignoredProgrammaticPopstates <= 0) return false;
  if (!ignoreClearScheduled) {
    ignoreClearScheduled = true;
    queueMicrotask(() => {
      ignoredProgrammaticPopstates = Math.max(
        0,
        ignoredProgrammaticPopstates - 1,
      );
      ignoreClearScheduled = false;
      if (ignoredProgrammaticPopstates === 0 && ignoreResetTimer != null) {
        window.clearTimeout(ignoreResetTimer);
        ignoreResetTimer = null;
      }
    });
  }
  return true;
}

export function useHistoryDismiss(open: boolean, onClose: () => void): void {
  const onCloseRef = useRef(onClose);
  useEffect(() => {
    onCloseRef.current = onClose;
  }, [onClose]);

  useEffect(() => {
    if (!open) return;
    if (typeof window === "undefined") return;

    const layerKey = `__layer_${Date.now()}_${Math.random()
      .toString(36)
      .slice(2)}`;

    // The APK sends a dismiss event instead of traversing WebView history.
    if (/ Mobile\/\d+\.\d+\.\d+$/.test(navigator.userAgent)) {
      activeLayerKeys.push(layerKey);
      const handler = (event: Event) => {
        if (!isTopLayer(layerKey)) return;
        event.stopImmediatePropagation();
        onCloseRef.current();
      };
      window.addEventListener("rua:back", handler);
      return () => {
        window.removeEventListener("rua:back", handler);
        removeLayerKey(layerKey);
      };
    }

    const prevState = window.history.state;
    window.history.pushState(
      {
        ...(prevState && typeof prevState === "object" ? prevState : {}),
        __layerKey: layerKey,
      },
      "",
    );
    activeLayerKeys.push(layerKey);

    let dismissedByBack = false;
    const handler = () => {
      if (shouldIgnoreProgrammaticPopstate()) return;
      if (!isTopLayer(layerKey)) return;
      dismissedByBack = true;
      removeLayerKey(layerKey);
      try {
        onCloseRef.current();
      } catch (err) {
        console.error("[useHistoryDismiss] onClose threw:", err);
      }
    };
    window.addEventListener("popstate", handler);

    return () => {
      window.removeEventListener("popstate", handler);
      removeLayerKey(layerKey);
      const hasLowerLayer = activeLayerKeys.length > 0;

      if (dismissedByBack) return;
      const cur = window.history.state;
      if (
        cur &&
        typeof cur === "object" &&
        (cur as { __layerKey?: string }).__layerKey === layerKey
      ) {
        if (hasLowerLayer) armIgnoredProgrammaticPopstate();
        window.history.back();
      }
    };
  }, [open]);
}
