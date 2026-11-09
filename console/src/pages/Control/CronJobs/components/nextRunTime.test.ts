import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { computeNextRun, formatNextRun } from "./nextRunTime";

const t = (key: string, opts?: Record<string, unknown>) => {
  if (opts?.count !== undefined) return `${key}:${opts.count}`;
  return key;
};

describe("computeNextRun", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 5, 18, 10, 20, 0));
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("computes hourly and daily next-run previews from local time", () => {
    expect(computeNextRun("0 * * * *")?.format("YYYY-MM-DD HH:mm")).toBe(
      "2026-06-18 11:00",
    );
    expect(computeNextRun("45 10 * * *")?.format("YYYY-MM-DD HH:mm")).toBe(
      "2026-06-18 10:45",
    );
    expect(computeNextRun("5 9 * * *")?.format("YYYY-MM-DD HH:mm")).toBe(
      "2026-06-19 09:05",
    );
  });

  it("wraps weekly previews to the next selected weekday", () => {
    expect(computeNextRun("0 9 * * thu")?.format("YYYY-MM-DD HH:mm")).toBe(
      "2026-06-25 09:00",
    );
    expect(computeNextRun("0 12 * * thu,fri")?.format("YYYY-MM-DD HH:mm")).toBe(
      "2026-06-18 12:00",
    );
  });

  it("returns null for custom or invalid cron previews", () => {
    expect(computeNextRun("*/15 * * * *")).toBeNull();
    expect(computeNextRun("")).toBeNull();
  });
});

describe("formatNextRun", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 5, 18, 10, 20, 30));
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("formats paused, imminent, minute, hour, and custom states", () => {
    expect(formatNextRun("0 * * * *", false, t)).toBe("cronJobs.nextRunPaused");
    expect(formatNextRun("21 10 * * *", true, t)).toBe(
      "cronJobs.nextRunImminent",
    );
    expect(formatNextRun("45 10 * * *", true, t)).toBe(
      "cronJobs.nextRunInMinutes:24",
    );
    expect(formatNextRun("0 12 * * *", true, t)).toBe(
      "cronJobs.nextRunInHours:1",
    );
    expect(formatNextRun("*/15 * * * *", true, t)).toBe("—");
  });
});
