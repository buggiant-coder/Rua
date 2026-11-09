export interface TerminalStreamCursor {
  next_activity_sequence: number;
  next_output_offset: number;
}

interface TerminalStreamFrameBase {
  activity_sequence: number;
}

export interface TerminalOutputFrame extends TerminalStreamFrameBase {
  type: "output";
  start_offset: number;
  end_offset: number;
  data_base64: string;
}

export type TerminalStreamFrame =
  | TerminalOutputFrame
  | (TerminalStreamFrameBase & {
      type: "input_lease_changed";
      holder?: { device_id?: string | null; browser_id: string } | null;
    })
  | (TerminalStreamFrameBase & {
      type: "pty_geometry_changed";
      geometry: { cols: number; rows: number };
    })
  | (TerminalStreamFrameBase & { type: "exited" })
  | (TerminalStreamFrameBase & { type: "error"; detail: string });

export type TerminalStreamReduction =
  | {
      kind: "apply";
      cursor: TerminalStreamCursor;
      frame: TerminalStreamFrame;
      data?: Uint8Array;
    }
  | {
      kind: "duplicate" | "gap" | "invalid";
      cursor: TerminalStreamCursor;
      frame: TerminalStreamFrame;
    };

export function decodeTerminalOutput(frame: TerminalOutputFrame): Uint8Array {
  const binary = atob(frame.data_base64);
  return Uint8Array.from(binary, (character) => character.charCodeAt(0));
}

export function reduceTerminalStreamFrame(
  cursor: TerminalStreamCursor,
  frame: TerminalStreamFrame,
): TerminalStreamReduction {
  if (frame.type !== "output") {
    if (frame.activity_sequence < cursor.next_activity_sequence) {
      return { kind: "duplicate", cursor, frame };
    }
    if (frame.activity_sequence > cursor.next_activity_sequence) {
      return { kind: "gap", cursor, frame };
    }
    return {
      kind: "apply",
      cursor: {
        next_activity_sequence: cursor.next_activity_sequence + 1,
        next_output_offset: cursor.next_output_offset,
      },
      frame,
    };
  }

  let data: Uint8Array;
  try {
    data = decodeTerminalOutput(frame);
  } catch {
    return { kind: "invalid", cursor, frame };
  }
  if (
    frame.start_offset < 0 ||
    frame.end_offset < frame.start_offset ||
    frame.end_offset - frame.start_offset !== data.byteLength
  ) {
    return { kind: "invalid", cursor, frame };
  }
  if (
    frame.start_offset < cursor.next_output_offset &&
    frame.end_offset > cursor.next_output_offset
  ) {
    return { kind: "invalid", cursor, frame };
  }
  if (frame.end_offset <= cursor.next_output_offset) {
    return frame.activity_sequence < cursor.next_activity_sequence
      ? { kind: "duplicate", cursor, frame }
      : { kind: "invalid", cursor, frame };
  }
  if (
    frame.activity_sequence > cursor.next_activity_sequence ||
    frame.start_offset > cursor.next_output_offset
  ) {
    return { kind: "gap", cursor, frame };
  }
  if (
    frame.activity_sequence < cursor.next_activity_sequence ||
    frame.start_offset < cursor.next_output_offset
  ) {
    return { kind: "invalid", cursor, frame };
  }
  return {
    kind: "apply",
    cursor: {
      next_activity_sequence: cursor.next_activity_sequence + 1,
      next_output_offset: frame.end_offset,
    },
    data,
    frame,
  };
}
