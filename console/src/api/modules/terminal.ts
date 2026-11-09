import { request } from "../request";
import type {
  TerminalStreamCursor,
  TerminalStreamFrame,
} from "../../terminal/stream";

export type TerminalExecutionDevice =
  | { kind: "local" }
  | { kind: "remote"; device_id: string };

export interface TerminalViewerIdentity {
  device_id?: string | null;
  browser_id: string;
}

export interface TerminalPtyGeometry {
  cols: number;
  rows: number;
}

export interface TerminalPalette {
  foreground: string;
  background: string;
  cursor: string;
}

export interface TerminalSession {
  session_id: string;
  cwd: string;
  profile_id: string;
  profile_label: string;
  display_name?: string | null;
  viewer_count: number;
  input_lease_holder?: TerminalViewerIdentity | null;
  pty_geometry: TerminalPtyGeometry;
}

export interface TerminalProfile {
  id: string;
  label: string;
  kind: string;
}

export interface TerminalProfileListResponse {
  os: string;
  default_profile_id: string;
  profiles: TerminalProfile[];
}

export interface TerminalSnapshot {
  data: string;
  cols: number;
  rows: number;
  cursor: TerminalStreamCursor;
}

export interface OpenTerminalSessionRequest {
  execution_device: TerminalExecutionDevice;
  browser_viewer_id: string;
  scrollback_lines: number;
  cwd?: string;
  terminal_profile_id?: string;
}

export interface TerminalSessionListResponse {
  sessions: TerminalSession[];
}

export interface TerminalSessionResponse {
  session: TerminalSession;
}

export interface TerminalAttachResponse {
  session: TerminalSession;
  snapshot: TerminalSnapshot;
  tail: TerminalStreamFrame[];
  live_cursor: TerminalStreamCursor;
}

function executionDeviceQuery(device: TerminalExecutionDevice): string {
  return device.kind === "local" ? "local" : device.device_id;
}

function withOptionalExecutionDevice(
  browserViewerId: string,
  executionDevice?: TerminalExecutionDevice,
) {
  return executionDevice
    ? { browser_viewer_id: browserViewerId, execution_device: executionDevice }
    : { browser_viewer_id: browserViewerId };
}

export const terminalApi = {
  listProfiles: (executionDevice: TerminalExecutionDevice) =>
    request<TerminalProfileListResponse>(
      `/terminal/profiles?execution_device=${encodeURIComponent(
        executionDeviceQuery(executionDevice),
      )}`,
    ),

  listSessions: (executionDevice: TerminalExecutionDevice) =>
    request<TerminalSessionListResponse>(
      `/terminal/sessions?execution_device=${encodeURIComponent(
        executionDeviceQuery(executionDevice),
      )}`,
    ),

  openSession: (body: OpenTerminalSessionRequest) =>
    request<TerminalSessionResponse>("/terminal/sessions", {
      method: "POST",
      body: JSON.stringify(body),
    }),

  attachSession: (
    sessionId: string,
    browserViewerId: string,
    scrollbackLines: number,
    executionDevice?: TerminalExecutionDevice,
    cursor?: TerminalStreamCursor,
  ) =>
    request<TerminalAttachResponse>(
      `/terminal/sessions/${encodeURIComponent(sessionId)}/attach`,
      {
        method: "POST",
        body: JSON.stringify({
          ...withOptionalExecutionDevice(browserViewerId, executionDevice),
          scrollback_lines: scrollbackLines,
          ...(cursor ? { cursor } : {}),
        }),
      },
    ),

  updateScrollback: (
    sessionId: string,
    scrollbackLines: number,
    executionDevice?: TerminalExecutionDevice,
  ) =>
    request<void>(
      `/terminal/sessions/${encodeURIComponent(sessionId)}/scrollback`,
      {
        method: "POST",
        body: JSON.stringify({
          scrollback_lines: scrollbackLines,
          ...(executionDevice ? { execution_device: executionDevice } : {}),
        }),
      },
    ),

  detachSession: (
    sessionId: string,
    browserViewerId: string,
    executionDevice?: TerminalExecutionDevice,
  ) =>
    request<void>(
      `/terminal/sessions/${encodeURIComponent(sessionId)}/detach`,
      {
        method: "POST",
        body: JSON.stringify(
          withOptionalExecutionDevice(browserViewerId, executionDevice),
        ),
      },
    ),

  takeInputLease: (
    sessionId: string,
    browserViewerId: string,
    executionDevice?: TerminalExecutionDevice,
  ) =>
    request<TerminalSessionResponse>(
      `/terminal/sessions/${encodeURIComponent(sessionId)}/take-input-lease`,
      {
        method: "POST",
        body: JSON.stringify(
          withOptionalExecutionDevice(browserViewerId, executionDevice),
        ),
      },
    ),

  adaptPtyGeometry: (
    sessionId: string,
    browserViewerId: string,
    cols: number,
    rows: number,
    executionDevice?: TerminalExecutionDevice,
  ) =>
    request<TerminalSessionResponse>(
      `/terminal/sessions/${encodeURIComponent(sessionId)}/adapt-pty-geometry`,
      {
        method: "POST",
        body: JSON.stringify({
          browser_viewer_id: browserViewerId,
          cols,
          rows,
          ...(executionDevice ? { execution_device: executionDevice } : {}),
        }),
      },
    ),

  updatePalette: (
    sessionId: string,
    browserViewerId: string,
    palette: TerminalPalette,
    executionDevice?: TerminalExecutionDevice,
  ) =>
    request<void>(
      `/terminal/sessions/${encodeURIComponent(sessionId)}/palette`,
      {
        method: "POST",
        body: JSON.stringify({
          browser_viewer_id: browserViewerId,
          palette,
          ...(executionDevice ? { execution_device: executionDevice } : {}),
        }),
      },
    ),

  renameSession: (
    sessionId: string,
    displayName: string | null,
    executionDevice?: TerminalExecutionDevice,
  ) =>
    request<TerminalSessionResponse>(
      `/terminal/sessions/${encodeURIComponent(sessionId)}/rename`,
      {
        method: "POST",
        body: JSON.stringify({
          display_name: displayName,
          ...(executionDevice ? { execution_device: executionDevice } : {}),
        }),
      },
    ),

  sendInput: (
    sessionId: string,
    browserViewerId: string,
    data: string,
    executionDevice?: TerminalExecutionDevice,
  ) =>
    request<void>(`/terminal/sessions/${encodeURIComponent(sessionId)}/input`, {
      method: "POST",
      body: JSON.stringify(
        executionDevice
          ? {
              browser_viewer_id: browserViewerId,
              data,
              execution_device: executionDevice,
            }
          : { browser_viewer_id: browserViewerId, data },
      ),
    }),

  killSession: (
    sessionId: string,
    browserViewerId: string,
    executionDevice?: TerminalExecutionDevice,
  ) => {
    const query = new URLSearchParams({
      browser_viewer_id: browserViewerId,
      ...(executionDevice
        ? { execution_device: executionDeviceQuery(executionDevice) }
        : {}),
    });
    return request<void>(
      `/terminal/sessions/${encodeURIComponent(sessionId)}?${query.toString()}`,
      {
        method: "DELETE",
      },
    );
  },
};
