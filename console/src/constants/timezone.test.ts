import { describe, expect, it } from "vitest";

import { TIMEZONE_OPTIONS } from "./timezone";

describe("TIMEZONE_OPTIONS", () => {
  it("keeps a static no-dependency timezone option list for Rust settings", () => {
    expect(TIMEZONE_OPTIONS.length).toBeGreaterThan(0);

    for (const option of TIMEZONE_OPTIONS) {
      expect(option.value).toBeTruthy();
      expect(option.label).toContain(option.value);
    }
  });

  it("includes the primary regions used by the console settings UI", () => {
    const values = TIMEZONE_OPTIONS.map((option) => option.value);

    expect(values).toEqual(
      expect.arrayContaining([
        "America/Los_Angeles",
        "America/New_York",
        "UTC",
        "Europe/London",
        "Asia/Shanghai",
        "Asia/Tokyo",
        "Australia/Sydney",
      ]),
    );
  });

  it("has no duplicate timezone values", () => {
    const values = TIMEZONE_OPTIONS.map((option) => option.value);

    expect(new Set(values).size).toBe(values.length);
  });
});
