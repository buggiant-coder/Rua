import { FitAddon } from "@xterm/addon-fit";
import { SerializeAddon } from "@xterm/addon-serialize";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal } from "@xterm/xterm";
import type { TerminalAttachResponse } from "../../src/api/modules/terminal";
import type {
  TerminalActivityConnection,
  TerminalActivityTransport,
} from "../../src/terminal/runtime/viewerRuntime";
import { TerminalViewerRuntime } from "../../src/terminal/runtime/viewerRuntime";
import { XtermTerminalSurfaceFactory } from "../../src/terminal/runtime/xtermSurface";
import { installXtermCompatibility } from "../../src/terminal/runtime/xtermCompatibility";
import type { TerminalStreamFrame } from "../../src/terminal/stream";
import pageStyles from "../../src/components/Page/index.module.less";
import pageHeaderStyles from "../../src/components/PageHeader/index.module.less";
import "../../src/pages/Terminal/terminal.css";
import "@xterm/xterm/css/xterm.css";

interface CreateTerminalOptions {
  id: string;
  cols: number;
  rows: number;
  scrollback: number;
  snapshot?: string;
  tail?: string[];
}

interface CanaryEntry {
  terminal: Terminal;
  serializeAddon: SerializeAddon;
  host: HTMLElement;
  input: string[];
  compatibility: { dispose(): void };
  webgl: "loaded" | "fallback";
}

interface CanaryState {
  allLines: string[];
  backgroundRgb: number[];
  baseY: number;
  bufferType: "normal" | "alternate";
  cols: number;
  cursorX: number;
  cursorY: number;
  foregroundPaletteIndexes: number[];
  input: string[];
  modes: Record<string, unknown>;
  rows: number;
  selection: string;
  serialized: string;
  viewportY: number;
  visibleLines: string[];
  webgl: "loaded" | "fallback";
}

const entries = new Map<string, CanaryEntry>();
const pendingReplacements = new Map<string, CanaryEntry>();
const runtimeEntries = new Map<
  string,
  {
    host: HTMLElement;
    input: string[];
    replacement: string;
    runtime: TerminalViewerRuntime;
    transport: CanaryRuntimeTransport;
  }
>();
const root = document.querySelector<HTMLElement>("#terminals");
if (!root) {
  throw new Error("terminal canary root is missing");
}

function entry(id: string): CanaryEntry {
  const value = entries.get(id);
  if (!value) {
    throw new Error(`terminal canary not found: ${id}`);
  }
  return value;
}

function write(terminal: Terminal, data: string | Uint8Array): Promise<void> {
  return new Promise((resolve) => terminal.write(data, resolve));
}

class CanaryRuntimeTransport implements TerminalActivityTransport {
  handlers:
    | {
        onFrame: (frame: TerminalStreamFrame) => void;
        onError: (error: unknown) => void;
      }
    | undefined;
  openCount = 0;

  open(
    _cursor: { next_activity_sequence: number; next_output_offset: number },
    handlers: {
      onFrame: (frame: TerminalStreamFrame) => void;
      onError: (error: unknown) => void;
    },
  ): TerminalActivityConnection {
    this.handlers = handlers;
    this.openCount += 1;
    return { close() {} };
  }

  emitDelayed(frame: TerminalStreamFrame) {
    this.handlers?.onFrame(frame);
  }

  fail() {
    this.handlers?.onError(new Error("canary transport failure"));
  }
}

function runtimeAttachment(
  id: string,
  snapshot: string,
): TerminalAttachResponse {
  return {
    session: {
      session_id: id,
      cwd: "/tmp",
      profile_id: "canary",
      profile_label: "Canary",
      viewer_count: 1,
      pty_geometry: { cols: 30, rows: 6 },
    },
    snapshot: {
      data: snapshot,
      cols: 30,
      rows: 6,
      cursor: {
        next_activity_sequence: 0,
        next_output_offset: 0,
      },
    },
    tail: [],
    live_cursor: {
      next_activity_sequence: 0,
      next_output_offset: 0,
    },
  };
}

function lineText(terminal: Terminal, line: number): string {
  return terminal.buffer.active.getLine(line)?.translateToString(true) ?? "";
}

async function createTerminal(options: CreateTerminalOptions): Promise<void> {
  const existing = entries.get(options.id);
  existing?.compatibility.dispose();
  existing?.terminal.dispose();
  root
    .querySelector<HTMLElement>(`[data-terminal-id="${options.id}"]`)
    ?.remove();

  const host = document.createElement("div");
  host.className = "terminal-host";
  host.dataset.terminalId = options.id;
  root.appendChild(host);

  const terminal = new Terminal({
    allowProposedApi: true,
    cols: options.cols,
    cursorBlink: false,
    rows: options.rows,
    scrollback: options.scrollback,
  });
  const fitAddon = new FitAddon();
  const serializeAddon = new SerializeAddon();
  const unicode11Addon = new Unicode11Addon();
  terminal.loadAddon(fitAddon);
  terminal.loadAddon(serializeAddon);
  terminal.loadAddon(unicode11Addon);
  terminal.unicode.activeVersion = "11";
  const compatibility = installXtermCompatibility(terminal);

  if (options.snapshot) {
    await write(terminal, options.snapshot);
  }
  for (const frame of options.tail ?? []) {
    await write(terminal, frame);
  }
  terminal.open(host);

  let webgl: CanaryEntry["webgl"] = "fallback";
  try {
    const webglAddon = new WebglAddon();
    terminal.loadAddon(webglAddon);
    webgl = "loaded";
  } catch {}

  const input: string[] = [];
  terminal.onData((data) => input.push(data));
  entries.set(options.id, {
    terminal,
    serializeAddon,
    host,
    input,
    compatibility,
    webgl,
  });
}

function state(id: string): CanaryState {
  const value = entry(id);
  const { terminal } = value;
  const active = terminal.buffer.active;
  const foregroundPaletteIndexes = new Set<number>();
  const backgroundRgb = new Set<number>();
  for (let row = 0; row < active.length; row += 1) {
    const line = active.getLine(row);
    for (let column = 0; column < terminal.cols; column += 1) {
      const cell = line?.getCell(column);
      if (cell?.isFgPalette()) {
        foregroundPaletteIndexes.add(cell.getFgColor());
      }
      if (cell?.isBgRGB()) {
        backgroundRgb.add(cell.getBgColor());
      }
    }
  }
  return {
    allLines: Array.from({ length: active.length }, (_, index) =>
      lineText(terminal, index),
    ),
    backgroundRgb: [...backgroundRgb].sort((left, right) => left - right),
    baseY: active.baseY,
    bufferType: active.type,
    cols: terminal.cols,
    cursorX: active.cursorX,
    cursorY: active.cursorY,
    foregroundPaletteIndexes: [...foregroundPaletteIndexes].sort(
      (left, right) => left - right,
    ),
    input: [...value.input],
    modes: { ...terminal.modes },
    rows: terminal.rows,
    selection: terminal.getSelection(),
    serialized: value.serializeAddon.serialize(),
    viewportY: active.viewportY,
    visibleLines: Array.from({ length: terminal.rows }, (_, index) =>
      lineText(terminal, active.baseY + index),
    ),
    webgl: value.webgl,
  };
}

const canary = {
  clearInput(id: string) {
    entry(id).input.length = 0;
  },
  create: createTerminal,
  focus(id: string) {
    entry(id).terminal.focus();
  },
  async createRuntime(
    id: string,
    snapshot: string,
    highWatermarkBytes = 1024 * 1024,
    scrollbackLines = 20,
  ) {
    runtimeEntries.get(id)?.runtime.dispose();
    runtimeEntries.get(id)?.host.remove();
    const host = document.createElement("div");
    host.className = "terminal-host";
    host.dataset.terminalRuntimeId = id;
    root.appendChild(host);
    const transport = new CanaryRuntimeTransport();
    const input: string[] = [];
    const runtime = new TerminalViewerRuntime({
      key: `canary:${id}`,
      scrollbackLines,
      surfaceFactory: new XtermTerminalSurfaceFactory(),
      onInput: (data) => input.push(data),
      writerBatchDelayMs: 0,
      writerHighWatermarkBytes: highWatermarkBytes,
      writerLowWatermarkBytes: Math.max(1, Math.floor(highWatermarkBytes / 4)),
    });
    const runtimeEntry = {
      host,
      input,
      replacement: snapshot,
      runtime,
      transport,
    };
    runtimeEntries.set(id, runtimeEntry);
    runtime.setRecoveryProvider(async () =>
      runtimeAttachment(id, runtimeEntry.replacement),
    );
    await runtime.bind(host);
    await runtime.hydrate(runtimeAttachment(id, snapshot));
    runtime.setInputEnabled(true);
    runtime.startActivity(transport);
  },
  runtimeCounters(id: string) {
    const value = runtimeEntries.get(id);
    if (!value) throw new Error(`terminal runtime canary not found: ${id}`);
    return {
      ...value.runtime.lifecycleCounters,
      openCount: value.transport.openCount,
    };
  },
  runtimeEmitDelayed(
    id: string,
    data: string,
    activitySequence = 0,
    startOffset = 0,
  ) {
    const value = runtimeEntries.get(id);
    if (!value) throw new Error(`terminal runtime canary not found: ${id}`);
    const bytes = new TextEncoder().encode(data);
    let binary = "";
    for (const byte of bytes) binary += String.fromCharCode(byte);
    value.transport.emitDelayed({
      type: "output",
      activity_sequence: activitySequence,
      start_offset: startOffset,
      end_offset: startOffset + bytes.byteLength,
      data_base64: btoa(binary),
    });
  },
  runtimeFail(id: string) {
    const value = runtimeEntries.get(id);
    if (!value) throw new Error(`terminal runtime canary not found: ${id}`);
    value.transport.fail();
  },
  runtimeFocus(id: string) {
    const value = runtimeEntries.get(id);
    if (!value) throw new Error(`terminal runtime canary not found: ${id}`);
    value.runtime.focus();
  },
  runtimeInput(id: string) {
    const value = runtimeEntries.get(id);
    if (!value) throw new Error(`terminal runtime canary not found: ${id}`);
    return [...value.input];
  },
  runtimeSelection(id: string) {
    const value = runtimeEntries.get(id);
    if (!value) throw new Error(`terminal runtime canary not found: ${id}`);
    return value.runtime.getSelection();
  },
  runtimeTheme(id: string) {
    const value = runtimeEntries.get(id);
    if (!value) throw new Error(`terminal runtime canary not found: ${id}`);
    const surface = (
      value.runtime as unknown as {
        surface: {
          terminal: {
            options: {
              theme?: {
                background?: string;
                cursor?: string;
                foreground?: string;
                selectionBackground?: string;
              };
            };
          };
        } | null;
      }
    ).surface;
    if (!surface) throw new Error(`terminal runtime surface not found: ${id}`);
    return { ...surface.terminal.options.theme };
  },
  runtimeSetLockedGeometry(id: string, locked: boolean, cols = 30, rows = 6) {
    const value = runtimeEntries.get(id);
    if (!value) throw new Error(`terminal runtime canary not found: ${id}`);
    value.runtime.setLockedGeometry(locked);
    value.runtime.resize(cols, rows);
  },
  runtimeSetGeometryLocked(id: string, locked: boolean) {
    const value = runtimeEntries.get(id);
    if (!value) throw new Error(`terminal runtime canary not found: ${id}`);
    value.runtime.setLockedGeometry(locked);
  },
  runtimeFit(id: string) {
    const value = runtimeEntries.get(id);
    if (!value) throw new Error(`terminal runtime canary not found: ${id}`);
    return value.runtime.fit();
  },
  runtimeSetHostSize(id: string, width: number, height: number) {
    const value = runtimeEntries.get(id);
    if (!value) throw new Error(`terminal runtime canary not found: ${id}`);
    value.host.style.width = `${width}px`;
    value.host.style.height = `${height}px`;
  },
  runtimeScrollMetrics(id: string) {
    const value = runtimeEntries.get(id);
    if (!value) throw new Error(`terminal runtime canary not found: ${id}`);
    const host = value.host.querySelector<HTMLElement>(
      ".rua-terminal-xterm-host",
    );
    if (!host) throw new Error(`terminal runtime host not found: ${id}`);
    const styles = getComputedStyle(host);
    return {
      clientHeight: host.clientHeight,
      clientWidth: host.clientWidth,
      overflowX: styles.overflowX,
      overflowY: styles.overflowY,
      scrollHeight: host.scrollHeight,
      scrollLeft: host.scrollLeft,
      scrollTop: host.scrollTop,
      scrollWidth: host.scrollWidth,
    };
  },
  runtimeSetInnerScroll(id: string, position: "top" | "bottom") {
    const value = runtimeEntries.get(id);
    if (!value) throw new Error(`terminal runtime canary not found: ${id}`);
    const surface = (
      value.runtime as unknown as {
        surface: {
          terminal: { scrollToBottom(): void; scrollToTop(): void };
        } | null;
      }
    ).surface;
    if (!surface) throw new Error(`terminal runtime surface not found: ${id}`);
    if (position === "top") {
      surface.terminal.scrollToTop();
    } else {
      surface.terminal.scrollToBottom();
    }
  },
  runtimeScrollInnerLines(id: string, amount: number) {
    const value = runtimeEntries.get(id);
    if (!value) throw new Error(`terminal runtime canary not found: ${id}`);
    const surface = (
      value.runtime as unknown as {
        surface: {
          terminal: { scrollLines(amount: number): void };
        } | null;
      }
    ).surface;
    if (!surface) throw new Error(`terminal runtime surface not found: ${id}`);
    surface.terminal.scrollLines(amount);
  },
  runtimeSetOuterScroll(id: string, position: "top" | "bottom") {
    const value = runtimeEntries.get(id);
    if (!value) throw new Error(`terminal runtime canary not found: ${id}`);
    const host = value.host.querySelector<HTMLElement>(
      ".rua-terminal-xterm-host",
    );
    if (!host) throw new Error(`terminal runtime host not found: ${id}`);
    host.scrollTop =
      position === "top" ? 0 : host.scrollHeight - host.clientHeight;
  },
  runtimeWrapInTerminalPage(id: string, width: number, height: number) {
    const value = runtimeEntries.get(id);
    if (!value) throw new Error(`terminal runtime canary not found: ${id}`);
    const shell = document.createElement("div");
    shell.dataset.terminalPageShellId = id;
    shell.style.width = `${width}px`;
    shell.style.height = `${height}px`;
    shell.style.display = "flex";
    shell.style.flexDirection = "column";
    shell.style.overflow = "hidden";
    const page = document.createElement("div");
    page.className = `${pageStyles.page} rua-terminal-page`;
    page.dataset.terminalPageId = id;

    const pageHeader = document.createElement("div");
    pageHeader.className = `${pageHeaderStyles.pageHeader} rua-terminal-page-header`;
    const leading = document.createElement("div");
    leading.className = pageHeaderStyles.leading;
    leading.textContent = "Terminal";
    const extra = document.createElement("div");
    extra.className = pageHeaderStyles.extra;
    const actions = document.createElement("div");
    actions.className = "rua-terminal-header-actions";
    for (const width of [104, 220, 40]) {
      const action = document.createElement("div");
      action.style.width = `${width}px`;
      action.style.flex = "0 0 auto";
      actions.appendChild(action);
    }
    extra.appendChild(actions);
    pageHeader.append(leading, extra);
    const body = document.createElement("div");
    body.className = `${pageStyles.pageBody} rua-terminal-body`;
    const viewport = document.createElement("main");
    viewport.className = "rua-terminal-viewport";
    const sessionHeader = document.createElement("div");
    sessionHeader.className = "rua-terminal-session-header";
    sessionHeader.textContent = "Terminal session header";
    const stack = document.createElement("div");
    stack.className = "rua-terminal-viewport-stack";
    const slot = document.createElement("div");
    slot.className = "rua-terminal-viewport-slot is-active";

    value.host.classList.add("rua-terminal-xterm");
    value.host.style.width = "100%";
    value.host.style.height = "100%";
    value.host.replaceWith(shell);
    slot.appendChild(value.host);
    stack.appendChild(slot);
    viewport.append(sessionHeader, stack);
    body.appendChild(viewport);
    page.append(pageHeader, body);
    shell.appendChild(page);
  },
  runtimePageMetrics(id: string) {
    const shell = root.querySelector<HTMLElement>(
      `[data-terminal-page-shell-id="${id}"]`,
    );
    const page = root.querySelector<HTMLElement>(
      `[data-terminal-page-id="${id}"]`,
    );
    const pageHeader = page?.querySelector<HTMLElement>(
      ".rua-terminal-page-header",
    );
    const body = page?.querySelector<HTMLElement>(".rua-terminal-body");
    const viewport = page?.querySelector<HTMLElement>(
      ".rua-terminal-viewport",
    );
    const sessionHeader = page?.querySelector<HTMLElement>(
      ".rua-terminal-session-header",
    );
    const xterm = page?.querySelector<HTMLElement>(".rua-terminal-xterm");
    const host = page?.querySelector<HTMLElement>(".rua-terminal-xterm-host");
    if (
      !shell ||
      !page ||
      !pageHeader ||
      !body ||
      !viewport ||
      !sessionHeader ||
      !xterm ||
      !host
    ) {
      throw new Error(`terminal page fixture not found: ${id}`);
    }
    const containment = (element: HTMLElement) => {
      const styles = getComputedStyle(element);
      return {
        maxWidth: styles.maxWidth,
        minWidth: styles.minWidth,
        width: styles.width,
      };
    };
    return {
      bodyContainment: containment(body),
      hostClientWidth: host.clientWidth,
      hostContainment: containment(host),
      hostScrollWidth: host.scrollWidth,
      pageClientWidth: page.clientWidth,
      pageContainment: containment(page),
      pageHeaderContainment: containment(pageHeader),
      pageHeaderWidth: pageHeader.getBoundingClientRect().width,
      pageScrollWidth: page.scrollWidth,
      pageWidth: page.getBoundingClientRect().width,
      sessionHeaderContainment: containment(sessionHeader),
      sessionHeaderWidth: sessionHeader.getBoundingClientRect().width,
      shellClientWidth: shell.clientWidth,
      shellScrollWidth: shell.scrollWidth,
      viewportContainment: containment(viewport),
      xtermContainment: containment(xterm),
    };
  },
  runtimeViewport(id: string) {
    const value = runtimeEntries.get(id);
    if (!value) throw new Error(`terminal runtime canary not found: ${id}`);
    const surface = (
      value.runtime as unknown as {
        surface: {
          getViewportMetrics(): {
            baseY: number;
            cols: number;
            rows: number;
            viewportY: number;
          };
        } | null;
      }
    ).surface;
    if (!surface) throw new Error(`terminal runtime surface not found: ${id}`);
    return surface.getViewportMetrics();
  },
  runtimeSetReplacement(id: string, snapshot: string) {
    const value = runtimeEntries.get(id);
    if (!value) throw new Error(`terminal runtime canary not found: ${id}`);
    value.replacement = snapshot;
  },
  runtimeSuspend(id: string) {
    const value = runtimeEntries.get(id);
    if (!value) throw new Error(`terminal runtime canary not found: ${id}`);
    value.runtime.suspend();
  },
  async runtimeResume(id: string) {
    const value = runtimeEntries.get(id);
    if (!value) throw new Error(`terminal runtime canary not found: ${id}`);
    await value.runtime.resume();
  },
  commitReplacement(id: string) {
    const pending = pendingReplacements.get(id);
    if (!pending) {
      throw new Error(`terminal replacement not found: ${id}`);
    }
    const previous = entry(id);
    pending.host.style.removeProperty("position");
    pending.host.style.removeProperty("inset");
    pending.host.style.removeProperty("visibility");
    pending.host.dataset.terminalId = id;
    previous.host.replaceWith(pending.host);
    previous.compatibility.dispose();
    previous.terminal.dispose();
    entries.set(id, pending);
    pendingReplacements.delete(id);
  },
  async prepareReplacement(options: CreateTerminalOptions) {
    const existing = pendingReplacements.get(options.id);
    existing?.compatibility.dispose();
    existing?.terminal.dispose();
    existing?.host.remove();

    const host = document.createElement("div");
    host.className = "terminal-host";
    host.dataset.terminalId = `${options.id}-replacement`;
    host.style.position = "absolute";
    host.style.inset = "0";
    host.style.visibility = "hidden";
    root.appendChild(host);

    const terminal = new Terminal({
      allowProposedApi: true,
      cols: options.cols,
      cursorBlink: false,
      rows: options.rows,
      scrollback: options.scrollback,
    });
    const fitAddon = new FitAddon();
    const serializeAddon = new SerializeAddon();
    const unicode11Addon = new Unicode11Addon();
    terminal.loadAddon(fitAddon);
    terminal.loadAddon(serializeAddon);
    terminal.loadAddon(unicode11Addon);
    terminal.unicode.activeVersion = "11";
    const compatibility = installXtermCompatibility(terminal);
    if (options.snapshot) {
      await write(terminal, options.snapshot);
    }
    for (const frame of options.tail ?? []) {
      await write(terminal, frame);
    }
    terminal.open(host);

    let webgl: CanaryEntry["webgl"] = "fallback";
    try {
      terminal.loadAddon(new WebglAddon());
      webgl = "loaded";
    } catch {}
    const input: string[] = [];
    terminal.onData((data) => input.push(data));
    pendingReplacements.set(options.id, {
      terminal,
      serializeAddon,
      host,
      input,
      compatibility,
      webgl,
    });
  },
  select(id: string, column: number, row: number, length: number) {
    entry(id).terminal.select(column, row, length);
  },
  scrollLines(id: string, amount: number) {
    entry(id).terminal.scrollLines(amount);
  },
  viewportScrollTop(id: string) {
    const terminal = entry(id).terminal as unknown as {
      _core?: {
        _viewport?: {
          _scrollableElement?: {
            getScrollPosition(): { scrollTop: number };
          };
        };
      };
    };
    return (
      terminal._core?._viewport?._scrollableElement?.getScrollPosition()
        .scrollTop ?? 0
    );
  },
  state,
  async write(id: string, data: string) {
    await write(entry(id).terminal, data);
  },
  async writeBytes(id: string, data: number[]) {
    await write(entry(id).terminal, Uint8Array.from(data));
  },
  async writeChunks(id: string, chunks: string[]) {
    for (const chunk of chunks) {
      await write(entry(id).terminal, chunk);
    }
  },
  async writeBurst(id: string, chunks: string[]) {
    const callbacks: number[] = [];
    await Promise.all(
      chunks.map(
        (chunk, index) =>
          new Promise<void>((resolve) => {
            entry(id).terminal.write(chunk, () => {
              callbacks.push(index);
              resolve();
            });
          }),
      ),
    );
    return callbacks;
  },
};

declare global {
  interface Window {
    __terminalCanary: typeof canary;
  }
}

window.__terminalCanary = canary;
