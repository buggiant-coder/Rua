import { describe, expect, it } from "vitest";
import {
  decodeTerminalOutput,
  reduceTerminalStreamFrame,
  type TerminalStreamCursor,
  type TerminalStreamFrame,
} from "./stream";

function base64(bytes: Uint8Array) {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

function outputFrame(
  activitySequence: number,
  startOffset: number,
  bytes: Uint8Array,
): TerminalStreamFrame {
  return {
    type: "output",
    activity_sequence: activitySequence,
    start_offset: startOffset,
    end_offset: startOffset + bytes.byteLength,
    data_base64: base64(bytes),
  };
}

const initialCursor: TerminalStreamCursor = {
  next_activity_sequence: 4,
  next_output_offset: 10,
};

describe("terminal stream reducer", () => {
  it("decodes original PTY bytes without using JavaScript string length", () => {
    const bytes = new TextEncoder().encode("中");
    const frame = outputFrame(4, 10, bytes);

    expect(frame.end_offset - frame.start_offset).toBe(3);
    expect(Array.from(decodeTerminalOutput(frame))).toEqual(Array.from(bytes));
  });

  it("applies one contiguous output frame and advances both cursor positions", () => {
    const frame = outputFrame(4, 10, new Uint8Array([0x1b, 0x5b, 0x4a]));

    expect(reduceTerminalStreamFrame(initialCursor, frame)).toEqual({
      kind: "apply",
      cursor: {
        next_activity_sequence: 5,
        next_output_offset: 13,
      },
      data: new Uint8Array([0x1b, 0x5b, 0x4a]),
      frame,
    });
  });

  it("ignores a complete duplicate without moving the cursor", () => {
    const frame = outputFrame(3, 7, new Uint8Array([1, 2, 3]));

    expect(reduceTerminalStreamFrame(initialCursor, frame)).toEqual({
      kind: "duplicate",
      cursor: initialCursor,
      frame,
    });
  });

  it("reports a sequence or output gap instead of guessing terminal text overlap", () => {
    const sequenceGap = outputFrame(6, 10, new Uint8Array([1]));
    const outputGap = outputFrame(4, 12, new Uint8Array([1]));

    expect(reduceTerminalStreamFrame(initialCursor, sequenceGap)).toEqual({
      kind: "gap",
      cursor: initialCursor,
      frame: sequenceGap,
    });
    expect(reduceTerminalStreamFrame(initialCursor, outputGap)).toEqual({
      kind: "gap",
      cursor: initialCursor,
      frame: outputGap,
    });
  });

  it("rejects partial overlap and byte-range length mismatches", () => {
    const overlap = outputFrame(4, 9, new Uint8Array([1, 2]));
    const wrongLength = {
      ...outputFrame(4, 10, new Uint8Array([1, 2])),
      end_offset: 13,
    };

    expect(reduceTerminalStreamFrame(initialCursor, overlap).kind).toBe(
      "invalid",
    );
    expect(reduceTerminalStreamFrame(initialCursor, wrongLength).kind).toBe(
      "invalid",
    );
  });

  it("orders control activity without advancing the output offset", () => {
    const frame: TerminalStreamFrame = {
      type: "pty_geometry_changed",
      activity_sequence: 4,
      geometry: { cols: 100, rows: 30 },
    };

    expect(reduceTerminalStreamFrame(initialCursor, frame)).toEqual({
      kind: "apply",
      cursor: {
        next_activity_sequence: 5,
        next_output_offset: 10,
      },
      frame,
    });
  });
});
