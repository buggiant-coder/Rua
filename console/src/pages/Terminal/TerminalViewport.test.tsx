import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import TerminalViewport from "./TerminalViewport";

const { runtime, getOrCreate, transportInstances, resizeObservers } =
  vi.hoisted(() => {
    const runtime = {
      bind: vi.fn(async () => undefined),
      unbind: vi.fn(),
      ensureAttached: vi.fn(async () => undefined),
      setEventHandlers: vi.fn(),
      setRecoveryProvider: vi.fn(),
      setInputEnabled: vi.fn(),
      setScreenReaderMode: vi.fn(),
      setSelectionListener: vi.fn(),
      setLockedGeometry: vi.fn(),
      resize: vi.fn(),
      fit: vi.fn(() => ({ cols: 100, rows: 30 })),
      suspend: vi.fn(),
      resume: vi.fn(async () => undefined),
      focus: vi.fn(),
      refresh: vi.fn(),
      replaceActivityTransport: vi.fn(),
      getSelection: vi.fn(() => "selected terminal text"),
      scrollToTop: vi.fn(),
      scrollPages: vi.fn(),
      scrollToBottom: vi.fn(),
    };
    return {
      runtime,
      getOrCreate: vi.fn(() => runtime),
      transportInstances: [] as unknown[],
      resizeObservers: [] as Array<{
        callback: ResizeObserverCallback;
        observe: ReturnType<typeof vi.fn>;
        disconnect: ReturnType<typeof vi.fn>;
      }>,
    };
  });

vi.mock("../../terminal/runtime/registry", () => ({
  terminalRuntimeRegistry: { getOrCreate },
}));

vi.mock("../../terminal/runtime/eventSourceActivityTransport", () => ({
  EventSourceActivityTransport: vi.fn().mockImplementation(function (options) {
    Object.assign(this, options);
    transportInstances.push(this);
  }),
}));

vi.mock("../../terminal/runtime/xtermSurface", () => ({
  XtermTerminalSurfaceFactory: vi.fn(),
  observeTerminalDisplayPalette: vi.fn(
    (
      listener: (palette: {
        foreground: string;
        background: string;
        cursor: string;
      }) => void,
    ) => {
      listener({
        foreground: "#101418",
        background: "#f8f9fa",
        cursor: "#228be6",
      });
      return { dispose: vi.fn() };
    },
  ),
}));

vi.mock("@xterm/xterm/css/xterm.css", () => ({}));

vi.mock("../../utils/isMobile", () => ({
  isMobile: () => true,
}));

const activity = {
  browserViewerId: "browser-a",
  executionDeviceQueryValue: "local",
  placement: "local" as const,
  sessionId: "session-a",
  onFrame: vi.fn(),
  onError: vi.fn(),
  onRecoveryComplete: vi.fn(),
  onRecoveryError: vi.fn(),
  requestAttachment: vi.fn(async () => ({
    session: {
      session_id: "session-a",
      cwd: "/tmp",
      viewer_count: 1,
    },
    snapshot: {
      data: "",
      cols: 100,
      rows: 30,
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
  })),
};

function installVisualViewport(width = 412, height = 842) {
  let currentWidth = width;
  let currentHeight = height;
  const viewport = new EventTarget();
  Object.defineProperties(viewport, {
    width: {
      configurable: true,
      get: () => currentWidth,
    },
    height: {
      configurable: true,
      get: () => currentHeight,
    },
  });
  Object.defineProperty(window, "visualViewport", {
    configurable: true,
    value: viewport,
  });
  return {
    resize(nextWidth: number, nextHeight: number) {
      currentWidth = nextWidth;
      currentHeight = nextHeight;
      viewport.dispatchEvent(new Event("resize"));
    },
  };
}

describe("TerminalViewport runtime binding", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    transportInstances.length = 0;
    resizeObservers.length = 0;
    vi.stubGlobal(
      "ResizeObserver",
      vi.fn().mockImplementation(function (callback: ResizeObserverCallback) {
        const observer = {
          callback,
          observe: vi.fn(),
          disconnect: vi.fn(),
        };
        resizeObservers.push(observer);
        return observer;
      }),
    );
    Object.defineProperty(document, "hidden", {
      configurable: true,
      value: false,
    });
    Object.defineProperty(window, "visualViewport", {
      configurable: true,
      value: undefined,
    });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("binds one runtime and does not attach again for visibility changes", async () => {
    const view = render(
      <TerminalViewport
        active
        activity={activity}
        inputEnabled
        sessionKey="local:session-a:browser-a"
        scrollbackLines={5000}
      />,
    );

    await waitFor(() =>
      expect(runtime.ensureAttached).toHaveBeenCalledTimes(1),
    );
    expect(runtime.bind).toHaveBeenCalledTimes(1);
    expect(runtime.setInputEnabled).toHaveBeenCalledWith(true, undefined);

    view.rerender(
      <TerminalViewport
        active={false}
        activity={activity}
        inputEnabled={false}
        sessionKey="local:session-a:browser-a"
        scrollbackLines={5000}
      />,
    );
    view.rerender(
      <TerminalViewport
        active
        activity={activity}
        inputEnabled
        sessionKey="local:session-a:browser-a"
        scrollbackLines={5000}
      />,
    );

    expect(getOrCreate).toHaveBeenCalledTimes(1);
    expect(runtime.ensureAttached).toHaveBeenCalledTimes(1);
    expect(runtime.unbind).not.toHaveBeenCalled();
  });

  it("keeps passive authority geometry and reports local owner geometry", async () => {
    const onGeometryChange = vi.fn();
    const view = render(
      <TerminalViewport
        active
        activity={activity}
        inputEnabled={false}
        lockedGeometry={{ cols: 120, rows: 40 }}
        onGeometryChange={onGeometryChange}
        sessionKey="local:session-a:browser-a"
        scrollbackLines={5000}
      />,
    );

    expect(runtime.setLockedGeometry).toHaveBeenCalledWith(true);
    expect(runtime.resize).toHaveBeenCalledWith(120, 40);
    expect(onGeometryChange).not.toHaveBeenCalled();

    view.rerender(
      <TerminalViewport
        active
        activity={activity}
        inputEnabled
        lockedGeometry={null}
        onGeometryChange={onGeometryChange}
        sessionKey="local:session-a:browser-a"
        scrollbackLines={5000}
      />,
    );
    await waitFor(() =>
      expect(runtime.setLockedGeometry).toHaveBeenCalledWith(false),
    );
  });

  it("reports owner geometry again after retaking a previously held lease", async () => {
    const onGeometryChange = vi.fn();
    const view = render(
      <TerminalViewport
        active
        activity={activity}
        inputEnabled
        onGeometryChange={onGeometryChange}
        sessionKey="local:session-a:browser-a"
        scrollbackLines={5000}
      />,
    );
    await waitFor(() =>
      expect(onGeometryChange).toHaveBeenCalledWith("session-a", {
        cols: 100,
        rows: 30,
      }),
    );

    view.rerender(
      <TerminalViewport
        active
        activity={activity}
        inputEnabled={false}
        lockedGeometry={{ cols: 155, rows: 40 }}
        onGeometryChange={onGeometryChange}
        sessionKey="local:session-a:browser-a"
        scrollbackLines={5000}
      />,
    );
    await waitFor(() => expect(runtime.resize).toHaveBeenCalledWith(155, 40));

    view.rerender(
      <TerminalViewport
        active
        activity={activity}
        inputEnabled
        lockedGeometry={null}
        onGeometryChange={onGeometryChange}
        sessionKey="local:session-a:browser-a"
        scrollbackLines={5000}
      />,
    );

    await waitFor(() => expect(onGeometryChange).toHaveBeenCalledTimes(2));
    expect(onGeometryChange).toHaveBeenLastCalledWith("session-a", {
      cols: 100,
      rows: 30,
    });
  });

  it("reports stable owner geometry after the initial attachment", async () => {
    const onGeometryChange = vi.fn();
    render(
      <TerminalViewport
        active
        activity={activity}
        inputEnabled
        onGeometryChange={onGeometryChange}
        sessionKey="local:session-a:browser-a"
        scrollbackLines={5000}
      />,
    );

    await waitFor(() =>
      expect(onGeometryChange).toHaveBeenCalledWith("session-a", {
        cols: 100,
        rows: 30,
      }),
    );
  });

  it("keeps owner geometry fixed while a docked mobile keyboard is visible", async () => {
    const visualViewport = installVisualViewport();
    const onGeometryChange = vi.fn();
    const { container } = render(
      <TerminalViewport
        active
        activity={activity}
        inputEnabled
        onGeometryChange={onGeometryChange}
        sessionKey="local:session-a:browser-a"
        scrollbackLines={5000}
      />,
    );

    await waitFor(() =>
      expect(onGeometryChange).toHaveBeenCalledWith("session-a", {
        cols: 100,
        rows: 30,
      }),
    );
    runtime.fit.mockClear();
    runtime.resize.mockClear();
    runtime.scrollToBottom.mockClear();
    runtime.setLockedGeometry.mockClear();
    onGeometryChange.mockClear();

    const textarea = document.createElement("textarea");
    container.querySelector(".rua-terminal-xterm")?.appendChild(textarea);
    act(() => textarea.focus());
    act(() => {
      resizeObservers[0]?.callback(
        [],
        resizeObservers[0] as unknown as ResizeObserver,
      );
      visualViewport.resize(412, 530);
    });

    await waitFor(() =>
      expect(runtime.setLockedGeometry).toHaveBeenCalledWith(true),
    );
    expect(runtime.resize).toHaveBeenCalledWith(100, 30);
    await waitFor(() => expect(runtime.scrollToBottom).toHaveBeenCalled());
    await new Promise((resolve) => window.setTimeout(resolve, 250));
    expect(runtime.fit).not.toHaveBeenCalled();
    expect(onGeometryChange).not.toHaveBeenCalled();

    act(() => visualViewport.resize(412, 842));

    await waitFor(() =>
      expect(runtime.setLockedGeometry).toHaveBeenLastCalledWith(false),
    );
    await waitFor(() => expect(runtime.fit).toHaveBeenCalled());
  });

  it("freezes the authority geometry when the keyboard opens immediately after takeover", async () => {
    const visualViewport = installVisualViewport();
    const onGeometryChange = vi.fn();
    const view = render(
      <TerminalViewport
        active
        activity={activity}
        inputEnabled={false}
        lockedGeometry={{ cols: 120, rows: 40 }}
        onGeometryChange={onGeometryChange}
        sessionKey="local:session-a:browser-a"
        scrollbackLines={5000}
      />,
    );

    await waitFor(() => expect(runtime.resize).toHaveBeenCalledWith(120, 40));
    runtime.fit.mockClear();
    runtime.resize.mockClear();
    runtime.setLockedGeometry.mockClear();
    onGeometryChange.mockClear();

    view.rerender(
      <TerminalViewport
        active
        activity={activity}
        inputEnabled
        lockedGeometry={null}
        onGeometryChange={onGeometryChange}
        sessionKey="local:session-a:browser-a"
        scrollbackLines={5000}
      />,
    );

    const textarea = document.createElement("textarea");
    view.container
      .querySelector(".rua-terminal-xterm")
      ?.appendChild(textarea);
    act(() => textarea.focus());
    act(() => {
      resizeObservers[0]?.callback(
        [],
        resizeObservers[0] as unknown as ResizeObserver,
      );
      visualViewport.resize(412, 530);
    });

    await waitFor(() =>
      expect(runtime.setLockedGeometry).toHaveBeenCalledWith(true),
    );
    expect(runtime.resize).toHaveBeenCalledWith(120, 40);
    await new Promise((resolve) => window.setTimeout(resolve, 250));
    expect(runtime.fit).not.toHaveBeenCalled();
    expect(onGeometryChange).not.toHaveBeenCalled();
    expect(runtime.ensureAttached).toHaveBeenCalledTimes(1);
  });

  it("ignores a floating mobile keyboard that does not resize the viewport", async () => {
    const visualViewport = installVisualViewport();
    const { container } = render(
      <TerminalViewport
        active
        activity={activity}
        inputEnabled
        sessionKey="local:session-a:browser-a"
        scrollbackLines={5000}
      />,
    );

    await waitFor(() => expect(runtime.fit).toHaveBeenCalled());
    runtime.resize.mockClear();
    runtime.setLockedGeometry.mockClear();

    const textarea = document.createElement("textarea");
    container.querySelector(".rua-terminal-xterm")?.appendChild(textarea);
    act(() => textarea.focus());
    act(() => visualViewport.resize(412, 842));

    await new Promise((resolve) => window.setTimeout(resolve, 250));
    expect(runtime.setLockedGeometry).not.toHaveBeenCalledWith(true);
    expect(runtime.resize).not.toHaveBeenCalled();
  });

  it("reports the display palette only for the active input lease holder", async () => {
    const onPaletteChange = vi.fn();
    const view = render(
      <TerminalViewport
        active
        activity={activity}
        inputEnabled
        onPaletteChange={onPaletteChange}
        sessionKey="local:session-a:browser-a"
        scrollbackLines={5000}
      />,
    );

    await waitFor(() =>
      expect(onPaletteChange).toHaveBeenCalledWith({
        foreground: "#101418",
        background: "#f8f9fa",
        cursor: "#228be6",
      }),
    );
    onPaletteChange.mockClear();
    view.rerender(
      <TerminalViewport
        active={false}
        activity={activity}
        inputEnabled={false}
        onPaletteChange={onPaletteChange}
        sessionKey="local:session-a:browser-a"
        scrollbackLines={5000}
      />,
    );
    expect(onPaletteChange).not.toHaveBeenCalled();
  });

  it("distinguishes document suspension from route visibility", async () => {
    render(
      <TerminalViewport
        active
        activity={activity}
        sessionKey="local:session-a:browser-a"
        scrollbackLines={5000}
      />,
    );

    Object.defineProperty(document, "hidden", {
      configurable: true,
      value: true,
    });
    document.dispatchEvent(new Event("visibilitychange"));
    expect(runtime.suspend).toHaveBeenCalledTimes(1);

    Object.defineProperty(document, "hidden", {
      configurable: true,
      value: false,
    });
    document.dispatchEvent(new Event("visibilitychange"));
    await waitFor(() => expect(runtime.resume).toHaveBeenCalledTimes(1));
  });

  it("uses native Android lifecycle events when WebView visibility is stale", async () => {
    render(
      <TerminalViewport
        active
        activity={activity}
        sessionKey="local:session-a:browser-a"
        scrollbackLines={5000}
      />,
    );

    window.dispatchEvent(new Event("rua:background"));
    expect(runtime.suspend).toHaveBeenCalledTimes(1);

    window.dispatchEvent(new Event("rua:foreground"));
    await waitFor(() => expect(runtime.resume).toHaveBeenCalledTimes(1));
    expect(runtime.refresh).toHaveBeenCalledTimes(1);
  });

  it("keeps a retained terminal live while inactive and only releases authority", async () => {
    const remoteActivity = {
      ...activity,
      executionDeviceQueryValue: "device-b",
      placement: "remote" as const,
    };
    const view = render(
      <TerminalViewport
        active
        activity={remoteActivity}
        sessionKey="local:session-a:browser-a"
        scrollbackLines={5000}
      />,
    );

    view.rerender(
      <TerminalViewport
        active={false}
        activity={remoteActivity}
        sessionKey="local:session-a:browser-a"
        scrollbackLines={5000}
      />,
    );
    expect(runtime.suspend).not.toHaveBeenCalled();
    expect(runtime.replaceActivityTransport).toHaveBeenCalled();
    expect(transportInstances.at(-1)).toMatchObject({
      retainAuthority: false,
    });

    view.rerender(
      <TerminalViewport
        active
        activity={remoteActivity}
        sessionKey="local:session-a:browser-a"
        scrollbackLines={5000}
      />,
    );
    expect(runtime.resume).not.toHaveBeenCalled();
    expect(runtime.ensureAttached).toHaveBeenCalledTimes(1);
    expect(transportInstances.at(-1)).toMatchObject({
      retainAuthority: true,
    });
  });

  it("unbinds without disposing the persistent runtime", async () => {
    const view = render(
      <TerminalViewport
        active
        activity={activity}
        sessionKey="local:session-a:browser-a"
        scrollbackLines={5000}
      />,
    );
    await waitFor(() => expect(runtime.bind).toHaveBeenCalledTimes(1));

    view.unmount();

    expect(runtime.unbind).toHaveBeenCalledTimes(1);
    expect(runtime).not.toHaveProperty("dispose");
  });

  it("copies the runtime-owned selection", async () => {
    const writeText = vi.fn(async () => undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    runtime.setSelectionListener.mockImplementation((listener) => {
      listener?.(true);
    });
    render(
      <TerminalViewport
        active
        activity={activity}
        sessionKey="local:session-a:browser-a"
        scrollbackLines={5000}
      />,
    );

    fireEvent.click(
      await screen.findByRole("button", { name: "复制终端选区" }),
    );

    expect(writeText).toHaveBeenCalledWith("selected terminal text");
  });

  it("shows all four terminal navigation controls and routes their commands", async () => {
    const { container } = render(
      <TerminalViewport
        active
        activity={activity}
        sessionKey="local:session-a:browser-a"
        scrollbackLines={5000}
      />,
    );

    fireEvent.wheel(
      container.querySelector(".rua-terminal-xterm") as HTMLElement,
    );

    fireEvent.click(
      await screen.findByRole("button", { name: "滚动到最上方" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "往前滚一屏" }));
    fireEvent.click(screen.getByRole("button", { name: "往下滚一屏" }));
    fireEvent.click(screen.getByRole("button", { name: "滚动到最下方" }));

    expect(runtime.scrollToTop).toHaveBeenCalledTimes(1);
    expect(runtime.scrollPages).toHaveBeenNthCalledWith(1, -1);
    expect(runtime.scrollPages).toHaveBeenNthCalledWith(2, 1);
    expect(runtime.scrollToBottom).toHaveBeenCalledTimes(1);
  });

  it("delegates touch activation to the surface without focusing on long press clicks", () => {
    const { container } = render(
      <TerminalViewport
        active
        activity={activity}
        inputEnabled
        sessionKey="local:session-a:browser-a"
        scrollbackLines={5000}
      />,
    );
    const viewport = container.querySelector(
      ".rua-terminal-xterm",
    ) as HTMLElement;

    fireEvent.pointerDown(viewport, { pointerType: "touch" });
    fireEvent.click(viewport);
    expect(runtime.focus).not.toHaveBeenCalled();

    fireEvent.pointerDown(viewport, { pointerType: "mouse" });
    expect(runtime.focus).toHaveBeenCalledTimes(1);
  });
});
