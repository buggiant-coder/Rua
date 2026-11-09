import { describe, expect, it } from "vitest";
import { parseEvery, serializeEvery } from "./parseEvery";

describe("parseEvery", () => {
  it("keeps empty or invalid Rust heartbeat intervals on the UI default", () => {
    expect(parseEvery("")).toEqual({ number: 6, unit: "h" });
    expect(parseEvery("not-an-interval")).toEqual({ number: 6, unit: "h" });
    expect(parseEvery("0m")).toEqual({ number: 6, unit: "h" });
  });

  it("preserves whole-hour intervals as hours", () => {
    expect(parseEvery("1h")).toEqual({ number: 1, unit: "h" });
    expect(parseEvery("2h0m")).toEqual({ number: 2, unit: "h" });
    expect(parseEvery("120m")).toEqual({ number: 2, unit: "h" });
  });

  it("uses minutes when the backend interval is not a whole hour", () => {
    expect(parseEvery("45m")).toEqual({ number: 45, unit: "m" });
    expect(parseEvery("1h30m")).toEqual({ number: 90, unit: "m" });
    expect(parseEvery("1h30s")).toEqual({ number: 61, unit: "m" });
  });
});

describe("serializeEvery", () => {
  it("serializes UI heartbeat form values back to Rust duration strings", () => {
    expect(serializeEvery({ number: 3, unit: "h" })).toBe("3h");
    expect(serializeEvery({ number: 30, unit: "m" })).toBe("30m");
  });
});
