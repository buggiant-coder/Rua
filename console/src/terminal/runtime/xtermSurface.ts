import { FitAddon } from "@xterm/addon-fit";
import { SerializeAddon } from "@xterm/addon-serialize";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal, type ITheme } from "@xterm/xterm";
import type {
  TerminalSurface,
  TerminalSurfaceFactory,
  TerminalSurfaceOptions,
} from "./viewerRuntime";
import { installXtermCompatibility } from "./xtermCompatibility";
import { recordTerminalDiagnostic } from "../diagnostics";

const TERMINAL_FONT_FAMILY = [
  '"Noto Sans Mono CJK SC"',
  '"Noto Sans Mono CJK"',
  '"Source Han Mono SC"',
  '"Sarasa Mono SC"',
  '"WenQuanYi Micro Hei Mono"',
  '"Droid Sans Mono"',
  "ui-monospace",
  "SFMono-Regular",
  "Menlo",
  "Monaco",
  "Consolas",
  '"Liberation Mono"',
  "monospace",
].join(", ");
const COMPOSITION_VIEW_MARGIN_PX = 4;
const TOUCH_SELECTION_HOLD_MS = 400;
const TOUCH_GESTURE_THRESHOLD_PX = 8;
const TOUCH_INERTIA_FRICTION_PX_PER_MS_SQUARED = 0.004;
const TOUCH_INERTIA_MAX_VELOCITY_PX_PER_MS = 1.5;
const TOUCH_INERTIA_MIN_VELOCITY_PX_PER_MS = 0.05;
const TERMINAL_SMOOTH_SCROLL_DURATION_MS = 120;
const FALLBACK_TERMINAL_THEME = {
  background: "#242424",
  cursor: "#e9ecef",
  foreground: "#e9ecef",
} as const;

export interface TerminalDisplayPalette {
  foreground: string;
  background: string;
  cursor: string;
}

interface TerminalPageNavigationTarget {
  attachCustomKeyEventHandler(handler: (event: KeyboardEvent) => boolean): void;
}

export function installTerminalPageNavigation(
  terminal: TerminalPageNavigationTarget,
  scrollPages: (pageCount: number) => void,
) {
  terminal.attachCustomKeyEventHandler((event) => {
    if (
      event.type !== "keydown" ||
      event.altKey ||
      event.ctrlKey ||
      event.metaKey ||
      event.shiftKey
    ) {
      return true;
    }

    const isPageKey = event.key === "PageUp" || event.key === "PageDown";
    if (!isPageKey) return true;

    event.preventDefault();
    scrollPages(event.key === "PageUp" ? -1 : 1);
    return false;
  });
}

function clampCompositionView(
  container: HTMLElement,
  textarea?: HTMLTextAreaElement | null,
) {
  const view = container.querySelector<HTMLElement>(".composition-view");
  if (!view?.classList.contains("active")) return;
  const screen =
    container.querySelector<HTMLElement>(".xterm-screen") ??
    container.querySelector<HTMLElement>(".xterm") ??
    container;
  const width = screen.clientWidth || screen.getBoundingClientRect().width;
  if (width <= 0) return;
  const maxWidth = Math.max(1, width - COMPOSITION_VIEW_MARGIN_PX * 2);
  view.style.maxWidth = `${maxWidth}px`;
  const currentLeft = Number.parseFloat(view.style.left) || view.offsetLeft;
  const viewWidth = Math.min(
    Math.max(view.scrollWidth, view.offsetWidth),
    maxWidth,
  );
  if (viewWidth <= 0) return;
  view.style.left = `${Math.max(
    COMPOSITION_VIEW_MARGIN_PX,
    Math.min(currentLeft, width - COMPOSITION_VIEW_MARGIN_PX - viewWidth),
  )}px`;
  if (textarea) {
    textarea.style.left = view.style.left;
    textarea.style.width = `${Math.max(viewWidth, 1)}px`;
  }
}

function syncCompositionPosition(
  terminal: Terminal,
  container: HTMLElement,
  textarea?: HTMLTextAreaElement | null,
) {
  const view = container.querySelector<HTMLElement>(".composition-view");
  const screen = container.querySelector<HTMLElement>(".xterm-screen");
  if (!view || !screen || !textarea) return;
  const bounds = screen.getBoundingClientRect();
  if (
    bounds.width <= 0 ||
    bounds.height <= 0 ||
    terminal.cols <= 0 ||
    terminal.rows <= 0
  ) {
    return;
  }
  const buffer = terminal.buffer.active;
  const cellWidth = bounds.width / terminal.cols;
  const cellHeight = bounds.height / terminal.rows;
  view.style.left = `${
    Math.min(buffer.cursorX, terminal.cols - 1) * cellWidth
  }px`;
  view.style.top = `${
    Math.min(buffer.cursorY, terminal.rows - 1) * cellHeight
  }px`;
  view.style.height = `${cellHeight}px`;
  view.style.lineHeight = `${cellHeight}px`;
  view.style.fontFamily = terminal.options.fontFamily ?? TERMINAL_FONT_FAMILY;
  view.style.fontSize = `${terminal.options.fontSize}px`;
  textarea.style.left = view.style.left;
  textarea.style.top = view.style.top;
  textarea.style.width = `${Math.max(cellWidth, 1)}px`;
  textarea.style.height = `${Math.max(cellHeight, 1)}px`;
  textarea.style.lineHeight = `${cellHeight}px`;
  clampCompositionView(container, textarea);
}

export function readTerminalDisplayPalette(): TerminalDisplayPalette {
  const rootStyle = getComputedStyle(document.documentElement);
  const bodyStyle = getComputedStyle(document.body);
  const token = (name: string) => rootStyle.getPropertyValue(name).trim();
  return {
    background:
      token("--surface") ||
      bodyStyle.backgroundColor ||
      FALLBACK_TERMINAL_THEME.background,
    foreground:
      token("--text") || bodyStyle.color || FALLBACK_TERMINAL_THEME.foreground,
    cursor:
      token("--primary") || bodyStyle.color || FALLBACK_TERMINAL_THEME.cursor,
  };
}

export function observeTerminalDisplayPalette(
  listener: (palette: TerminalDisplayPalette) => void,
) {
  listener(readTerminalDisplayPalette());
  const observer = new MutationObserver(() => {
    listener(readTerminalDisplayPalette());
  });
  observer.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ["class", "data-color-scheme", "data-skin", "style"],
  });
  return { dispose: () => observer.disconnect() };
}

function readTerminalTheme(): ITheme {
  const rootStyle = getComputedStyle(document.documentElement);
  const token = (name: string) => rootStyle.getPropertyValue(name).trim();
  return {
    ...readTerminalDisplayPalette(),
    selectionBackground: token("--primary") || "#1677ff",
    selectionForeground: token("--on-primary") || "#ffffff",
    selectionInactiveBackground: token("--primary") || "#1677ff",
  };
}

class XtermTerminalSurface implements TerminalSurface {
  private readonly terminal: Terminal;
  private readonly element = document.createElement("div");
  private readonly fitAddon = new FitAddon();
  private readonly diagnosticKey: string;
  private opened = false;
  private disposed = false;
  private inputEnabled = false;
  private onInput?: (data: string) => void;
  private selectionListener?: (hasSelection: boolean) => void;
  private lockedGeometry = false;
  private outerScrollInitialized = false;
  private pendingUnlockedBottomOffset: number | null = null;
  private viewportRestoreFrame: number | null = null;
  private interactionCleanup?: () => void;
  private readonly disposables: Array<{ dispose(): void }> = [];

  constructor(options: TerminalSurfaceOptions) {
    this.diagnosticKey = options.diagnosticKey ?? "terminal-surface";
    this.element.className = "rua-terminal-xterm-host";
    this.terminal = new Terminal({
      allowProposedApi: true,
      cols: options.cols,
      cursorBlink: true,
      convertEol: true,
      fontFamily: TERMINAL_FONT_FAMILY,
      fontSize: 13,
      rescaleOverlappingGlyphs: true,
      rows: options.rows,
      screenReaderMode: false,
      scrollOnUserInput: true,
      scrollback: options.scrollbackLines,
      smoothScrollDuration:
        navigator.maxTouchPoints > 0 ||
        window.matchMedia?.("(prefers-reduced-motion: reduce)").matches
          ? 0
          : TERMINAL_SMOOTH_SCROLL_DURATION_MS,
      theme: readTerminalTheme(),
    });
    installTerminalPageNavigation(this.terminal, (pageCount) =>
      this.scrollPages(pageCount),
    );
    this.terminal.loadAddon(this.fitAddon);
    this.terminal.loadAddon(new SerializeAddon());
    this.terminal.loadAddon(new Unicode11Addon());
    this.terminal.unicode.activeVersion = "11";
    this.disposables.push(
      observeTerminalDisplayPalette(() => {
        this.terminal.options.theme = readTerminalTheme();
      }),
      installXtermCompatibility(this.terminal),
      this.terminal.onData((data) => {
        if (this.inputEnabled) this.onInput?.(data);
      }),
      this.terminal.onBinary((data) => {
        if (this.inputEnabled) this.onInput?.(data);
      }),
      this.terminal.onSelectionChange(
        () => this.selectionListener?.(this.terminal.hasSelection()),
      ),
    );
  }

  async mount(host: HTMLElement) {
    this.ensureActive();
    this.element.style.visibility = "hidden";
    this.element.style.position = "absolute";
    this.element.style.inset = "0";
    host.appendChild(this.element);
    if (!this.opened) {
      this.terminal.open(this.element);
      this.opened = true;
      this.installInteraction();
      try {
        const webgl = new WebglAddon();
        webgl.onContextLoss(() => {
          webgl.dispose();
          recordTerminalDiagnostic({
            event: "renderer_change",
            session_key: this.diagnosticKey,
            renderer: "dom",
            recovery_reason: "webgl_context_loss",
          });
        });
        this.terminal.loadAddon(webgl);
        recordTerminalDiagnostic({
          event: "renderer_change",
          session_key: this.diagnosticKey,
          renderer: "webgl",
        });
      } catch {
        recordTerminalDiagnostic({
          event: "renderer_change",
          session_key: this.diagnosticKey,
          renderer: "dom",
          recovery_reason: "webgl_unavailable",
        });
      }
    }
    this.terminal.refresh(0, this.terminal.rows - 1);
    await new Promise<void>((resolve) =>
      requestAnimationFrame(() => resolve()),
    );
    this.syncLockedViewport();
  }

  reveal() {
    this.ensureActive();
    this.element.style.removeProperty("visibility");
    this.element.style.removeProperty("position");
    this.element.style.removeProperty("inset");
    this.syncLockedViewport();
  }

  unmount(host: HTMLElement) {
    if (this.element.parentElement === host) this.element.remove();
  }

  refresh() {
    this.ensureActive();
    if (!this.opened || !this.element.isConnected) return;
    this.terminal.refresh(0, this.terminal.rows - 1);
    requestAnimationFrame(() => this.syncLockedViewport());
  }

  write(data: string | Uint8Array) {
    this.ensureActive();
    return new Promise<void>((resolve) => this.terminal.write(data, resolve));
  }

  resize(cols: number, rows: number) {
    this.ensureActive();
    if (this.terminal.cols === cols && this.terminal.rows === rows) return;
    this.terminal.resize(cols, rows);
    requestAnimationFrame(() => this.syncLockedViewport());
  }

  fit() {
    this.ensureActive();
    if (!this.opened || !this.element.isConnected) return null;
    this.fitAddon.fit();
    if (this.pendingUnlockedBottomOffset !== null) {
      const bottomOffset = this.pendingUnlockedBottomOffset;
      this.pendingUnlockedBottomOffset = null;
      this.scheduleViewportRestore(bottomOffset);
    }
    return { cols: this.terminal.cols, rows: this.terminal.rows };
  }

  setLockedGeometry(locked: boolean) {
    if (this.lockedGeometry !== locked) {
      if (this.lockedGeometry) {
        const buffer = this.terminal.buffer.active;
        const screen = this.element.querySelector<HTMLElement>(".xterm-screen");
        const screenHeight = screen?.getBoundingClientRect().height ?? 0;
        const cellHeight =
          screenHeight > 0 && this.terminal.rows > 0
            ? screenHeight / this.terminal.rows
            : 1;
        const maxScrollTop = Math.max(
          0,
          this.element.scrollHeight - this.element.clientHeight,
        );
        const outerBottomRows = Math.round(
          (maxScrollTop - this.element.scrollTop) / cellHeight,
        );
        this.pendingUnlockedBottomOffset =
          Math.max(0, buffer.baseY - buffer.viewportY) + outerBottomRows;
      } else {
        this.pendingUnlockedBottomOffset = null;
        this.cancelViewportRestore();
      }
      this.outerScrollInitialized = false;
    }
    this.lockedGeometry = locked;
    this.element.classList.toggle(
      "rua-terminal-xterm-host--locked-geometry",
      locked,
    );
    requestAnimationFrame(() => this.syncLockedViewport());
  }

  setScreenReaderMode(enabled: boolean) {
    this.terminal.options.screenReaderMode = enabled;
  }

  setSelectionListener(listener?: (hasSelection: boolean) => void) {
    this.selectionListener = listener;
    listener?.(this.terminal.hasSelection());
  }

  getSelection() {
    return this.terminal.getSelection();
  }

  setInputHandler(enabled: boolean, handler?: (data: string) => void) {
    this.inputEnabled = enabled;
    this.onInput = handler;
  }

  getViewportMetrics() {
    const buffer = this.terminal.buffer.active;
    return {
      baseY: buffer.baseY,
      cols: this.terminal.cols,
      rows: this.terminal.rows,
      viewportY: buffer.viewportY,
    };
  }

  scrollToTop() {
    this.pendingUnlockedBottomOffset = null;
    this.cancelViewportRestore();
    this.terminal.scrollToTop();
    if (this.lockedGeometry) this.element.scrollTop = 0;
  }

  scrollPages(count: number) {
    this.pendingUnlockedBottomOffset = null;
    this.cancelViewportRestore();
    const buffer = this.terminal.buffer.active;
    const terminalCanScroll =
      buffer.type === "normal" &&
      (count < 0 ? buffer.viewportY > 0 : buffer.viewportY < buffer.baseY);
    if (!this.lockedGeometry || terminalCanScroll) {
      this.terminal.scrollPages(count);
      return;
    }
    this.element.scrollBy({
      top: count * this.element.clientHeight,
      behavior: "auto",
    });
  }

  scrollToBottom() {
    this.pendingUnlockedBottomOffset = null;
    this.cancelViewportRestore();
    this.terminal.scrollToBottom();
    if (this.lockedGeometry) {
      this.element.scrollTop =
        this.element.scrollHeight - this.element.clientHeight;
    }
  }

  focus() {
    this.terminal.focus();
  }

  dispose() {
    if (this.disposed) return;
    this.disposed = true;
    this.cancelViewportRestore();
    this.interactionCleanup?.();
    for (const disposable of this.disposables) disposable.dispose();
    this.element.remove();
    this.terminal.dispose();
    recordTerminalDiagnostic({
      event: "runtime_dispose",
      session_key: this.diagnosticKey,
      lifecycle: "disposed",
    });
  }

  private installInteraction() {
    if (this.interactionCleanup) return;
    const textarea = this.terminal.textarea;
    let selectionTimer: number | null = null;
    let selectionAnchor: { column: number; row: number } | undefined;
    let selectionArmed = false;
    let inputFocusedAtTouchStart = false;
    let inertiaFrame: number | null = null;
    let inertiaLineRemainder = 0;
    let gesture:
      | {
          mode: "pending" | "vertical" | "horizontal";
          verticalOwner?: "inner" | "outer";
          startX: number;
          startY: number;
          lastY: number;
          lastTimestamp: number;
          startScrollLeft: number;
          lineRemainder: number;
          velocityY: number;
        }
      | undefined;
    const ownerWindow = this.element.ownerDocument.defaultView ?? window;
    const cancelInertia = () => {
      if (inertiaFrame === null) return;
      ownerWindow.cancelAnimationFrame(inertiaFrame);
      inertiaFrame = null;
      inertiaLineRemainder = 0;
    };
    const terminalCellHeight = () => {
      const screen = this.element.querySelector<HTMLElement>(".xterm-screen");
      const height = screen?.getBoundingClientRect().height ?? 0;
      return height > 0 && this.terminal.rows > 0
        ? height / this.terminal.rows
        : 1;
    };
    const startInertia = (initialVelocity: number) => {
      if (
        Math.abs(initialVelocity) < TOUCH_INERTIA_MIN_VELOCITY_PX_PER_MS ||
        ownerWindow.matchMedia?.("(prefers-reduced-motion: reduce)").matches
      ) {
        return;
      }
      cancelInertia();
      let velocity =
        Math.sign(initialVelocity) *
        Math.min(
          Math.abs(initialVelocity),
          TOUCH_INERTIA_MAX_VELOCITY_PX_PER_MS,
        );
      let lastTimestamp = ownerWindow.performance.now();
      const tick = (timestamp: number) => {
        const elapsed = Math.min(32, Math.max(1, timestamp - lastTimestamp));
        lastTimestamp = timestamp;
        inertiaLineRemainder += (velocity * elapsed) / terminalCellHeight();
        const lines = Math.trunc(inertiaLineRemainder);
        if (lines !== 0) {
          inertiaLineRemainder -= lines;
          const before = this.terminal.buffer.active.viewportY;
          this.terminal.scrollLines(lines);
          if (this.terminal.buffer.active.viewportY === before) {
            cancelInertia();
            return;
          }
        }
        const nextSpeed = Math.max(
          0,
          Math.abs(velocity) -
            TOUCH_INERTIA_FRICTION_PX_PER_MS_SQUARED * elapsed,
        );
        if (nextSpeed < TOUCH_INERTIA_MIN_VELOCITY_PX_PER_MS) {
          inertiaFrame = null;
          inertiaLineRemainder = 0;
          return;
        }
        velocity = Math.sign(velocity) * nextSpeed;
        inertiaFrame = ownerWindow.requestAnimationFrame(tick);
      };
      inertiaFrame = ownerWindow.requestAnimationFrame(tick);
    };

    const syncComposition = () => {
      syncCompositionPosition(this.terminal, this.element, textarea);
    };
    const handleCompositionStart = () => {
      syncComposition();
      window.setTimeout(syncComposition, 0);
    };
    const handleCompositionUpdate = () => {
      syncComposition();
      window.setTimeout(syncComposition, 0);
    };
    const handleCompositionEnd = () => {
      this.element
        .querySelector<HTMLElement>(".composition-view")
        ?.style.removeProperty("max-width");
    };
    textarea?.addEventListener("compositionstart", handleCompositionStart);
    textarea?.addEventListener("compositionupdate", handleCompositionUpdate);
    textarea?.addEventListener("compositionend", handleCompositionEnd);

    const clearSelectionTimer = () => {
      if (selectionTimer === null) return;
      window.clearTimeout(selectionTimer);
      selectionTimer = null;
    };
    const terminalCellAt = (touch: Touch) => {
      const screen = this.element.querySelector<HTMLElement>(".xterm-screen");
      const bounds = screen?.getBoundingClientRect();
      if (!bounds || bounds.width <= 0 || bounds.height <= 0) return null;
      return {
        column: Math.max(
          0,
          Math.min(
            this.terminal.cols - 1,
            Math.floor(
              ((touch.clientX - bounds.left) / bounds.width) *
                this.terminal.cols,
            ),
          ),
        ),
        row:
          this.terminal.buffer.active.viewportY +
          Math.max(
            0,
            Math.min(
              this.terminal.rows - 1,
              Math.floor(
                ((touch.clientY - bounds.top) / bounds.height) *
                  this.terminal.rows,
              ),
            ),
          ),
      };
    };
    const handleTouchStart = (event: TouchEvent) => {
      this.terminal.options.smoothScrollDuration = 0;
      if (!this.lockedGeometry) {
        this.pendingUnlockedBottomOffset = null;
        this.cancelViewportRestore();
      }
      cancelInertia();
      clearSelectionTimer();
      selectionArmed = false;
      selectionAnchor = undefined;
      inputFocusedAtTouchStart = document.activeElement === textarea;
      gesture = undefined;
      if (event.touches.length !== 1) return;
      const touch = event.touches[0];
      gesture = {
        mode: "pending",
        startX: touch.clientX,
        startY: touch.clientY,
        lastY: touch.clientY,
        lastTimestamp: event.timeStamp,
        startScrollLeft: this.element.scrollLeft,
        lineRemainder: 0,
        velocityY: 0,
      };
      const startCell = terminalCellAt(touch);
      selectionTimer = window.setTimeout(() => {
        if (!startCell) return;
        selectionArmed = true;
        selectionAnchor = startCell;
        gesture = undefined;
        this.terminal.select(startCell.column, startCell.row, 1);
        selectionTimer = null;
      }, TOUCH_SELECTION_HOLD_MS);
    };
    const handleTouchMove = (event: TouchEvent) => {
      if (selectionArmed && selectionAnchor && event.touches.length === 1) {
        const end = terminalCellAt(event.touches[0]);
        if (!end) return;
        const anchorOffset =
          selectionAnchor.row * this.terminal.cols + selectionAnchor.column;
        const endOffset = end.row * this.terminal.cols + end.column;
        const start = anchorOffset <= endOffset ? selectionAnchor : end;
        this.terminal.select(
          start.column,
          start.row,
          Math.abs(endOffset - anchorOffset) + 1,
        );
        if (event.cancelable) event.preventDefault();
        event.stopPropagation();
        return;
      }
      if (!gesture || event.touches.length !== 1) return;
      const touch = event.touches[0];
      const deltaX = gesture.startX - touch.clientX;
      const deltaY = gesture.startY - touch.clientY;
      if (
        Math.abs(deltaX) >= TOUCH_GESTURE_THRESHOLD_PX ||
        Math.abs(deltaY) >= TOUCH_GESTURE_THRESHOLD_PX
      ) {
        clearSelectionTimer();
      }
      if (gesture.mode === "pending") {
        if (
          Math.abs(deltaX) < TOUCH_GESTURE_THRESHOLD_PX &&
          Math.abs(deltaY) < TOUCH_GESTURE_THRESHOLD_PX
        ) {
          return;
        }
        if (
          this.lockedGeometry &&
          this.element.scrollWidth > this.element.clientWidth &&
          Math.abs(deltaX) > Math.abs(deltaY)
        ) {
          gesture.mode = "horizontal";
        } else if (Math.abs(deltaY) >= Math.abs(deltaX)) {
          gesture.mode = "vertical";
          const buffer = this.terminal.buffer.active;
          const direction = gesture.lastY - touch.clientY;
          const terminalCanScroll =
            buffer.type === "normal" &&
            (direction < 0
              ? buffer.viewportY > 0
              : buffer.viewportY < buffer.baseY);
          gesture.verticalOwner = terminalCanScroll ? "inner" : "outer";
        } else {
          return;
        }
      }
      if (gesture.mode === "horizontal") {
        this.element.scrollLeft = gesture.startScrollLeft + deltaX;
      } else if (gesture.verticalOwner === "inner") {
        const movement = gesture.lastY - touch.clientY;
        const elapsed = Math.max(1, event.timeStamp - gesture.lastTimestamp);
        const velocity = movement / elapsed;
        gesture.velocityY =
          gesture.velocityY === 0
            ? velocity
            : gesture.velocityY * 0.25 + velocity * 0.75;
        gesture.lastTimestamp = event.timeStamp;
        gesture.lineRemainder += movement / terminalCellHeight();
        gesture.lastY = touch.clientY;
        const lines = Math.trunc(gesture.lineRemainder);
        if (lines !== 0) {
          gesture.lineRemainder -= lines;
          this.terminal.scrollLines(lines);
        }
      } else {
        return;
      }
      if (event.cancelable) event.preventDefault();
      event.stopPropagation();
    };
    const handleTouchEnd = (event: TouchEvent) => {
      const inertiaVelocity =
        !selectionArmed &&
        gesture?.mode === "vertical" &&
        gesture.verticalOwner === "inner"
          ? gesture.velocityY
          : 0;
      if (selectionArmed) {
        if (event.cancelable) event.preventDefault();
        event.stopPropagation();
        if (!inputFocusedAtTouchStart) textarea?.blur();
      }
      resetTouch();
      startInertia(inertiaVelocity);
    };
    const resetTouch = () => {
      clearSelectionTimer();
      selectionArmed = false;
      selectionAnchor = undefined;
      inputFocusedAtTouchStart = false;
      gesture = undefined;
    };
    const handleWheel = (event: WheelEvent) => {
      this.terminal.options.smoothScrollDuration = ownerWindow.matchMedia?.(
        "(prefers-reduced-motion: reduce)",
      ).matches
        ? 0
        : TERMINAL_SMOOTH_SCROLL_DURATION_MS;
      if (!this.lockedGeometry) return;
      if (Math.abs(event.deltaX) > Math.abs(event.deltaY)) {
        event.stopPropagation();
        return;
      }
      const buffer = this.terminal.buffer.active;
      const terminalCanScroll =
        buffer.type === "normal" &&
        (event.deltaY < 0
          ? buffer.viewportY > 0
          : buffer.viewportY < buffer.baseY);
      if (!terminalCanScroll) {
        event.stopPropagation();
        return;
      }
      const screen = this.element.querySelector<HTMLElement>(".xterm-screen");
      const height = screen?.getBoundingClientRect().height ?? 0;
      const cellHeight =
        height > 0 && this.terminal.rows > 0 ? height / this.terminal.rows : 1;
      const lines =
        Math.trunc(event.deltaY / cellHeight) || Math.sign(event.deltaY);
      this.terminal.scrollLines(lines);
      if (event.cancelable) event.preventDefault();
      event.stopPropagation();
    };
    this.element.addEventListener("touchstart", handleTouchStart, {
      capture: true,
      passive: true,
    });
    this.element.addEventListener("touchmove", handleTouchMove, {
      capture: true,
      passive: false,
    });
    this.element.addEventListener("touchend", handleTouchEnd, {
      capture: true,
      passive: false,
    });
    const handleTouchCancel = () => {
      cancelInertia();
      resetTouch();
    };
    this.element.addEventListener("touchcancel", handleTouchCancel, {
      capture: true,
      passive: true,
    });
    this.element.addEventListener("wheel", handleWheel, {
      capture: true,
      passive: false,
    });

    this.interactionCleanup = () => {
      cancelInertia();
      clearSelectionTimer();
      textarea?.removeEventListener("compositionstart", handleCompositionStart);
      textarea?.removeEventListener(
        "compositionupdate",
        handleCompositionUpdate,
      );
      textarea?.removeEventListener("compositionend", handleCompositionEnd);
      this.element.removeEventListener("touchstart", handleTouchStart, true);
      this.element.removeEventListener("touchmove", handleTouchMove, true);
      this.element.removeEventListener("touchend", handleTouchEnd, true);
      this.element.removeEventListener("touchcancel", handleTouchCancel, true);
      this.element.removeEventListener("wheel", handleWheel, true);
    };
  }

  private syncLockedViewport() {
    const xterm = this.element.querySelector<HTMLElement>(".xterm");
    if (!xterm) return;
    xterm.style.removeProperty("transform");
    if (!this.lockedGeometry) {
      this.outerScrollInitialized = false;
      return;
    }
    const maxScrollTop = Math.max(
      0,
      this.element.scrollHeight - this.element.clientHeight,
    );
    if (!this.outerScrollInitialized) {
      const buffer = this.terminal.buffer.active;
      const screen = this.element.querySelector<HTMLElement>(".xterm-screen");
      const screenHeight = screen?.getBoundingClientRect().height ?? 0;
      const cellHeight =
        screenHeight > 0 && this.terminal.rows > 0
          ? screenHeight / this.terminal.rows
          : 1;
      const cursorScrollTop = Math.max(
        0,
        (buffer.cursorY + 1) * cellHeight - this.element.clientHeight,
      );
      this.element.scrollTop =
        buffer.viewportY === buffer.baseY
          ? Math.min(cursorScrollTop, maxScrollTop)
          : maxScrollTop;
      this.outerScrollInitialized = true;
    } else {
      this.element.scrollTop = Math.min(this.element.scrollTop, maxScrollTop);
    }
  }

  private cancelViewportRestore() {
    if (this.viewportRestoreFrame === null) return;
    const ownerWindow = this.element.ownerDocument.defaultView ?? window;
    ownerWindow.cancelAnimationFrame(this.viewportRestoreFrame);
    this.viewportRestoreFrame = null;
  }

  private scheduleViewportRestore(bottomOffset: number) {
    this.cancelViewportRestore();
    const ownerWindow = this.element.ownerDocument.defaultView ?? window;
    let attempts = 0;
    let stableFrames = 0;
    const restore = () => {
      if (this.disposed || this.lockedGeometry) {
        this.viewportRestoreFrame = null;
        return;
      }
      attempts += 1;
      const buffer = this.terminal.buffer.active;
      const currentBottomOffset = Math.max(0, buffer.baseY - buffer.viewportY);
      if (currentBottomOffset === bottomOffset) {
        stableFrames += 1;
      } else {
        stableFrames = 0;
        this.terminal.scrollLines(currentBottomOffset - bottomOffset);
      }
      this.element.scrollTop = 0;
      if (stableFrames >= 2 || attempts >= 8) {
        this.viewportRestoreFrame = null;
        return;
      }
      this.viewportRestoreFrame = ownerWindow.requestAnimationFrame(restore);
    };
    this.viewportRestoreFrame = ownerWindow.requestAnimationFrame(restore);
  }

  private ensureActive() {
    if (this.disposed) throw new Error("terminal surface is disposed");
  }
}

export class XtermTerminalSurfaceFactory implements TerminalSurfaceFactory {
  create(options: TerminalSurfaceOptions) {
    return new XtermTerminalSurface(options);
  }
}
