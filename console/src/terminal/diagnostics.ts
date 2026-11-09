import type { TerminalStreamCursor } from "./stream";

const TERMINAL_DIAGNOSTICS_KEY = "rua-terminal-diagnostics";
const TERMINAL_DIAGNOSTICS_LIMIT = 500;

export type TerminalDiagnosticEventName =
  | "attach_start"
  | "attach_complete"
  | "frame_classified"
  | "runtime_state"
  | "surface_change"
  | "visibility_change"
  | "recovery_start"
  | "recovery_complete"
  | "subscription_open"
  | "subscription_close"
  | "writer_enqueue"
  | "writer_complete"
  | "viewport_fit"
  | "focus_change"
  | "ime_change"
  | "geometry_adapt"
  | "renderer_change"
  | "runtime_dispose";

export type TerminalDiagnosticRuntimeState =
  | "new"
  | "attaching"
  | "hydrating"
  | "live"
  | "suspended"
  | "resyncing"
  | "disposed";

export interface TerminalDiagnosticEvent {
  at: string;
  event: TerminalDiagnosticEventName;
  session_key: string;
  session_id?: string;
  viewer_id?: string;
  placement?: "local" | "remote";
  cursor_before?: TerminalStreamCursor;
  cursor_after?: TerminalStreamCursor;
  snapshot_cursor?: TerminalStreamCursor;
  live_cursor?: TerminalStreamCursor;
  activity_sequence?: number;
  start_offset?: number;
  end_offset?: number;
  frame_classification?: "apply" | "duplicate" | "gap" | "invalid";
  recovery_reason?: string;
  queue_depth?: number;
  queued_bytes?: number;
  in_flight_bytes?: number;
  acknowledged_bytes?: number;
  batch_bytes?: number;
  high_watermark_bytes?: number;
  write_callback_completed?: boolean;
  viewport?: {
    cols: number;
    rows: number;
    base_y?: number;
    viewport_y?: number;
    bottom_offset?: number;
    following_output?: boolean;
  };
  focused?: boolean;
  composing?: boolean;
  renderer?: "webgl" | "dom";
  state_before?: TerminalDiagnosticRuntimeState;
  state_after?: TerminalDiagnosticRuntimeState;
  surface_action?: "create" | "reveal" | "swap" | "dispose";
  visibility_scope?: "route" | "session" | "document";
  visible?: boolean;
  lifecycle?: "created" | "hydrated" | "bound" | "hidden" | "disposed";
}

type TerminalDiagnosticInput = Omit<TerminalDiagnosticEvent, "at">;

declare global {
  interface Window {
    __ruaTerminalDiagnostics?: TerminalDiagnosticEvent[];
  }
}

export function recordTerminalDiagnostic(event: TerminalDiagnosticInput) {
  try {
    if (window.localStorage.getItem(TERMINAL_DIAGNOSTICS_KEY) !== "1") return;
  } catch {
    return;
  }
  const log = window.__ruaTerminalDiagnostics ?? [];
  const safe: TerminalDiagnosticEvent = {
    at: new Date().toISOString(),
    event: event.event,
    session_key: event.session_key,
  };
  const assign = <K extends keyof TerminalDiagnosticInput>(key: K) => {
    const value = event[key];
    if (value !== undefined) Object.assign(safe, { [key]: value });
  };
  const copyCursor = (
    cursor: TerminalStreamCursor | undefined,
  ): TerminalStreamCursor | undefined =>
    cursor
      ? {
          next_activity_sequence: cursor.next_activity_sequence,
          next_output_offset: cursor.next_output_offset,
        }
      : undefined;
  safe.cursor_before = copyCursor(event.cursor_before);
  safe.cursor_after = copyCursor(event.cursor_after);
  safe.snapshot_cursor = copyCursor(event.snapshot_cursor);
  safe.live_cursor = copyCursor(event.live_cursor);
  if (event.viewport) {
    safe.viewport = {
      cols: event.viewport.cols,
      rows: event.viewport.rows,
      ...(event.viewport.base_y === undefined
        ? {}
        : { base_y: event.viewport.base_y }),
      ...(event.viewport.viewport_y === undefined
        ? {}
        : { viewport_y: event.viewport.viewport_y }),
      ...(event.viewport.bottom_offset === undefined
        ? {}
        : { bottom_offset: event.viewport.bottom_offset }),
      ...(event.viewport.following_output === undefined
        ? {}
        : { following_output: event.viewport.following_output }),
    };
  }
  for (const key of [
    "session_id",
    "viewer_id",
    "placement",
    "activity_sequence",
    "start_offset",
    "end_offset",
    "frame_classification",
    "recovery_reason",
    "queue_depth",
    "queued_bytes",
    "in_flight_bytes",
    "acknowledged_bytes",
    "batch_bytes",
    "high_watermark_bytes",
    "write_callback_completed",
    "focused",
    "composing",
    "renderer",
    "state_before",
    "state_after",
    "surface_action",
    "visibility_scope",
    "visible",
    "lifecycle",
  ] as const) {
    assign(key);
  }
  log.push(safe);
  if (log.length > TERMINAL_DIAGNOSTICS_LIMIT) {
    log.splice(0, log.length - TERMINAL_DIAGNOSTICS_LIMIT);
  }
  window.__ruaTerminalDiagnostics = log;
}
