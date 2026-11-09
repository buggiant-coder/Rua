import { afterEach, describe, expect, it } from "vitest";
import { recordTerminalDiagnostic } from "./diagnostics";

describe("terminal diagnostics", () => {
  afterEach(() => {
    window.localStorage.removeItem("rua-terminal-diagnostics");
    delete window.__ruaTerminalDiagnostics;
  });

  it("records bounded metadata without terminal content fields", () => {
    window.localStorage.setItem("rua-terminal-diagnostics", "1");
    recordTerminalDiagnostic({
      event: "frame_classified",
      session_key: "local:session-a:viewer-a",
      session_id: "session-a",
      viewer_id: "viewer-a",
      placement: "local",
      cursor_before: {
        next_activity_sequence: 4,
        next_output_offset: 10,
      },
      cursor_after: {
        next_activity_sequence: 5,
        next_output_offset: 13,
        ...({ data: "secret-terminal-cursor" } as object),
      },
      activity_sequence: 4,
      start_offset: 10,
      end_offset: 13,
      frame_classification: "apply",
      queue_depth: 1,
      viewport: {
        cols: 100,
        rows: 30,
        ...({ text: "secret-terminal-viewport" } as object),
      },
      ...({
        data: "secret-terminal-output",
        input: "secret-terminal-input",
        snapshot: "secret-terminal-snapshot",
      } as object),
    });

    const encoded = JSON.stringify(window.__ruaTerminalDiagnostics);
    expect(encoded).not.toContain("secret-terminal-");
    expect(encoded).not.toMatch(/"data"|"text"|"input"|"snapshot"|"command"/);
    expect(window.__ruaTerminalDiagnostics).toHaveLength(1);
  });

  it("stays disabled unless explicitly requested", () => {
    recordTerminalDiagnostic({
      event: "runtime_dispose",
      session_key: "local:session-a:viewer-a",
    });

    expect(window.__ruaTerminalDiagnostics).toBeUndefined();
  });
});
