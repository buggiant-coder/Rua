import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import TerminalPage from "./index";

const mockListDevices = vi.fn();
const mockGetSyncStatus = vi.fn();
const mockListProfiles = vi.fn();
const mockListSessions = vi.fn();
const mockOpenSession = vi.fn();
const mockAttachSession = vi.fn();
const mockAttachScrollback = vi.fn();
const mockUpdateScrollback = vi.fn();
const mockDetachSession = vi.fn();
const mockKillSession = vi.fn();
const mockTakeInputLease = vi.fn();
const mockAdaptPtyGeometry = vi.fn();
const mockUpdatePalette = vi.fn();
const mockRenameSession = vi.fn();
const mockSendInput = vi.fn();
const mockViewportAppend = vi.fn();
const mockViewportReset = vi.fn();
const mockViewportResize = vi.fn();
const mockViewportMount = vi.fn();
const mockViewportUnmount = vi.fn();
const mockViewportDispose = vi.fn();
const mockDisposeAllViewports = vi.fn();
let nextViewportAppendGate: Promise<void> | null = null;
let mockViewportGeometryHandler:
  | ((sessionId: string, geometry: { cols: number; rows: number }) => void)
  | undefined;
let mockViewportPaletteHandler:
  | ((palette: {
      foreground: string;
      background: string;
      cursor: string;
    }) => void)
  | undefined;

const eventSources: FakeEventSource[] = [];

function encodeBase64(data: string) {
  const bytes = new TextEncoder().encode(data);
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return { bytes, encoded: btoa(binary) };
}

function positionedOutput(
  data: string,
  activitySequence: number,
  startOffset: number,
) {
  const encoded = encodeBase64(data);
  return {
    type: "output" as const,
    activity_sequence: activitySequence,
    start_offset: startOffset,
    end_offset: startOffset + encoded.bytes.byteLength,
    data_base64: encoded.encoded,
  };
}

function normalizeAttachmentFixture(response: {
  session: unknown;
  cursor?: { next_activity_sequence: number; next_output_offset: number };
  live_cursor?: { next_activity_sequence: number; next_output_offset: number };
  snapshot?:
    | string
    | {
        data: string;
        cols: number;
        rows: number;
        output_offset: number;
      }
    | null;
  tail?: unknown[];
  replay?: Array<{ type: string; data?: string }>;
}) {
  if (
    response.live_cursor &&
    response.snapshot &&
    typeof response.snapshot !== "string"
  ) {
    return { ...response, tail: response.tail ?? [] };
  }
  if (response.cursor) {
    const snapshot =
      response.snapshot && typeof response.snapshot !== "string"
        ? {
            data: response.snapshot.data,
            cols: response.snapshot.cols,
            rows: response.snapshot.rows,
            cursor:
              "cursor" in response.snapshot
                ? response.snapshot.cursor
                : response.cursor,
          }
        : {
            data:
              typeof response.snapshot === "string" ? response.snapshot : "",
            cols: 80,
            rows: 24,
            cursor: response.cursor,
          };
    let liveCursor = { ...snapshot.cursor };
    for (const frame of response.tail ?? []) {
      const activity = frame as {
        type?: string;
        activity_sequence?: number;
        end_offset?: number;
      };
      if (activity.activity_sequence === liveCursor.next_activity_sequence) {
        liveCursor = {
          next_activity_sequence: liveCursor.next_activity_sequence + 1,
          next_output_offset:
            activity.type === "output" && activity.end_offset !== undefined
              ? activity.end_offset
              : liveCursor.next_output_offset,
        };
      }
    }
    return {
      session: response.session,
      snapshot,
      tail: response.tail ?? [],
      live_cursor: liveCursor,
    };
  }
  if (typeof response.snapshot === "string") {
    const outputOffset = new TextEncoder().encode(response.snapshot).byteLength;
    return {
      session: response.session,
      cursor: {
        next_activity_sequence: 0,
        next_output_offset: outputOffset,
      },
      snapshot: {
        data: response.snapshot,
        cols: 80,
        rows: 24,
        cursor: {
          next_activity_sequence: 0,
          next_output_offset: outputOffset,
        },
      },
      tail: [],
      live_cursor: {
        next_activity_sequence: 0,
        next_output_offset: outputOffset,
      },
    };
  }
  let sequence = 0;
  let offset = 0;
  const tail = (response.replay ?? []).flatMap((frame) => {
    if (frame.type !== "output" || frame.data === undefined) return [];
    const positioned = positionedOutput(frame.data, sequence, offset);
    sequence += 1;
    offset = positioned.end_offset;
    return [positioned];
  });
  return {
    session: response.session,
    snapshot: {
      data: "",
      cols: 80,
      rows: 24,
      cursor: {
        next_activity_sequence: 0,
        next_output_offset: 0,
      },
    },
    tail,
    live_cursor: {
      next_activity_sequence: sequence,
      next_output_offset: offset,
    },
  };
}

class FakeEventSource {
  url: string;
  onmessage: ((event: MessageEvent<string>) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  listeners = new Map<string, Array<(event: MessageEvent<string>) => void>>();
  closed = false;
  nextActivitySequence: number;
  nextOutputOffset: number;

  constructor(url: string) {
    this.url = url;
    const parsed = new URL(url);
    this.nextActivitySequence = Number(
      parsed.searchParams.get("next_activity_sequence") ?? 0,
    );
    this.nextOutputOffset = Number(
      parsed.searchParams.get("next_output_offset") ?? 0,
    );
    eventSources.push(this);
  }

  emit(data: unknown) {
    if (this.closed) return;
    const input = data as {
      type?: string;
      data?: string;
      activity_sequence?: number;
    };
    let frame: unknown = data;
    if (input.activity_sequence === undefined && input.type === "output") {
      frame = positionedOutput(
        input.data ?? "",
        this.nextActivitySequence,
        this.nextOutputOffset,
      );
      this.nextOutputOffset = (frame as { end_offset: number }).end_offset;
      this.nextActivitySequence += 1;
    } else if (input.activity_sequence === undefined && input.type) {
      frame = { ...input, activity_sequence: this.nextActivitySequence };
      this.nextActivitySequence += 1;
    }
    const event = { data: JSON.stringify(frame) } as MessageEvent<string>;
    this.onmessage?.(event);
    for (const listener of this.listeners.get("terminal_activity") ?? []) {
      listener(event);
    }
  }

  addEventListener(
    type: string,
    listener: (event: MessageEvent<string>) => void,
  ) {
    const listeners = this.listeners.get(type) ?? [];
    listeners.push(listener);
    this.listeners.set(type, listeners);
  }

  close() {
    this.closed = true;
  }

  fail() {
    this.onerror?.(new Event("error"));
  }
}

vi.mock("../../api/modules/sync", () => ({
  devicesApi: {
    listDevices: () => mockListDevices(),
  },
  syncApi: {
    getStatus: () => mockGetSyncStatus(),
  },
}));

vi.mock("../../api/modules/terminal", () => ({
  terminalApi: {
    listProfiles: (device: unknown) => mockListProfiles(device),
    listSessions: (device: unknown) => mockListSessions(device),
    openSession: (body: unknown) => mockOpenSession(body),
    attachSession: (
      sessionId: string,
      browserViewerId: string,
      scrollbackLines: number,
      executionDevice: unknown,
      cursor?: unknown,
    ) => {
      mockAttachScrollback(sessionId, scrollbackLines, executionDevice);
      const response = cursor
        ? mockAttachSession(sessionId, browserViewerId, executionDevice, cursor)
        : mockAttachSession(sessionId, browserViewerId, executionDevice);
      return Promise.resolve(response).then(normalizeAttachmentFixture);
    },
    updateScrollback: (
      sessionId: string,
      scrollbackLines: number,
      executionDevice: unknown,
    ) => mockUpdateScrollback(sessionId, scrollbackLines, executionDevice),
    detachSession: (
      sessionId: string,
      browserViewerId: string,
      executionDevice: unknown,
    ) => mockDetachSession(sessionId, browserViewerId, executionDevice),
    killSession: (
      sessionId: string,
      browserViewerId: string,
      executionDevice: unknown,
    ) => mockKillSession(sessionId, browserViewerId, executionDevice),
    takeInputLease: (
      sessionId: string,
      browserViewerId: string,
      executionDevice: unknown,
    ) => mockTakeInputLease(sessionId, browserViewerId, executionDevice),
    adaptPtyGeometry: (
      sessionId: string,
      browserViewerId: string,
      cols: number,
      rows: number,
      executionDevice: unknown,
    ) =>
      mockAdaptPtyGeometry(
        sessionId,
        browserViewerId,
        cols,
        rows,
        executionDevice,
      ),
    updatePalette: (
      sessionId: string,
      browserViewerId: string,
      palette: unknown,
      executionDevice: unknown,
    ) =>
      mockUpdatePalette(sessionId, browserViewerId, palette, executionDevice),
    renameSession: (
      sessionId: string,
      displayName: string | null,
      executionDevice: unknown,
    ) => mockRenameSession(sessionId, displayName, executionDevice),
    sendInput: (
      sessionId: string,
      browserViewerId: string,
      data: string,
      executionDevice: unknown,
    ) => mockSendInput(sessionId, browserViewerId, data, executionDevice),
  },
}));

vi.mock("../../terminal/runtime/registry", () => ({
  terminalRuntimeRegistry: {
    dispose: (key: string) => mockViewportDispose(key),
    disposeAll: () => mockDisposeAllViewports(),
  },
}));

vi.mock("../../api/config", () => ({
  getApiUrl: (path: string) => `http://test/api${path}`,
}));

vi.mock("./TerminalViewport", async () => {
  const React = await vi.importActual<typeof import("react")>("react");
  return {
    default: React.forwardRef<
      {
        append: (data: string | Uint8Array) => Promise<void>;
        hydrate: (snapshot: {
          data: string;
          cols: number;
          rows: number;
        }) => Promise<void>;
        resize: (geometry: { cols: number; rows: number }) => void;
        serialize: () => string;
        hydrateAttachment: (attachment: {
          snapshot: { data: string };
          tail: Array<{ type: string; data_base64?: string }>;
          live_cursor: {
            next_activity_sequence: number;
            next_output_offset: number;
          };
        }) => Promise<void>;
        applyLiveFrame: (frame: {
          type: string;
          activity_sequence: number;
          start_offset?: number;
          end_offset?: number;
          data_base64?: string;
          geometry?: { cols: number; rows: number };
        }) => Promise<"apply">;
        getCursor: () => {
          next_activity_sequence: number;
          next_output_offset: number;
        };
        disposeRuntime: () => void;
      },
      {
        active?: boolean;
        inputEnabled?: boolean;
        lockedGeometry?: { cols: number; rows: number } | null;
        sessionKey: string;
        scrollbackLines?: number;
        onInput?: (data: string) => void;
        onGeometryChange?: (
          sessionId: string,
          geometry: { cols: number; rows: number },
        ) => void;
        onPaletteChange?: (palette: {
          foreground: string;
          background: string;
          cursor: string;
        }) => void;
        activity?: {
          browserViewerId: string;
          executionDeviceQueryValue: string;
          sessionId: string;
          onFrame: (
            frame: {
              type: string;
              activity_sequence: number;
              start_offset?: number;
              end_offset?: number;
              data_base64?: string;
              geometry?: { cols: number; rows: number };
            },
            cursor: {
              next_activity_sequence: number;
              next_output_offset: number;
            },
          ) => void;
          onError: (error: unknown) => void;
          onRecoveryComplete: (reason: string) => void;
          onRecoveryError: (reason: string, error: unknown) => void;
          requestAttachment: (reason?: string) => Promise<{
            snapshot: { data: string };
            tail: Array<{ type: string; data_base64?: string }>;
            live_cursor: {
              next_activity_sequence: number;
              next_output_offset: number;
            };
          }>;
        };
      }
    >(function MockTerminalViewport(
      {
        active = true,
        inputEnabled,
        lockedGeometry,
        sessionKey,
        scrollbackLines,
        onInput,
        onGeometryChange,
        onPaletteChange,
        activity,
      },
      ref,
    ) {
      const [output, setOutput] = React.useState("");
      const cursorRef = React.useRef({
        next_activity_sequence: 0,
        next_output_offset: 0,
      });
      const activitySourceRef = React.useRef<FakeEventSource | null>(null);
      const activityApplyQueueRef = React.useRef<Promise<void>>(
        Promise.resolve(),
      );
      const initialAttachStartedRef = React.useRef(false);
      const recoveryRef = React.useRef<Promise<void> | null>(null);
      const applyFrame = async (frame: {
        type: string;
        activity_sequence: number;
        start_offset?: number;
        end_offset?: number;
        data_base64?: string;
        geometry?: { cols: number; rows: number };
      }) => {
        if (frame.type === "output" && frame.data_base64) {
          const binary = atob(frame.data_base64);
          const text = new TextDecoder().decode(
            Uint8Array.from(binary, (character) => character.charCodeAt(0)),
          );
          mockViewportAppend(text);
          const gate = nextViewportAppendGate ?? Promise.resolve();
          nextViewportAppendGate = null;
          await gate;
          setOutput((current) => current + text);
        } else if (frame.type === "pty_geometry_changed" && frame.geometry) {
          mockViewportResize(sessionKey, frame.geometry);
        }
        cursorRef.current = {
          next_activity_sequence: frame.activity_sequence + 1,
          next_output_offset:
            frame.type === "output" && frame.end_offset !== undefined
              ? frame.end_offset
              : cursorRef.current.next_output_offset,
        };
      };
      const hydrateRuntimeAttachment = async (attachment: {
        snapshot: { data: string };
        tail: Array<{ type: string; data_base64?: string }>;
        live_cursor: {
          next_activity_sequence: number;
          next_output_offset: number;
        };
      }): Promise<void> => {
        const tailOutput = attachment.tail
          .filter(
            (frame) =>
              frame.type === "output" && typeof frame.data_base64 === "string",
          )
          .map((frame) => {
            const binary = atob(frame.data_base64!);
            return new TextDecoder().decode(
              Uint8Array.from(binary, (character) => character.charCodeAt(0)),
            );
          })
          .join("");
        const hydrated = `${attachment.snapshot.data}${tailOutput}`;
        mockViewportReset(hydrated);
        cursorRef.current = { ...attachment.live_cursor };
        setOutput(hydrated);
        if (!activity || activitySourceRef.current) return;

        const query = new URLSearchParams({
          execution_device: activity.executionDeviceQueryValue,
          next_activity_sequence: String(
            attachment.live_cursor.next_activity_sequence,
          ),
          next_output_offset: String(attachment.live_cursor.next_output_offset),
        });
        const source = new FakeEventSource(
          `http://test/api/terminal/sessions/${encodeURIComponent(
            activity.sessionId,
          )}/activity?${query.toString()}`,
        );
        const recover = (reason: string, error: unknown) => {
          if (recoveryRef.current) return;
          source.close();
          activitySourceRef.current = null;
          activity.onError(error);
          const recovery = activity
            .requestAttachment(reason)
            .then(hydrateRuntimeAttachment)
            .then(() => activity.onRecoveryComplete(reason))
            .catch((recoveryError) =>
              activity.onRecoveryError(reason, recoveryError),
            )
            .finally(() => {
              if (recoveryRef.current === recovery) {
                recoveryRef.current = null;
              }
            });
          recoveryRef.current = recovery;
        };
        source.addEventListener("terminal_activity", (event) => {
          const frame = JSON.parse(event.data) as Parameters<
            typeof applyFrame
          >[0];
          activityApplyQueueRef.current = activityApplyQueueRef.current
            .catch(() => undefined)
            .then(async () => {
              const cursor = cursorRef.current;
              if (
                frame.activity_sequence < cursor.next_activity_sequence ||
                (frame.type === "output" &&
                  frame.end_offset !== undefined &&
                  frame.end_offset <= cursor.next_output_offset)
              ) {
                return;
              }
              if (
                frame.activity_sequence > cursor.next_activity_sequence ||
                (frame.type === "output" &&
                  frame.start_offset !== cursor.next_output_offset)
              ) {
                recover("stream_gap", new Error("terminal stream gap"));
                return;
              }
              await applyFrame(frame);
              activity.onFrame(frame, { ...cursorRef.current });
            });
        });
        source.onerror = (error) => recover("transport_failure", error);
        activitySourceRef.current = source;
      };
      React.useEffect(() => {
        if (!activity || initialAttachStartedRef.current) return;
        initialAttachStartedRef.current = true;
        void activity
          .requestAttachment()
          .then(hydrateRuntimeAttachment)
          .catch(activity.onError);

        // eslint-disable-next-line react-hooks/exhaustive-deps
      }, [activity]);
      React.useEffect(() => {
        mockViewportMount(sessionKey);
        return () => {
          activitySourceRef.current?.close();
          mockViewportUnmount(sessionKey);
        };
      }, [sessionKey]);
      React.useEffect(() => {
        mockViewportGeometryHandler = onGeometryChange;
        onGeometryChange?.(activity.sessionId, { cols: 100, rows: 30 });
        return () => {
          if (mockViewportGeometryHandler === onGeometryChange) {
            mockViewportGeometryHandler = undefined;
          }
        };
      }, [activity.sessionId, onGeometryChange]);
      React.useEffect(() => {
        if (!active || !inputEnabled) {
          mockViewportPaletteHandler = undefined;
          return;
        }
        mockViewportPaletteHandler = onPaletteChange;
        onPaletteChange?.({
          foreground: "#101418",
          background: "#f8f9fa",
          cursor: "#228be6",
        });
        return () => {
          if (mockViewportPaletteHandler === onPaletteChange) {
            mockViewportPaletteHandler = undefined;
          }
        };
      }, [active, inputEnabled, onPaletteChange]);
      React.useImperativeHandle(ref, () => ({
        append: (data: string | Uint8Array) => {
          const text =
            typeof data === "string" ? data : new TextDecoder().decode(data);
          mockViewportAppend(text);
          const gate = nextViewportAppendGate ?? Promise.resolve();
          nextViewportAppendGate = null;
          return gate.then(() => setOutput((current) => current + text));
        },
        hydrate: (snapshot: { data: string }) => {
          mockViewportReset(snapshot.data);
          setOutput(snapshot.data);
          return Promise.resolve();
        },
        resize: (geometry: { cols: number; rows: number }) => {
          mockViewportResize(sessionKey, geometry);
        },
        serialize: () => output,
        hydrateAttachment: hydrateRuntimeAttachment,
        applyLiveFrame: async (frame) => {
          await applyFrame(frame);
          return "apply";
        },
        getCursor: () => ({ ...cursorRef.current }),
        disposeRuntime: () => {
          mockViewportDispose(sessionKey);
          activitySourceRef.current?.close();
          activitySourceRef.current = null;
        },
      }));
      return (
        <div hidden={!active}>
          <pre
            aria-label={
              active ? "Terminal output" : `Terminal output ${sessionKey}`
            }
          >
            {output}
          </pre>
          <span aria-label="Terminal scrollback">{scrollbackLines}</span>
          <span aria-label="Terminal locked geometry">
            {lockedGeometry
              ? `${lockedGeometry.cols}x${lockedGeometry.rows}`
              : "fit"}
          </span>
          <button
            type="button"
            disabled={!inputEnabled}
            onClick={() => onInput?.("echo hi\n")}
          >
            Simulate xterm input
          </button>
          <button
            type="button"
            disabled={!inputEnabled}
            onClick={() => onInput?.("\x03")}
          >
            Simulate Ctrl+C
          </button>
        </div>
      );
    }),
  };
});

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (_key: string, fallback?: string) => fallback ?? _key,
  }),
}));

async function chooseSelectOption(name: string, option: string | RegExp) {
  fireEvent.click(await screen.findByRole("combobox", { name }));
  fireEvent.click(await screen.findByRole("option", { name: option }));
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((innerResolve, innerReject) => {
    resolve = innerResolve;
    reject = innerReject;
  });
  return { promise, resolve, reject };
}

describe("TerminalPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    eventSources.length = 0;
    nextViewportAppendGate = null;
    mockViewportGeometryHandler = undefined;
    mockViewportPaletteHandler = undefined;
    vi.stubGlobal("EventSource", FakeEventSource);
    window.localStorage.clear();
    delete window.__ruaTerminalDiagnostics;
    window.localStorage.setItem("rua-terminal-viewer-id", "browser-test");
    mockListDevices.mockResolvedValue([
      {
        device_id: "local-device",
        label: "MacBook",
        friendly_name: "MacBook",
        is_self: true,
        connectivity_state: "connected",
      },
      {
        device_id: "remote-device",
        label: "Phone",
        friendly_name: "Phone",
        is_self: false,
        connectivity_state: "connected",
      },
    ]);
    mockGetSyncStatus.mockResolvedValue({
      enabled: true,
      paired: true,
      peer_count: 1,
    });
    mockListSessions.mockResolvedValue({ sessions: [] });
    mockListProfiles.mockResolvedValue({
      os: "linux",
      default_profile_id: "bash",
      profiles: [
        { id: "bash", label: "Bash", kind: "bash" },
        { id: "zsh", label: "Zsh", kind: "zsh" },
      ],
    });
    mockOpenSession.mockResolvedValue({
      session: {
        session_id: "terminal-session-1",
        cwd: "/Users/test",
        profile_id: "bash",
        profile_label: "Bash",
        viewer_count: 1,
        input_lease_holder: { browser_id: "browser-test" },
      },
    });
    mockAttachSession.mockImplementation((sessionId: string) =>
      Promise.resolve({
        session: {
          session_id: sessionId,
          cwd: "/Users/test",
          profile_id: "bash",
          profile_label: "Bash",
          viewer_count: 1,
          input_lease_holder: {
            browser_id:
              sessionId.includes("locked") || sessionId.includes("passive")
                ? "other-browser"
                : "browser-test",
          },
        },
        cursor: {
          next_activity_sequence: 0,
          next_output_offset: 0,
        },
        snapshot: null,
        tail: [],
      }),
    );
    mockDetachSession.mockResolvedValue(undefined);
    mockUpdateScrollback.mockResolvedValue(undefined);
    mockKillSession.mockResolvedValue(undefined);
    mockTakeInputLease.mockResolvedValue({
      session: {
        session_id: "terminal-session-locked",
        cwd: "/Users/test",
        profile_id: "bash",
        profile_label: "Bash",
        viewer_count: 2,
        input_lease_holder: { browser_id: "browser-test" },
      },
    });
    mockAdaptPtyGeometry.mockImplementation((sessionId: string) =>
      Promise.resolve({
        session: {
          session_id: sessionId,
          cwd: "/Users/test",
          profile_id: "bash",
          profile_label: "Bash",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
          pty_geometry: { cols: 100, rows: 30 },
        },
      }),
    );
    mockUpdatePalette.mockResolvedValue(undefined);
    mockRenameSession.mockImplementation(
      (sessionId: string, displayName: string | null) =>
        Promise.resolve({
          session: {
            session_id: sessionId,
            cwd: "/Users/test",
            profile_id: "bash",
            profile_label: "Bash",
            display_name: displayName,
            viewer_count: 1,
            input_lease_holder: { browser_id: "browser-test" },
          },
        }),
    );
    mockSendInput.mockResolvedValue(undefined);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("opens Android Debian directly even when a removed profile is stored", async () => {
    window.localStorage.setItem(
      "rua-terminal-default-profile:local",
      "unknown-profile",
    );
    mockListProfiles.mockResolvedValue({
      os: "android",
      default_profile_id: "proot",
      profiles: [{ id: "proot", label: "Debian", kind: "proot" }],
    });
    render(<TerminalPage />);
    await waitFor(() => expect(mockListProfiles).toHaveBeenCalled());
    fireEvent.click(
      await screen.findByRole("button", { name: "New terminal" }),
    );
    await waitFor(() =>
      expect(mockOpenSession).toHaveBeenCalledWith(
        expect.objectContaining({ terminal_profile_id: "proot" }),
      ),
    );
    expect(screen.queryByText("Choose terminal")).not.toBeInTheDocument();
  });

  it("loads execution devices and asks for a terminal profile before creating a session", async () => {
    render(<TerminalPage />);

    const executionDeviceSelect = await screen.findByRole("combobox", {
      name: "Device",
    });
    expect(executionDeviceSelect).toHaveTextContent("MacBook");
    fireEvent.click(executionDeviceSelect);
    expect(
      await screen.findByRole("option", { name: "MacBook" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Phone" })).toHaveTextContent(
      "Phone · Connected",
    );
    expect(mockListSessions).toHaveBeenCalledWith({ kind: "local" });
    await waitFor(() => {
      expect(mockListProfiles).toHaveBeenCalledWith({ kind: "local" });
    });

    fireEvent.click(screen.getByRole("button", { name: "New terminal" }));
    expect(await screen.findByText("Choose terminal")).toBeInTheDocument();
    const createButtons = screen.getAllByRole("button", {
      name: "New terminal",
    });
    fireEvent.click(createButtons[createButtons.length - 1]);

    await waitFor(() => {
      expect(mockOpenSession).toHaveBeenCalledWith({
        execution_device: { kind: "local" },
        browser_viewer_id: expect.any(String),
        scrollback_lines: 5000,
        terminal_profile_id: "bash",
      });
    });
    await waitFor(() => {
      expect(screen.getAllByText("MacBook - Bash").length).toBeGreaterThan(0);
    });
  });

  it("keeps terminal header controls in one compact action group", async () => {
    const { container } = render(<TerminalPage />);

    const deviceSelect = await screen.findByRole("combobox", {
      name: "Device",
    });
    const sessionSelect = screen.getByRole("combobox", { name: "Session" });
    const newTerminal = screen.getByRole("button", { name: "New terminal" });
    const more = screen.getByRole("button", { name: "More" });
    const pageHeader = container.querySelector(".rua-terminal-page-header");
    const actionGroup = pageHeader?.lastElementChild;

    expect(actionGroup).toContainElement(deviceSelect);
    expect(actionGroup).toContainElement(sessionSelect);
    expect(actionGroup).toContainElement(newTerminal);
    expect(actionGroup).toContainElement(more);
    expect(actionGroup?.lastElementChild).toContainElement(more);
    expect(
      newTerminal.compareDocumentPosition(more) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(
      actionGroup?.querySelector(".rua-terminal-header-actions"),
    ).toBeNull();
    expect(deviceSelect.closest("label")).toHaveClass(
      "rua-terminal-device-field",
    );
    expect(sessionSelect.closest("label")).toHaveClass(
      "rua-terminal-session-field",
    );
  });

  it("shows the shared connectivity state for the selected remote Device", async () => {
    mockListDevices.mockResolvedValue([
      {
        device_id: "local-device",
        label: "MacBook",
        friendly_name: "MacBook",
        is_self: true,
        connectivity_state: "connected",
      },
      {
        device_id: "remote-device",
        label: "Phone",
        friendly_name: "Phone",
        is_self: false,
        connectivity_state: "connecting",
      },
    ]);

    render(<TerminalPage />);

    await chooseSelectOption("Device", "Phone");
    expect(await screen.findByText("Connecting")).toBeVisible();
  });

  it("hides the execution device selector when fewer than two devices exist", async () => {
    mockListDevices.mockResolvedValueOnce([
      {
        device_id: "local-device",
        label: "MacBook",
        friendly_name: "MacBook",
        is_self: true,
        connectivity_state: "connected",
      },
    ]);

    render(<TerminalPage />);

    await waitFor(() => expect(mockListDevices).toHaveBeenCalled());
    expect(
      screen.queryByRole("combobox", { name: "Device" }),
    ).not.toBeInTheDocument();
  });

  it("stores the default terminal profile from the terminal options menu", async () => {
    render(<TerminalPage />);

    fireEvent.click(await screen.findByRole("button", { name: "More" }));
    fireEvent.click(await screen.findByText("Terminal options"));

    expect(await screen.findByText("Current system")).toBeInTheDocument();
    expect(screen.getByText("linux")).toBeInTheDocument();
    expect(screen.getByText("Bash, Zsh")).toBeInTheDocument();
    expect(screen.getByLabelText("Default terminal")).toHaveTextContent(
      "Ask every time",
    );

    await chooseSelectOption("Default terminal", "Zsh");

    expect(
      window.localStorage.getItem("rua-terminal-default-profile:local"),
    ).toBe("zsh");
  });

  it("stores terminal scrollback locally", async () => {
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-scrollback",
          cwd: "/Users/test",
          profile_id: "bash",
          profile_label: "Bash",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
          pty_geometry: { cols: 100, rows: 30 },
        },
      ],
    });
    render(<TerminalPage />);

    expect(
      await screen.findByLabelText("Terminal scrollback"),
    ).toHaveTextContent("5000");
    await waitFor(() => {
      expect(mockAttachScrollback).toHaveBeenCalledWith(
        "terminal-session-scrollback",
        5000,
        { kind: "local" },
      );
    });

    fireEvent.click(await screen.findByRole("button", { name: "More" }));
    fireEvent.click(await screen.findByText("Terminal options"));

    const scrollbackInput = await screen.findByLabelText("Scrollback lines");

    expect(scrollbackInput).toHaveValue(5000);

    fireEvent.change(scrollbackInput, { target: { value: "12000" } });

    expect(window.localStorage.getItem("rua-terminal-scrollback-lines")).toBe(
      "12000",
    );
    expect(screen.getByLabelText("Terminal scrollback")).toHaveTextContent(
      "12000",
    );
    await waitFor(() => {
      expect(mockUpdateScrollback).toHaveBeenCalledWith(
        expect.any(String),
        12000,
        { kind: "local" },
      );
    });
    expect(
      screen.queryByLabelText("Session cache (MiB)"),
    ).not.toBeInTheDocument();
  });

  it("can restore terminate confirmation from terminal options", async () => {
    window.localStorage.setItem("rua-terminal-skip-kill-confirm", "true");
    render(<TerminalPage />);

    fireEvent.click(await screen.findByRole("button", { name: "More" }));
    fireEvent.click(await screen.findByText("Terminal options"));

    const confirmToggle = await screen.findByLabelText(
      "Confirm before terminating",
    );
    expect(confirmToggle).not.toBeChecked();

    fireEvent.click(confirmToggle);
    expect(
      window.localStorage.getItem("rua-terminal-skip-kill-confirm"),
    ).toBe(null);
    expect(confirmToggle).toBeChecked();

    fireEvent.click(confirmToggle);
    expect(
      window.localStorage.getItem("rua-terminal-skip-kill-confirm"),
    ).toBe("true");
    expect(confirmToggle).not.toBeChecked();
  });

  it("restores the last selected local terminal session", async () => {
    window.localStorage.setItem(
      "rua-terminal-active-session:device:local-device:local",
      "terminal-session-beta",
    );
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-alpha",
          cwd: "/Users/alpha",
          profile_id: "alpha",
          profile_label: "alpha",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
        {
          session_id: "terminal-session-beta",
          cwd: "/Users/beta",
          profile_id: "beta",
          profile_label: "beta",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });
    render(<TerminalPage />);

    await waitFor(() => {
      expect(mockAttachSession).toHaveBeenCalledWith(
        "terminal-session-beta",
        "browser-test",
        { kind: "local" },
      );
    });
    expect(await screen.findByTitle("terminal-session-beta")).toBeVisible();
  });

  it("restores the last selected remote terminal filter", async () => {
    window.localStorage.setItem(
      "rua-terminal-active-execution-device:device:local-device",
      "remote:remote-device",
    );
    window.localStorage.setItem(
      "rua-terminal-active-session:device:local-device:remote:remote-device",
      "terminal-session-phone",
    );
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-phone",
          cwd: "/home/phone",
          profile_id: "bash",
          profile_label: "Bash",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });

    render(<TerminalPage />);

    await waitFor(() => {
      expect(mockListSessions).toHaveBeenCalledWith({
        kind: "remote",
        device_id: "remote-device",
      });
      expect(mockAttachSession).toHaveBeenCalledWith(
        "terminal-session-phone",
        "browser-test",
        { kind: "remote", device_id: "remote-device" },
      );
    });
    expect(await screen.findByLabelText("Device")).toHaveTextContent("Phone");
    expect(await screen.findByTitle("terminal-session-phone")).toBeVisible();
  });

  it("allows remote input from Not connected so the operation can reconnect", async () => {
    window.localStorage.setItem(
      "rua-terminal-active-execution-device:device:local-device",
      "remote:remote-device",
    );
    mockListDevices.mockResolvedValue([
      {
        device_id: "local-device",
        label: "MacBook",
        friendly_name: "MacBook",
        is_self: true,
        connectivity_state: "connected",
      },
      {
        device_id: "remote-device",
        label: "Phone",
        friendly_name: "Phone",
        is_self: false,
        connectivity_state: "not_connected",
      },
    ]);
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-phone",
          cwd: "/home/phone",
          profile_id: "bash",
          profile_label: "Bash",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });

    render(<TerminalPage />);

    const input = await screen.findByRole("button", {
      name: "Simulate xterm input",
    });
    await waitFor(() => expect(input).not.toBeDisabled());
    fireEvent.click(input);

    await waitFor(() =>
      expect(mockSendInput).toHaveBeenCalledWith(
        "terminal-session-phone",
        "browser-test",
        "echo hi\n",
        { kind: "remote", device_id: "remote-device" },
      ),
    );
  });

  it("ignores terminal restore keys scoped to another device", async () => {
    window.localStorage.setItem(
      "rua-terminal-active-execution-device:device:other-device",
      "remote:remote-device",
    );
    window.localStorage.setItem(
      "rua-terminal-active-session:device:other-device:remote:remote-device",
      "terminal-session-phone",
    );
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-local",
          cwd: "/Users/local",
          profile_id: "bash",
          profile_label: "Bash",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });

    render(<TerminalPage />);

    await waitFor(() => {
      expect(mockListSessions).toHaveBeenCalledWith({ kind: "local" });
      expect(mockAttachSession).toHaveBeenCalledWith(
        "terminal-session-local",
        "browser-test",
        { kind: "local" },
      );
    });
    expect(await screen.findByLabelText("Device")).toHaveTextContent("MacBook");
    expect(await screen.findByTitle("terminal-session-local")).toBeVisible();
  });

  it("keeps local terminal surfaces mounted when switching sessions", async () => {
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-alpha",
          cwd: "/Users/test",
          profile_id: "bash",
          profile_label: "Bash",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
        {
          session_id: "terminal-session-beta",
          cwd: "/Users/test",
          profile_id: "bash",
          profile_label: "Bash",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });
    mockAttachSession.mockImplementation((sessionId: string) =>
      Promise.resolve({
        session: {
          session_id: sessionId,
          cwd: "/Users/test",
          profile_id: "bash",
          profile_label: "Bash",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
        replay: [
          {
            type: "output",
            data: sessionId.endsWith("alpha")
              ? "alpha replay\n"
              : "beta replay\n",
          },
        ],
      }),
    );

    render(<TerminalPage />);

    await waitFor(() => {
      expect(screen.getByLabelText("Terminal output")).toHaveTextContent(
        "alpha replay",
      );
    });
    await waitFor(() => expect(eventSources).toHaveLength(1));
    act(() => {
      eventSources[0].emit({ type: "output", data: "alpha live\n" });
    });
    await waitFor(() => {
      expect(screen.getByLabelText("Terminal output")).toHaveTextContent(
        "alpha live",
      );
    });
    expect(
      mockViewportMount.mock.calls.filter(
        ([sessionKey]) =>
          sessionKey === "local:terminal-session-alpha:browser-test",
      ),
    ).toHaveLength(1);

    await chooseSelectOption("Session", /beta/);
    await waitFor(() => {
      expect(screen.getByLabelText("Terminal output")).toHaveTextContent(
        "beta replay",
      );
    });
    expect(
      window.localStorage.getItem(
        "rua-terminal-active-session:device:local-device:local",
      ),
    ).toBe("terminal-session-beta");
    expect(mockViewportMount).toHaveBeenCalledWith(
      "local:terminal-session-beta:browser-test",
    );
    expect(mockViewportUnmount).not.toHaveBeenCalledWith(
      "local:terminal-session-alpha:browser-test",
    );

    await chooseSelectOption("Session", /alpha/);
    await waitFor(() => {
      expect(screen.getByLabelText("Terminal output")).toHaveTextContent(
        "alpha replay",
      );
    });
    expect(screen.getByLabelText("Terminal output")).toHaveTextContent(
      "alpha live",
    );
    expect(
      window.localStorage.getItem(
        "rua-terminal-active-session:device:local-device:local",
      ),
    ).toBe("terminal-session-alpha");
    expect(
      mockViewportMount.mock.calls.filter(
        ([sessionKey]) =>
          sessionKey === "local:terminal-session-alpha:browser-test",
      ),
    ).toHaveLength(1);
    expect(mockViewportUnmount).not.toHaveBeenCalledWith(
      "local:terminal-session-alpha:browser-test",
    );

    await chooseSelectOption("Session", /beta/);
    expect(screen.getByLabelText("Terminal output")).toHaveTextContent(
      "beta replay",
    );
    expect(mockAttachSession).toHaveBeenCalledTimes(2);
  });

  it("does not reattach or reset a retained terminal when switching sessions", async () => {
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-alpha-snapshot",
          cwd: "/Users/test",
          profile_id: "bash",
          profile_label: "Bash",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
        {
          session_id: "terminal-session-beta-snapshot",
          cwd: "/Users/test",
          profile_id: "bash",
          profile_label: "Bash",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });
    mockAttachSession.mockImplementation((sessionId: string) =>
      Promise.resolve({
        session: {
          session_id: sessionId,
          cwd: "/Users/test",
          profile_id: "bash",
          profile_label: "Bash",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
        snapshot: sessionId.includes("alpha")
          ? "alpha snapshot\n"
          : "beta snapshot\n",
      }),
    );

    render(<TerminalPage />);

    await waitFor(() => {
      expect(screen.getByLabelText("Terminal output")).toHaveTextContent(
        "alpha snapshot",
      );
    });
    await chooseSelectOption("Session", /beta-sna/);
    await waitFor(() => {
      expect(screen.getByLabelText("Terminal output")).toHaveTextContent(
        "beta snapshot",
      );
    });
    await chooseSelectOption("Session", /alpha-sn/);
    await waitFor(() => {
      expect(screen.getByLabelText("Terminal output")).toHaveTextContent(
        "alpha snapshot",
      );
    });

    expect(mockAttachSession).toHaveBeenCalledTimes(2);
    expect(
      mockViewportReset.mock.calls.filter(
        ([output]) => output === "alpha snapshot\n",
      ),
    ).toHaveLength(1);
  });

  it("retains the terminal route host across visibility changes", async () => {
    window.localStorage.setItem("rua-terminal-diagnostics", "1");
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-route-visibility",
          cwd: "/Users/test",
          profile_id: "bash",
          profile_label: "Bash",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });
    const view = render(<TerminalPage routeActive />);
    await waitFor(() => expect(eventSources).toHaveLength(1));

    view.rerender(<TerminalPage routeActive={false} />);
    expect(
      screen.getByRole("button", {
        name: "Simulate xterm input",
        hidden: true,
      }),
    ).toBeDisabled();
    view.rerender(<TerminalPage routeActive />);

    expect(
      (window.__ruaTerminalDiagnostics ?? [])
        .filter(
          (event) =>
            event.event === "visibility_change" &&
            event.visibility_scope === "route",
        )
        .map((event) => event.visible),
    ).toEqual(expect.arrayContaining([false, true]));
  });

  it("performs a cold attach after remount instead of replaying a raw browser cache", async () => {
    const session = {
      session_id: "terminal-session-remount-cache",
      cwd: "/Users/test",
      profile_id: "bash",
      profile_label: "Bash",
      viewer_count: 1,
      input_lease_holder: { browser_id: "browser-test" },
    };
    mockListSessions.mockResolvedValue({ sessions: [session] });
    mockAttachSession.mockResolvedValueOnce({
      session,
      snapshot: "cached before navigation\n",
    });

    const firstRender = render(<TerminalPage />);
    await waitFor(() => {
      expect(screen.getByLabelText("Terminal output")).toHaveTextContent(
        "cached before navigation",
      );
    });
    firstRender.unmount();
    const eventSourceCountBeforeRemount = eventSources.length;

    const reattach = deferred<{
      session: typeof session;
      replay: Array<{ type: "output"; data: string }>;
      snapshot?: string | null;
    }>();
    mockAttachSession.mockReturnValueOnce(reattach.promise);
    render(<TerminalPage />);

    await waitFor(() => expect(mockAttachSession).toHaveBeenCalledTimes(2));
    expect(screen.getByLabelText("Terminal output")).toHaveTextContent("");
    expect(eventSources).toHaveLength(eventSourceCountBeforeRemount);

    await act(async () => {
      reattach.resolve({
        session,
        replay: [],
        snapshot: "restored after navigation\n",
      });
      await reattach.promise;
    });
    await waitFor(() => {
      expect(screen.getByLabelText("Terminal output")).toHaveTextContent(
        "restored after navigation",
      );
    });
    expect(eventSources).toHaveLength(eventSourceCountBeforeRemount + 1);
    expect(screen.getByLabelText("Terminal output")).not.toHaveTextContent(
      "cached before navigation",
    );
  });

  it("asks for a terminal profile when the default profile is ask every time", async () => {
    render(<TerminalPage />);

    fireEvent.click(
      await screen.findByRole("button", { name: "New terminal" }),
    );

    expect(await screen.findByText("Choose terminal")).toBeInTheDocument();
    await chooseSelectOption("Terminal", "Zsh");
    const createButtons = screen.getAllByRole("button", {
      name: "New terminal",
    });
    fireEvent.click(createButtons[createButtons.length - 1]);

    await waitFor(() => {
      expect(mockOpenSession).toHaveBeenCalledWith({
        execution_device: { kind: "local" },
        browser_viewer_id: expect.any(String),
        scrollback_lines: 5000,
        terminal_profile_id: "zsh",
      });
    });
  });

  it("stores the selected terminal profile from the create dialog when opting out of future prompts", async () => {
    render(<TerminalPage />);

    fireEvent.click(
      await screen.findByRole("button", { name: "New terminal" }),
    );

    expect(await screen.findByText("Choose terminal")).toBeInTheDocument();
    await chooseSelectOption("Terminal", "Zsh");
    fireEvent.click(screen.getByLabelText("Do not ask again"));
    const createButtons = screen.getAllByRole("button", {
      name: "New terminal",
    });
    fireEvent.click(createButtons[createButtons.length - 1]);

    await waitFor(() => {
      expect(mockOpenSession).toHaveBeenCalledWith({
        execution_device: { kind: "local" },
        browser_viewer_id: expect.any(String),
        scrollback_lines: 5000,
        terminal_profile_id: "zsh",
      });
    });
    expect(
      window.localStorage.getItem("rua-terminal-default-profile:local"),
    ).toBe("zsh");
  });

  it("locks input until the viewer takes the input lease", async () => {
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-locked",
          cwd: "/Users/test",
          viewer_count: 2,
          input_lease_holder: { browser_id: "other-browser" },
        },
      ],
    });

    render(<TerminalPage />);

    await waitFor(() => {
      expect(screen.getAllByText("MacBook - Bash").length).toBeGreaterThan(0);
    });
    expect(
      screen.getByRole("button", { name: "Simulate xterm input" }),
    ).toBeDisabled();

    expect(screen.queryByText("Unlock input")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Take input lease" }));

    await waitFor(() => {
      expect(mockTakeInputLease).toHaveBeenCalledWith(
        "terminal-session-locked",
        expect.any(String),
        { kind: "local" },
      );
    });
    const input = screen.getByRole("button", {
      name: "Simulate xterm input",
    });
    await waitFor(() => expect(input).not.toBeDisabled());

    fireEvent.click(input);

    await waitFor(() => {
      expect(mockSendInput).toHaveBeenCalledWith(
        "terminal-session-locked",
        expect.any(String),
        "echo hi\n",
        { kind: "local" },
      );
    });
  });

  it("waits for fresh owner geometry after taking the input lease", async () => {
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-locked-takeover-geometry",
          cwd: "/Users/test",
          viewer_count: 2,
          input_lease_holder: { browser_id: "other-browser" },
          pty_geometry: { cols: 155, rows: 40 },
        },
      ],
    });
    mockTakeInputLease.mockResolvedValueOnce({
      session: {
        session_id: "terminal-session-locked-takeover-geometry",
        cwd: "/Users/test",
        profile_id: "bash",
        profile_label: "Bash",
        viewer_count: 2,
        input_lease_holder: { browser_id: "browser-test" },
        pty_geometry: { cols: 155, rows: 40 },
      },
    });

    render(<TerminalPage />);

    await waitFor(() => expect(mockViewportGeometryHandler).toBeDefined());
    await waitFor(() => expect(mockAttachSession).toHaveBeenCalledTimes(1));
    const resetsBeforeTakeover = mockViewportReset.mock.calls.length;
    mockAdaptPtyGeometry.mockClear();
    fireEvent.click(screen.getByRole("button", { name: "Take input lease" }));
    await waitFor(() => {
      expect(mockTakeInputLease).toHaveBeenCalledWith(
        "terminal-session-locked-takeover-geometry",
        "browser-test",
        { kind: "local" },
      );
    });
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Simulate xterm input" }),
      ).not.toBeDisabled(),
    );

    expect(mockAdaptPtyGeometry).not.toHaveBeenCalled();

    act(() => {
      mockViewportGeometryHandler?.(
        "terminal-session-locked-takeover-geometry",
        {
          cols: 92,
          rows: 28,
        },
      );
    });

    await waitFor(() => {
      expect(mockAdaptPtyGeometry).toHaveBeenCalledTimes(1);
      expect(mockAdaptPtyGeometry).toHaveBeenCalledWith(
        "terminal-session-locked-takeover-geometry",
        "browser-test",
        92,
        28,
        { kind: "local" },
      );
    });
    expect(mockAttachSession).toHaveBeenCalledTimes(1);
    expect(mockViewportReset).toHaveBeenCalledTimes(resetsBeforeTakeover);
  });

  it("updates the shared PTY palette only from the active input lease holder", async () => {
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-palette",
          cwd: "/Users/test",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });

    render(<TerminalPage />);

    await waitFor(() => {
      expect(mockUpdatePalette).toHaveBeenCalledWith(
        "terminal-session-palette",
        "browser-test",
        {
          foreground: "#101418",
          background: "#f8f9fa",
          cursor: "#228be6",
        },
        { kind: "local" },
      );
    });
    expect(mockUpdatePalette).toHaveBeenCalledTimes(1);

    act(() => {
      mockViewportPaletteHandler?.({
        foreground: "#101418",
        background: "#f8f9fa",
        cursor: "#228be6",
      });
      mockViewportPaletteHandler?.({
        foreground: "#101418",
        background: "#f8f9fa",
        cursor: "#228be6",
      });
    });
    expect(mockUpdatePalette).toHaveBeenCalledTimes(1);
  });

  it("does not let passive viewers update the shared PTY palette", async () => {
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-passive-palette",
          cwd: "/Users/test",
          viewer_count: 2,
          input_lease_holder: { browser_id: "other-browser" },
        },
      ],
    });

    render(<TerminalPage />);

    await waitFor(() => {
      expect(screen.getAllByText("MacBook - Bash").length).toBeGreaterThan(0);
    });
    expect(mockUpdatePalette).not.toHaveBeenCalled();
  });

  it("locks input when activity reports that another viewer took the lease", async () => {
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-active",
          cwd: "/Users/test",
          viewer_count: 2,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });
    mockAttachSession.mockResolvedValueOnce({
      session: {
        session_id: "terminal-session-active",
        cwd: "/Users/test",
        profile_id: "bash",
        profile_label: "Bash",
        viewer_count: 2,
        input_lease_holder: { browser_id: "browser-test" },
      },
      replay: [],
    });

    render(<TerminalPage />);

    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Simulate xterm input" }),
      ).not.toBeDisabled(),
    );
    await waitFor(() => expect(eventSources).toHaveLength(1));

    eventSources[0].emit({
      type: "input_lease_changed",
      holder: { browser_id: "other-browser" },
    });

    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Simulate xterm input" }),
      ).toBeDisabled(),
    );
  });

  it("forwards Ctrl+C without reconnecting or replaying terminal state", async () => {
    const snapshotData = "codex history\n";
    const snapshotOffset = new TextEncoder().encode(snapshotData).byteLength;
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-codex-interrupt",
          cwd: "/Users/test",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });
    mockAttachSession.mockResolvedValueOnce({
      session: {
        session_id: "terminal-session-codex-interrupt",
        cwd: "/Users/test",
        profile_id: "bash",
        profile_label: "Bash",
        viewer_count: 1,
        input_lease_holder: { browser_id: "browser-test" },
      },
      cursor: {
        next_activity_sequence: 0,
        next_output_offset: snapshotOffset,
      },
      snapshot: {
        data: snapshotData,
        cols: 80,
        rows: 24,
        output_offset: snapshotOffset,
      },
      tail: [],
    });

    render(<TerminalPage />);

    await waitFor(() => {
      expect(screen.getByLabelText("Terminal output")).toHaveTextContent(
        "codex history",
      );
      expect(eventSources).toHaveLength(1);
    });
    const source = eventSources[0];

    fireEvent.click(screen.getByRole("button", { name: "Simulate Ctrl+C" }));
    await waitFor(() => {
      expect(mockSendInput).toHaveBeenCalledWith(
        "terminal-session-codex-interrupt",
        "browser-test",
        "\x03",
        { kind: "local" },
      );
    });

    act(() => {
      source.emit({ type: "output", data: "^C\n" });
      source.emit({
        type: "input_lease_changed",
        holder: { browser_id: "browser-test" },
      });
    });

    await waitFor(() => {
      expect(screen.getByLabelText("Terminal output")).toHaveTextContent("^C");
    });
    expect(eventSources).toHaveLength(1);
    expect(source.closed).toBe(false);
    expect(mockAttachSession).toHaveBeenCalledTimes(1);
    expect(mockViewportReset).toHaveBeenCalledTimes(1);
  });

  it("renders terminal output received from activity frames", async () => {
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-output",
          cwd: "/Users/test",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });

    render(<TerminalPage />);

    await waitFor(() => {
      expect(screen.getAllByText("MacBook - Bash").length).toBeGreaterThan(0);
    });
    await waitFor(() => expect(eventSources).toHaveLength(1));

    eventSources[0].emit({
      type: "output",
      data: "hello from terminal\n",
    });

    await waitFor(() => {
      expect(screen.getByLabelText("Terminal output")).toHaveTextContent(
        "hello from terminal",
      );
    });
  });

  it("automatically adapts PTY geometry to the lease holder viewport", async () => {
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-active",
          cwd: "/Users/test",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });

    render(<TerminalPage />);

    await waitFor(() => {
      expect(screen.getAllByText("MacBook - Bash").length).toBeGreaterThan(0);
    });
    await waitFor(() => {
      expect(mockAdaptPtyGeometry).toHaveBeenCalledWith(
        "terminal-session-active",
        "browser-test",
        100,
        30,
        { kind: "local" },
      );
    });
    expect(
      screen.queryByRole("button", { name: "Adapt PTY to viewport" }),
    ).not.toBeInTheDocument();
    expect(mockAttachSession).toHaveBeenCalledTimes(1);
    expect(mockViewportReset).toHaveBeenCalledTimes(1);
    expect(mockViewportReset).toHaveBeenCalledWith("");
  });

  it("coalesces intermediate viewport sizes into one automatic final adaptation", async () => {
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-coalesced-geometry",
          cwd: "/Users/test",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });

    render(<TerminalPage />);
    await waitFor(() => expect(mockViewportGeometryHandler).toBeDefined());
    await waitFor(() => {
      expect(mockAdaptPtyGeometry).toHaveBeenCalledWith(
        "terminal-session-coalesced-geometry",
        "browser-test",
        100,
        30,
        { kind: "local" },
      );
    });
    mockAdaptPtyGeometry.mockClear();
    await act(async () => {
      mockViewportGeometryHandler?.("terminal-session-coalesced-geometry", {
        cols: 90,
        rows: 25,
      });
      mockViewportGeometryHandler?.("terminal-session-coalesced-geometry", {
        cols: 95,
        rows: 27,
      });
      mockViewportGeometryHandler?.("terminal-session-coalesced-geometry", {
        cols: 120,
        rows: 32,
      });
      await Promise.resolve();
    });

    expect(mockAdaptPtyGeometry).toHaveBeenCalledTimes(1);
    expect(mockAdaptPtyGeometry).toHaveBeenCalledWith(
      "terminal-session-coalesced-geometry",
      "browser-test",
      120,
      32,
      { kind: "local" },
    );
  });

  it("serializes in-flight geometry adaptation and keeps only the latest size", async () => {
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-serialized-geometry",
          cwd: "/Users/test",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
          pty_geometry: { cols: 100, rows: 30 },
        },
      ],
    });
    const firstRequest = deferred<{
      session: {
        session_id: string;
        cwd: string;
        profile_id: string;
        profile_label: string;
        viewer_count: number;
        input_lease_holder: { browser_id: string };
        pty_geometry: { cols: number; rows: number };
      };
    }>();

    render(<TerminalPage />);
    await waitFor(() => expect(mockViewportGeometryHandler).toBeDefined());
    mockAdaptPtyGeometry.mockReset();
    mockAdaptPtyGeometry
      .mockReturnValueOnce(firstRequest.promise)
      .mockImplementation(
        async (
          sessionId: string,
          _browserViewerId: string,
          cols: number,
          rows: number,
        ) => ({
          session: {
            session_id: sessionId,
            cwd: "/Users/test",
            profile_id: "bash",
            profile_label: "Bash",
            viewer_count: 1,
            input_lease_holder: { browser_id: "browser-test" },
            pty_geometry: { cols, rows },
          },
        }),
      );

    act(() => {
      mockViewportGeometryHandler?.("terminal-session-serialized-geometry", {
        cols: 90,
        rows: 25,
      });
    });
    await waitFor(() => expect(mockAdaptPtyGeometry).toHaveBeenCalledTimes(1));

    act(() => {
      mockViewportGeometryHandler?.("terminal-session-serialized-geometry", {
        cols: 95,
        rows: 27,
      });
      mockViewportGeometryHandler?.("terminal-session-serialized-geometry", {
        cols: 120,
        rows: 32,
      });
    });
    expect(mockAdaptPtyGeometry).toHaveBeenCalledTimes(1);

    firstRequest.resolve({
      session: {
        session_id: "terminal-session-serialized-geometry",
        cwd: "/Users/test",
        profile_id: "bash",
        profile_label: "Bash",
        viewer_count: 1,
        input_lease_holder: { browser_id: "browser-test" },
        pty_geometry: { cols: 90, rows: 25 },
      },
    });

    await waitFor(() => expect(mockAdaptPtyGeometry).toHaveBeenCalledTimes(2));
    expect(mockAdaptPtyGeometry).toHaveBeenLastCalledWith(
      "terminal-session-serialized-geometry",
      "browser-test",
      120,
      32,
      { kind: "local" },
    );
  });

  it("suppresses automatic adaptation when PTY geometry already matches", async () => {
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-current-geometry",
          cwd: "/Users/test",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
          pty_geometry: { cols: 100, rows: 30 },
        },
      ],
    });
    mockAttachSession.mockResolvedValueOnce({
      session: {
        session_id: "terminal-session-current-geometry",
        cwd: "/Users/test",
        profile_id: "bash",
        profile_label: "Bash",
        viewer_count: 1,
        input_lease_holder: { browser_id: "browser-test" },
        pty_geometry: { cols: 100, rows: 30 },
      },
      cursor: {
        next_activity_sequence: 0,
        next_output_offset: 0,
      },
      snapshot: null,
      tail: [],
    });

    render(<TerminalPage />);
    await waitFor(() => {
      expect(screen.getAllByText("MacBook - Bash").length).toBeGreaterThan(0);
    });
    await new Promise((resolve) => window.setTimeout(resolve, 250));

    expect(mockAdaptPtyGeometry).not.toHaveBeenCalled();
  });

  it("does not let passive viewers adapt PTY geometry", async () => {
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-passive",
          cwd: "/Users/test",
          viewer_count: 2,
          input_lease_holder: { browser_id: "other-browser" },
        },
      ],
    });

    render(<TerminalPage />);

    await waitFor(() => {
      expect(screen.getAllByText("MacBook - Bash").length).toBeGreaterThan(0);
    });
    expect(
      screen.queryByRole("button", { name: "Adapt PTY to viewport" }),
    ).not.toBeInTheDocument();
    expect(mockAdaptPtyGeometry).not.toHaveBeenCalled();
  });

  it("renders passive viewers with the lease holder PTY geometry", async () => {
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-passive-geometry",
          cwd: "/Users/test",
          viewer_count: 2,
          input_lease_holder: { browser_id: "other-browser" },
          pty_geometry: { cols: 155, rows: 40 },
        },
      ],
    });
    mockAttachSession.mockResolvedValueOnce({
      session: {
        session_id: "terminal-session-passive-geometry",
        cwd: "/Users/test",
        profile_id: "bash",
        profile_label: "Bash",
        viewer_count: 2,
        input_lease_holder: { browser_id: "other-browser" },
        pty_geometry: { cols: 155, rows: 40 },
      },
      replay: [],
    });

    render(<TerminalPage />);

    await waitFor(() => {
      expect(
        screen.getByLabelText("Terminal locked geometry"),
      ).toHaveTextContent("155x40");
    });
    expect(mockAdaptPtyGeometry).not.toHaveBeenCalled();
    await waitFor(() => expect(eventSources).toHaveLength(1));

    eventSources[0].emit({
      type: "pty_geometry_changed",
      geometry: { cols: 120, rows: 32 },
    });

    await waitFor(() => {
      expect(
        screen.getByLabelText("Terminal locked geometry"),
      ).toHaveTextContent("120x32");
    });
    expect(mockViewportResize).toHaveBeenCalledWith(
      "local:terminal-session-passive-geometry:browser-test",
      { cols: 120, rows: 32 },
    );
  });

  it("applies authority geometry before the next output frame", async () => {
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-geometry-order",
          cwd: "/Users/test",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });

    render(<TerminalPage />);
    await waitFor(() => expect(eventSources).toHaveLength(1));

    act(() => {
      eventSources[0].emit({
        type: "pty_geometry_changed",
        geometry: { cols: 88, rows: 26 },
      });
      eventSources[0].emit({ type: "output", data: "redrawn footer\n" });
    });

    await waitFor(() =>
      expect(mockViewportAppend).toHaveBeenCalledWith("redrawn footer\n"),
    );
    expect(mockViewportResize.mock.invocationCallOrder.at(-1)).toBeLessThan(
      mockViewportAppend.mock.invocationCallOrder.at(-1)!,
    );
  });

  it("renders terminal activity errors without leaking transport names", async () => {
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-error",
          cwd: "/Users/test",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });

    render(<TerminalPage />);

    await waitFor(() => expect(eventSources).toHaveLength(1));
    eventSources[0].emit({
      type: "error",
      detail: "remote terminal stream failed",
    });

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "remote terminal stream failed",
    );
    expect(screen.getByRole("alert")).not.toHaveTextContent(
      "remote conversation input",
    );
  });

  it("removes the live session when terminal activity reports exit", async () => {
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-exit",
          cwd: "/Users/test",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });

    render(<TerminalPage />);

    await waitFor(() => {
      expect(screen.getAllByText("MacBook - Bash").length).toBeGreaterThan(0);
    });
    await waitFor(() => expect(eventSources).toHaveLength(1));

    eventSources[0].emit({ type: "exited" });

    await waitFor(() => {
      expect(screen.getAllByText("No terminal session").length).toBeGreaterThan(
        0,
      );
    });
    expect(screen.queryByText("MacBook - Bash")).not.toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Terminal exited");
    expect(mockViewportDispose).toHaveBeenCalledTimes(1);
    expect(mockViewportDispose).toHaveBeenCalledWith(
      "local:terminal-session-exit:browser-test",
    );
  });

  it("selects another live session after killing the active terminal", async () => {
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-kill-active",
          cwd: "/Users/test",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
        {
          session_id: "terminal-session-kill-next",
          cwd: "/Users/test",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });

    render(<TerminalPage />);

    await waitFor(() => {
      expect(screen.getAllByText("MacBook - Bash").length).toBeGreaterThan(0);
    });
    fireEvent.click(screen.getByRole("button", { name: "Kill terminal" }));
    fireEvent.click(await screen.findByRole("button", { name: "Terminate" }));

    await waitFor(() => {
      expect(mockKillSession).toHaveBeenCalledWith(
        "terminal-session-kill-active",
        "browser-test",
        { kind: "local" },
      );
    });
    await waitFor(() => {
      expect(mockAttachSession).toHaveBeenCalledWith(
        "terminal-session-kill-next",
        "browser-test",
        { kind: "local" },
      );
    });
    expect(screen.queryByText("No terminal session")).not.toBeInTheDocument();
  });

  it("clears cached terminal output after killing a session", async () => {
    window.localStorage.setItem("rua-terminal-default-profile:local", "bash");
    const session = {
      session_id: "terminal-session-cache-clear",
      cwd: "/Users/test",
      profile_id: "bash",
      profile_label: "Bash",
      viewer_count: 1,
      input_lease_holder: { browser_id: "browser-test" },
    };
    const secondAttach = deferred<{
      session: typeof session;
      replay: Array<{ type: "output"; data: string }>;
    }>();
    let attachCount = 0;
    mockListSessions.mockResolvedValueOnce({ sessions: [session] });
    mockAttachSession.mockImplementation(() => {
      attachCount += 1;
      if (attachCount === 1) {
        return Promise.resolve({
          session,
          replay: [{ type: "output", data: "initial replay\n" }],
        });
      }
      return secondAttach.promise;
    });
    mockOpenSession.mockResolvedValueOnce({ session });

    render(<TerminalPage />);

    await waitFor(() => {
      expect(screen.getByLabelText("Terminal output")).toHaveTextContent(
        "initial replay",
      );
    });
    await waitFor(() => expect(eventSources).toHaveLength(1));
    act(() => {
      eventSources[0].emit({ type: "output", data: "stale live output\n" });
    });
    await waitFor(() => {
      expect(screen.getByLabelText("Terminal output")).toHaveTextContent(
        "stale live output",
      );
    });

    fireEvent.click(screen.getByRole("button", { name: "Kill terminal" }));
    fireEvent.click(await screen.findByRole("button", { name: "Terminate" }));
    await waitFor(() => {
      expect(screen.getAllByText("No terminal session").length).toBeGreaterThan(
        0,
      );
    });

    fireEvent.click(screen.getByRole("button", { name: "New terminal" }));
    await waitFor(() => expect(mockAttachSession).toHaveBeenCalledTimes(2));
    expect(screen.getByLabelText("Terminal output")).not.toHaveTextContent(
      "stale live output",
    );
    expect(screen.getByLabelText("Terminal output")).not.toHaveTextContent(
      "initial replay",
    );

    await act(async () => {
      secondAttach.resolve({
        session,
        replay: [{ type: "output", data: "fresh replay\n" }],
      });
    });
    await waitFor(() => {
      expect(screen.getByLabelText("Terminal output")).toHaveTextContent(
        "fresh replay",
      );
    });
  });

  it("clears cached terminal output after activity reports exit", async () => {
    window.localStorage.setItem("rua-terminal-default-profile:local", "bash");
    const session = {
      session_id: "terminal-session-exit-cache-clear",
      cwd: "/Users/test",
      profile_id: "bash",
      profile_label: "Bash",
      viewer_count: 1,
      input_lease_holder: { browser_id: "browser-test" },
    };
    const secondAttach = deferred<{
      session: typeof session;
      replay: Array<{ type: "output"; data: string }>;
    }>();
    let attachCount = 0;
    mockListSessions.mockResolvedValueOnce({ sessions: [session] });
    mockAttachSession.mockImplementation(() => {
      attachCount += 1;
      if (attachCount === 1) {
        return Promise.resolve({
          session,
          replay: [{ type: "output", data: "before exit replay\n" }],
        });
      }
      return secondAttach.promise;
    });
    mockOpenSession.mockResolvedValueOnce({ session });

    render(<TerminalPage />);

    await waitFor(() => {
      expect(screen.getByLabelText("Terminal output")).toHaveTextContent(
        "before exit replay",
      );
    });
    await waitFor(() => expect(eventSources).toHaveLength(1));
    act(() => {
      eventSources[0].emit({ type: "output", data: "before exit live\n" });
    });
    await waitFor(() => {
      expect(screen.getByLabelText("Terminal output")).toHaveTextContent(
        "before exit live",
      );
    });

    act(() => {
      eventSources[0].emit({ type: "exited" });
    });
    await waitFor(() => {
      expect(screen.getAllByText("No terminal session").length).toBeGreaterThan(
        0,
      );
    });

    fireEvent.click(screen.getByRole("button", { name: "New terminal" }));
    await waitFor(() => expect(mockAttachSession).toHaveBeenCalledTimes(2));
    expect(screen.getByLabelText("Terminal output")).not.toHaveTextContent(
      "before exit live",
    );
    expect(screen.getByLabelText("Terminal output")).not.toHaveTextContent(
      "before exit replay",
    );

    await act(async () => {
      secondAttach.resolve({
        session,
        replay: [{ type: "output", data: "after exit replay\n" }],
      });
    });
    await waitFor(() => {
      expect(screen.getByLabelText("Terminal output")).toHaveTextContent(
        "after exit replay",
      );
    });
  });

  it("selects a target device session when switching execution devices", async () => {
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-local-only",
          cwd: "/Users/test",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-remote-only",
          cwd: "/home/remote",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });

    render(<TerminalPage />);

    await waitFor(() => {
      expect(mockAttachSession).toHaveBeenCalledWith(
        "terminal-session-local-only",
        "browser-test",
        { kind: "local" },
      );
    });
    mockAttachSession.mockClear();

    await chooseSelectOption("Device", "Phone");

    expect(mockDetachSession).toHaveBeenCalledWith(
      "terminal-session-local-only",
      "browser-test",
      { kind: "local" },
    );
    await waitFor(() =>
      expect(mockDisposeAllViewports).toHaveBeenCalledTimes(1),
    );
    await waitFor(() => {
      expect(mockAttachSession).toHaveBeenCalledWith(
        "terminal-session-remote-only",
        "browser-test",
        { kind: "remote", device_id: "remote-device" },
      );
    });
    expect(mockAttachSession).not.toHaveBeenCalledWith(
      "terminal-session-local-only",
      "browser-test",
      { kind: "remote", device_id: "remote-device" },
    );
  });

  it("attaches to an existing live session and renders replay output", async () => {
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-existing",
          cwd: "/Users/test",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });
    mockAttachSession.mockResolvedValueOnce({
      session: {
        session_id: "terminal-session-existing",
        cwd: "/Users/test",
        profile_id: "bash",
        profile_label: "Bash",
        viewer_count: 2,
        input_lease_holder: { browser_id: "browser-test" },
      },
      replay: [{ type: "output", data: "replayed output\n" }],
    });

    render(<TerminalPage />);

    await waitFor(() => {
      expect(mockAttachSession).toHaveBeenCalledWith(
        "terminal-session-existing",
        "browser-test",
        { kind: "local" },
      );
    });
    expect(await screen.findByLabelText("Terminal output")).toHaveTextContent(
      "replayed output",
    );
  });

  it("clears terminal output when switching to an empty session", async () => {
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-first",
          cwd: "/Users/first",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
        {
          session_id: "terminal-session-second",
          cwd: "/Users/second",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });
    mockAttachSession.mockImplementation((sessionId: string) =>
      Promise.resolve({
        session: {
          session_id: sessionId,
          cwd:
            sessionId === "terminal-session-first"
              ? "/Users/first"
              : "/Users/second",
          profile_id:
            sessionId === "terminal-session-first" ? "first" : "second",
          profile_label:
            sessionId === "terminal-session-first" ? "first" : "second",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
        replay:
          sessionId === "terminal-session-first"
            ? [{ type: "output", data: "first output\n" }]
            : [],
      }),
    );

    render(<TerminalPage />);

    await waitFor(() => {
      expect(screen.getByLabelText("Terminal output")).toHaveTextContent(
        "first output",
      );
    });

    await chooseSelectOption("Session", /MacBook - second/);

    await waitFor(() => {
      expect(screen.getByText("MacBook - second")).toBeVisible();
    });
    expect(screen.getByLabelText("Terminal output")).toHaveTextContent("");
  });

  it("does not render a separate detach action for terminal sessions", async () => {
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-detach",
          cwd: "/Users/test",
          viewer_count: 2,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });

    render(<TerminalPage />);

    await waitFor(() => {
      expect(screen.getAllByText("MacBook - Bash").length).toBeGreaterThan(0);
    });
    expect(
      screen.queryByRole("button", { name: "Detach terminal" }),
    ).not.toBeInTheDocument();
    expect(mockDetachSession).not.toHaveBeenCalled();
    expect(mockKillSession).not.toHaveBeenCalled();
  });

  it("renames the selected terminal session on the terminal service", async () => {
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-rename",
          cwd: "/Users/test",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });

    render(<TerminalPage />);

    await waitFor(() => {
      expect(screen.getAllByText("MacBook - Bash").length).toBeGreaterThan(0);
    });
    fireEvent.click(screen.getByRole("button", { name: "Rename terminal" }));
    const input = await screen.findByLabelText("Terminal name");
    fireEvent.change(input, { target: { value: "Deploy shell" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(mockRenameSession).toHaveBeenCalledWith(
        "terminal-session-rename",
        "Deploy shell",
        { kind: "local" },
      );
      expect(screen.getAllByText("Deploy shell").length).toBeGreaterThan(0);
    });
    expect(
      JSON.parse(
        window.localStorage.getItem("rua-terminal-session-names") ?? "{}",
      ),
    ).toEqual({});
  });

  it("does not submit a terminal rename while the IME is composing", async () => {
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-ime-rename",
          cwd: "/Users/test",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });

    render(<TerminalPage />);

    await waitFor(() => {
      expect(screen.getAllByText("MacBook - Bash").length).toBeGreaterThan(0);
    });
    fireEvent.click(screen.getByRole("button", { name: "Rename terminal" }));
    const input = await screen.findByLabelText("Terminal name");
    fireEvent.change(input, { target: { value: "English title" } });
    fireEvent.keyDown(input, {
      key: "Enter",
      keyCode: 229,
      which: 229,
      isComposing: true,
    });

    expect(mockRenameSession).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog")).toBeInTheDocument();

    fireEvent.keyDown(input, { key: "Enter", keyCode: 13, which: 13 });
    await waitFor(() => {
      expect(mockRenameSession).toHaveBeenCalledWith(
        "terminal-session-ime-rename",
        "English title",
        { kind: "local" },
      );
    });
  });

  it("uses the terminal session display name from the service", async () => {
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-remote-name",
          cwd: "/Users/test",
          profile_id: "bash",
          profile_label: "Bash",
          display_name: "Shared shell",
          viewer_count: 1,
          input_lease_holder: { browser_id: "other-browser" },
        },
      ],
    });

    render(<TerminalPage />);

    await waitFor(() => {
      expect(screen.getAllByText("Shared shell").length).toBeGreaterThan(0);
    });
  });

  it("clears the terminal rename draft from the input field", async () => {
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-clear-rename",
          cwd: "/Users/test",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });

    render(<TerminalPage />);

    await waitFor(() => {
      expect(screen.getAllByText("MacBook - Bash").length).toBeGreaterThan(0);
    });
    fireEvent.click(screen.getByRole("button", { name: "Rename terminal" }));
    const input = await screen.findByLabelText("Terminal name");
    expect(input).toHaveValue("MacBook - Bash");

    fireEvent.click(
      screen.getByRole("button", { name: "Clear terminal name" }),
    );

    expect(input).toHaveValue("");
    expect(input).toHaveFocus();
    expect(
      screen.queryByRole("button", { name: "Clear terminal name" }),
    ).not.toBeInTheDocument();
  });

  it("kills the selected remote terminal through the current execution device", async () => {
    mockListSessions.mockResolvedValueOnce({ sessions: [] });
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-remote",
          cwd: "/home/remote",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });

    render(<TerminalPage />);

    await chooseSelectOption("Device", "Phone");
    await waitFor(() => {
      expect(screen.getAllByText("Phone - Bash").length).toBeGreaterThan(0);
    });
    fireEvent.click(screen.getByRole("button", { name: "Kill terminal" }));
    expect(await screen.findByRole("dialog")).toHaveTextContent(
      "Terminate terminal?",
    );
    expect(mockKillSession).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Terminate" }));

    await waitFor(() => {
      expect(mockKillSession).toHaveBeenCalledWith(
        "terminal-session-remote",
        "browser-test",
        {
          kind: "remote",
          device_id: "remote-device",
        },
      );
    });
  });

  it("can skip the terminate confirmation after opting out", async () => {
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-kill-skip",
          cwd: "/Users/test",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });

    const first = render(<TerminalPage />);

    await waitFor(() => {
      expect(screen.getAllByText("MacBook - Bash").length).toBeGreaterThan(0);
    });
    fireEvent.click(screen.getByRole("button", { name: "Kill terminal" }));
    fireEvent.click(screen.getByLabelText("Do not ask again"));
    fireEvent.click(screen.getByRole("button", { name: "Terminate" }));

    await waitFor(() => {
      expect(mockKillSession).toHaveBeenCalledWith(
        "terminal-session-kill-skip",
        "browser-test",
        { kind: "local" },
      );
    });
    first.unmount();

    mockKillSession.mockClear();
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-kill-skip-next",
          cwd: "/Users/test",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });
    render(<TerminalPage />);
    await waitFor(() => {
      expect(screen.getAllByText("MacBook - Bash").length).toBeGreaterThan(0);
    });
    fireEvent.click(screen.getByRole("button", { name: "Kill terminal" }));

    await waitFor(() => {
      expect(mockKillSession).toHaveBeenCalledWith(
        "terminal-session-kill-skip-next",
        "browser-test",
        { kind: "local" },
      );
    });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("subscribes remote terminal activity for the selected execution device", async () => {
    mockListSessions.mockResolvedValueOnce({ sessions: [] });
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-remote-activity",
          cwd: "/home/remote",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });

    render(<TerminalPage />);

    await chooseSelectOption("Device", "Phone");
    await waitFor(() => expect(eventSources).toHaveLength(1));

    expect(eventSources[0].url).toBe(
      "http://test/api/terminal/sessions/terminal-session-remote-activity/activity?execution_device=remote-device&next_activity_sequence=0&next_output_offset=0",
    );
    act(() => {
      eventSources[0].emit({
        type: "pty_geometry_changed",
        geometry: { cols: 120, rows: 32 },
      });
      eventSources[0].emit({
        type: "input_lease_changed",
        holder: { browser_id: "other-browser" },
      });
    });
    await waitFor(() => {
      expect(mockViewportResize).toHaveBeenCalledWith(
        "remote:remote-device:terminal-session-remote-activity:browser-test",
        { cols: 120, rows: 32 },
      );
      expect(
        screen.getByRole("button", { name: "Simulate xterm input" }),
      ).toBeDisabled();
    });
    expect(mockAttachSession).toHaveBeenCalledTimes(1);
  });

  it("ignores duplicate positioned output without writing it to xterm twice", async () => {
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-duplicate",
          cwd: "/Users/test",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });

    render(<TerminalPage />);
    await waitFor(() => expect(eventSources).toHaveLength(1));
    const frame = positionedOutput("write-once\n", 0, 0);
    act(() => {
      eventSources[0].emit(frame);
      eventSources[0].emit(frame);
    });

    await waitFor(() => {
      expect(screen.getByLabelText("Terminal output").textContent).toBe(
        "write-once\n",
      );
    });
    expect(mockViewportAppend).toHaveBeenCalledTimes(1);
  });

  it("records output positions without recording terminal bytes", async () => {
    window.localStorage.setItem("rua-terminal-diagnostics", "1");
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-diagnostics",
          cwd: "/Users/test",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });

    render(<TerminalPage />);
    await waitFor(() => expect(eventSources).toHaveLength(1));
    act(() => {
      eventSources[0].emit(positionedOutput("secret-terminal-output", 0, 0));
    });
    await waitFor(() => {
      expect(window.__ruaTerminalDiagnostics?.length).toBeGreaterThan(0);
    });

    const encoded = JSON.stringify(window.__ruaTerminalDiagnostics);
    expect(encoded).not.toContain("secret-terminal-output");
    expect(encoded).not.toMatch(/"data"|"text"|"input"|"snapshot"|"command"/);
    expect(encoded).toContain('"start_offset":0');
  });

  it("does not reduce the next frame until xterm confirms the current write", async () => {
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-write-order",
          cwd: "/Users/test",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });
    const firstWrite = deferred<void>();
    nextViewportAppendGate = firstWrite.promise;

    render(<TerminalPage />);
    await waitFor(() => expect(eventSources).toHaveLength(1));
    const first = positionedOutput("first\n", 0, 0);
    const second = positionedOutput("second\n", 1, first.end_offset);
    act(() => {
      eventSources[0].emit(first);
      eventSources[0].emit(second);
    });

    await waitFor(() => expect(mockViewportAppend).toHaveBeenCalledTimes(1));
    expect(mockViewportAppend).toHaveBeenLastCalledWith("first\n");
    await act(async () => firstWrite.resolve());
    await waitFor(() => expect(mockViewportAppend).toHaveBeenCalledTimes(2));
    expect(mockViewportAppend).toHaveBeenLastCalledWith("second\n");
  });

  it("recovers one stream gap with a fresh atomic replacement", async () => {
    const session = {
      session_id: "terminal-session-gap",
      cwd: "/Users/test",
      profile_id: "bash",
      profile_label: "Bash",
      viewer_count: 1,
      input_lease_holder: { browser_id: "browser-test" },
    };
    const initialCursor = {
      next_activity_sequence: 0,
      next_output_offset: 5,
    };
    const missing = positionedOutput("missing\n", 0, 5);
    const delayed = positionedOutput("delayed\n", 1, missing.end_offset);
    mockListSessions.mockResolvedValueOnce({ sessions: [session] });
    mockAttachSession
      .mockResolvedValueOnce({
        session,
        cursor: initialCursor,
        snapshot: {
          data: "base\n",
          cols: 80,
          rows: 24,
          output_offset: 5,
        },
        tail: [],
      })
      .mockResolvedValueOnce({
        session,
        cursor: {
          next_activity_sequence: 2,
          next_output_offset: delayed.end_offset,
        },
        snapshot: {
          data: "base\nmissing\ndelayed\n",
          cols: 80,
          rows: 24,
          output_offset: delayed.end_offset,
        },
        tail: [],
      });

    render(<TerminalPage />);
    await waitFor(() => expect(eventSources).toHaveLength(1));
    act(() => {
      eventSources[0].emit(delayed);
    });

    await waitFor(() => expect(mockAttachSession).toHaveBeenCalledTimes(2));
    expect(mockAttachSession.mock.calls[1]).toEqual([
      "terminal-session-gap",
      "browser-test",
      { kind: "local" },
    ]);
    await waitFor(() => {
      expect(screen.getByLabelText("Terminal output").textContent).toBe(
        "base\nmissing\ndelayed\n",
      );
    });
    expect(mockViewportReset).toHaveBeenCalledTimes(2);
    expect(mockViewportReset).toHaveBeenLastCalledWith(
      "base\nmissing\ndelayed\n",
    );
    await waitFor(() => expect(eventSources).toHaveLength(2));
  });

  it("reattaches once from a fresh snapshot after activity transport failure", async () => {
    const session = {
      session_id: "terminal-session-reconnect",
      cwd: "/Users/test",
      profile_id: "bash",
      profile_label: "Bash",
      viewer_count: 1,
      input_lease_holder: { browser_id: "browser-test" },
    };
    const cursor = {
      next_activity_sequence: 0,
      next_output_offset: 0,
    };
    mockListSessions.mockResolvedValueOnce({ sessions: [session] });
    mockAttachSession
      .mockResolvedValueOnce({
        session,
        cursor,
        snapshot: {
          data: "",
          cols: 80,
          rows: 24,
          output_offset: 0,
        },
        tail: [],
      })
      .mockResolvedValueOnce({
        session,
        cursor: {
          next_activity_sequence: 1,
          next_output_offset: 12,
        },
        snapshot: {
          data: "reconnected\n",
          cols: 80,
          rows: 24,
          output_offset: 12,
        },
        tail: [],
      });

    render(<TerminalPage />);
    await waitFor(() => expect(eventSources).toHaveLength(1));
    act(() => eventSources[0].fail());

    await waitFor(() => expect(mockAttachSession).toHaveBeenCalledTimes(2));
    await waitFor(() => {
      expect(screen.getByLabelText("Terminal output")).toHaveTextContent(
        "reconnected",
      );
    });
    expect(mockAttachSession.mock.calls[1]).toEqual([
      "terminal-session-reconnect",
      "browser-test",
      { kind: "local" },
    ]);
    await waitFor(() => expect(eventSources).toHaveLength(2));
  });

  it("removes a stale runtime when recovery reports the authority session is gone", async () => {
    const session = {
      session_id: "terminal-session-gone",
      cwd: "/Users/test",
      profile_id: "bash",
      profile_label: "Bash",
      viewer_count: 1,
      input_lease_holder: { browser_id: "browser-test" },
    };
    mockListSessions.mockResolvedValueOnce({ sessions: [session] });
    mockAttachSession
      .mockResolvedValueOnce({
        session,
        snapshot: "",
        tail: [],
      })
      .mockRejectedValueOnce(
        new Error("terminal session not found: terminal-session-gone"),
      );

    render(<TerminalPage />);
    await waitFor(() => expect(eventSources).toHaveLength(1));
    act(() => eventSources[0].fail());

    await waitFor(() => {
      expect(screen.getAllByText("No terminal session").length).toBeGreaterThan(
        0,
      );
    });
    expect(mockViewportDispose).toHaveBeenCalledWith(
      "local:terminal-session-gone:browser-test",
    );
    expect(screen.getByRole("status")).toHaveTextContent("Terminal exited");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("applies the attach tail before subscribing to remote live activity", async () => {
    mockListSessions.mockResolvedValueOnce({ sessions: [] });
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-remote-buffer",
          cwd: "/home/remote",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });
    const attach = deferred<Awaited<ReturnType<typeof mockAttachSession>>>();
    mockAttachSession.mockReturnValueOnce(attach.promise);

    render(<TerminalPage />);

    await chooseSelectOption("Device", "Phone");
    await waitFor(() => expect(mockAttachSession).toHaveBeenCalledTimes(1));
    expect(eventSources).toHaveLength(0);
    expect(screen.getByLabelText("Terminal output")).not.toHaveTextContent(
      "remote history",
    );
    expect(screen.getByLabelText("Terminal output")).not.toHaveTextContent(
      "remote live",
    );

    const snapshotData = "remote history\n";
    const snapshotOffset = new TextEncoder().encode(snapshotData).byteLength;
    const liveData = "remote live\n";
    await act(async () => {
      attach.resolve({
        session: {
          session_id: "terminal-session-remote-buffer",
          cwd: "/home/remote",
          profile_id: "bash",
          profile_label: "Bash",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
        cursor: {
          next_activity_sequence: 1,
          next_output_offset: snapshotOffset,
        },
        snapshot: {
          data: snapshotData,
          cols: 80,
          rows: 24,
          output_offset: snapshotOffset,
        },
        tail: [positionedOutput(liveData, 1, snapshotOffset)],
      });
      await attach.promise;
    });

    await waitFor(() => {
      expect(screen.getByLabelText("Terminal output")).toHaveTextContent(
        "remote history",
      );
      expect(screen.getByLabelText("Terminal output")).toHaveTextContent(
        "remote live",
      );
    });
    expect(screen.getByLabelText("Terminal output").textContent).toBe(
      "remote history\nremote live\n",
    );
    await waitFor(() => expect(eventSources).toHaveLength(1));
    expect(eventSources[0].url).toContain(
      "next_activity_sequence=2&next_output_offset=27",
    );
  });

  it("consumes canonical remote snapshot and tail without a replay endpoint", async () => {
    mockListSessions.mockResolvedValueOnce({ sessions: [] });
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-remote-replay",
          cwd: "/home/remote",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });
    const snapshotData = "remote replay part 1\n";
    const snapshotOffset = new TextEncoder().encode(snapshotData).byteLength;
    mockAttachSession.mockImplementationOnce((sessionId: string) =>
      Promise.resolve({
        session: {
          session_id: sessionId,
          cwd: "/home/remote",
          profile_id: "bash",
          profile_label: "Bash",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
        cursor: {
          next_activity_sequence: 1,
          next_output_offset: snapshotOffset,
        },
        snapshot: {
          data: snapshotData,
          cols: 80,
          rows: 24,
          output_offset: snapshotOffset,
        },
        tail: [positionedOutput("remote replay part 2\n", 1, snapshotOffset)],
      }),
    );

    render(<TerminalPage />);

    await chooseSelectOption("Device", "Phone");

    await waitFor(() => {
      expect(screen.getByLabelText("Terminal output")).toHaveTextContent(
        "remote replay part 1",
      );
      expect(screen.getByLabelText("Terminal output")).toHaveTextContent(
        "remote replay part 2",
      );
    });
    expect(mockViewportReset).toHaveBeenCalledWith(
      "remote replay part 1\nremote replay part 2\n",
    );
    expect(mockViewportAppend).not.toHaveBeenCalledWith(
      "remote replay part 2\n",
    );
  });

  it("uses terminal snapshots from remote attach without byte replay", async () => {
    mockListSessions.mockResolvedValueOnce({ sessions: [] });
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-remote-snapshot",
          cwd: "/home/remote",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });
    mockAttachSession.mockImplementation((sessionId: string) =>
      Promise.resolve({
        session: {
          session_id: sessionId,
          cwd: "/home/remote",
          profile_id: "bash",
          profile_label: "Bash",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
        replay: [{ type: "output", data: "stale byte replay\n" }],
        snapshot: "serialized terminal snapshot\n",
      }),
    );

    render(<TerminalPage />);

    await chooseSelectOption("Device", "Phone");

    await waitFor(() => {
      expect(screen.getByLabelText("Terminal output")).toHaveTextContent(
        "serialized terminal snapshot",
      );
    });
    expect(screen.getByLabelText("Terminal output")).not.toHaveTextContent(
      "stale byte replay",
    );
    expect(mockViewportReset).toHaveBeenCalledWith(
      "serialized terminal snapshot\n",
    );
    expect(mockViewportAppend).not.toHaveBeenCalledWith(
      "serialized terminal snapshot\n",
    );
  });

  it("applies positioned live tail after a remote snapshot", async () => {
    mockListSessions.mockResolvedValueOnce({ sessions: [] });
    mockListSessions.mockResolvedValueOnce({
      sessions: [
        {
          session_id: "terminal-session-remote-replay-snapshot",
          cwd: "/home/remote",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
      ],
    });
    const snapshotData = "serialized replay snapshot\n";
    const snapshotOffset = 18;
    mockAttachSession.mockImplementationOnce((sessionId: string) =>
      Promise.resolve({
        session: {
          session_id: sessionId,
          cwd: "/home/remote",
          profile_id: "bash",
          profile_label: "Bash",
          viewer_count: 1,
          input_lease_holder: { browser_id: "browser-test" },
        },
        cursor: {
          next_activity_sequence: 3,
          next_output_offset: snapshotOffset,
        },
        snapshot: {
          data: snapshotData,
          cols: 80,
          rows: 24,
          output_offset: snapshotOffset,
        },
        tail: [positionedOutput("positioned live tail\n", 3, snapshotOffset)],
      }),
    );

    render(<TerminalPage />);

    await chooseSelectOption("Device", "Phone");

    await waitFor(() => {
      expect(screen.getByLabelText("Terminal output")).toHaveTextContent(
        "serialized replay snapshot",
      );
      expect(screen.getByLabelText("Terminal output")).toHaveTextContent(
        "positioned live tail",
      );
    });
    expect(mockViewportReset).toHaveBeenCalledWith(
      "serialized replay snapshot\npositioned live tail\n",
    );
    expect(mockViewportAppend).not.toHaveBeenCalledWith(
      "positioned live tail\n",
    );
  });

  it("shows remote terminal list errors explicitly", async () => {
    mockListSessions.mockResolvedValueOnce({ sessions: [] });
    mockListSessions.mockRejectedValueOnce(
      new Error("remote device unavailable"),
    );

    render(<TerminalPage />);

    await chooseSelectOption("Device", "Phone");

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "remote device unavailable",
    );
  });
});
