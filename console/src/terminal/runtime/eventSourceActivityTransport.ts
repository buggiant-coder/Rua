import { consoleFetch } from "../../api/consoleTransport";
import { buildAuthHeaders } from "../../api/authHeaders";
import { readSseChunks } from "../../pages/Chat/owned/sse";
import { recordTerminalDiagnostic } from "../diagnostics";
import type {
  TerminalActivityConnection,
  TerminalActivityTransport,
} from "./viewerRuntime";
import type { TerminalStreamCursor, TerminalStreamFrame } from "../stream";

interface EventSourceActivityTransportOptions {
  browserViewerId: string;
  executionDeviceQueryValue: string;
  placement: "local" | "remote";
  retainAuthority: boolean;
  sessionId: string;
  sessionKey: string;
}

export class EventSourceActivityTransport implements TerminalActivityTransport {
  private readonly options: EventSourceActivityTransportOptions;

  constructor(options: EventSourceActivityTransportOptions) {
    this.options = options;
  }

  open(
    cursor: TerminalStreamCursor,
    handlers: {
      onFrame: (frame: TerminalStreamFrame) => void;
      onError: (error: unknown) => void;
    },
  ): TerminalActivityConnection {
    const query = new URLSearchParams({
      execution_device: this.options.executionDeviceQueryValue,
      browser_viewer_id: this.options.browserViewerId,
      next_activity_sequence: String(cursor.next_activity_sequence),
      next_output_offset: String(cursor.next_output_offset),
      retain_authority: String(this.options.retainAuthority),
    });
    const url = `/terminal/sessions/${encodeURIComponent(
      this.options.sessionId,
    )}/activity?${query.toString()}`;
    const controller = new AbortController();
    let closed = false;
    recordTerminalDiagnostic({
      event: "subscription_open",
      session_key: this.options.sessionKey,
      session_id: this.options.sessionId,
      viewer_id: this.options.browserViewerId,
      placement: this.options.placement,
      cursor_before: cursor,
    });
    // Native EventSource cannot carry the Console's bearer authentication.
    // The Viewer Runtime still owns reconnection and positioned recovery.
    void (async () => {
      const response = await consoleFetch(url, {
        headers: { ...buildAuthHeaders(), Accept: "text/event-stream" },
        signal: controller.signal,
        cache: "no-store",
      });
      if (!response.ok) {
        throw new Error(
          `Terminal activity failed: ${
            response.status
          } ${await response.text()}`,
        );
      }
      for await (const chunk of readSseChunks(response.body)) {
        if (closed) break;
        if (chunk.event === "terminal_activity") {
          handlers.onFrame(JSON.parse(chunk.data) as TerminalStreamFrame);
        }
      }
      if (!closed) throw new Error("Terminal activity stream closed");
    })().catch((error: unknown) => {
      if (!closed) handlers.onError(error);
    });

    return {
      close: () => {
        if (closed) return;
        closed = true;
        controller.abort();
        recordTerminalDiagnostic({
          event: "subscription_close",
          session_key: this.options.sessionKey,
          session_id: this.options.sessionId,
          viewer_id: this.options.browserViewerId,
          placement: this.options.placement,
        });
      },
    };
  }
}
