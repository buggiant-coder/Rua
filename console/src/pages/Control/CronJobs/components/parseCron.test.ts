import { describe, expect, it } from "vitest";
import { parseCron, serializeCron } from "./parseCron";

describe("parseCron", () => {
  it("maps empty and structured cron expressions to form presets", () => {
    expect(parseCron("")).toEqual({ type: "daily", hour: 9, minute: 0 });
    expect(parseCron("0 * * * *")).toEqual({ type: "hourly", minute: 0 });
    expect(parseCron("15 8 * * *")).toEqual({
      type: "daily",
      hour: 8,
      minute: 15,
    });
    expect(parseCron("30 21 * * mon,wed,fri")).toEqual({
      type: "weekly",
      hour: 21,
      minute: 30,
      daysOfWeek: ["mon", "wed", "fri"],
    });
  });

  it("normalizes numeric crontab weekdays to Rust console day names", () => {
    expect(parseCron("0 9 * * 1-5")).toEqual({
      type: "weekly",
      hour: 9,
      minute: 0,
      daysOfWeek: ["mon", "tue", "wed", "thu", "fri"],
    });
    expect(parseCron("0 9 * * 0,7")).toEqual({
      type: "weekly",
      hour: 9,
      minute: 0,
      daysOfWeek: ["sun"],
    });
  });

  it("keeps lossy or unsupported expressions as custom cron", () => {
    expect(parseCron("*/15 * * * *")).toEqual({
      type: "custom",
      rawCron: "*/15 * * * *",
    });
    expect(parseCron("0 24 * * *")).toEqual({
      type: "custom",
      rawCron: "0 24 * * *",
    });
    expect(parseCron("0 9 * * fri-mon")).toEqual({
      type: "custom",
      rawCron: "0 9 * * fri-mon",
    });
  });
});

describe("serializeCron", () => {
  it("serializes presets using Rust console cron conventions", () => {
    expect(serializeCron({ type: "hourly" })).toBe("0 * * * *");
    expect(serializeCron({ type: "daily", hour: 6, minute: 5 })).toBe(
      "5 6 * * *",
    );
    expect(
      serializeCron({
        type: "weekly",
        hour: 10,
        minute: 45,
        daysOfWeek: ["mon", "tue", "wed", "fri", "sun"],
      }),
    ).toBe("45 10 * * mon-wed,fri,sun");
  });

  it("falls back to safe defaults when optional fields are missing", () => {
    expect(serializeCron({ type: "daily" })).toBe("0 9 * * *");
    expect(serializeCron({ type: "weekly", daysOfWeek: [] })).toBe(
      "0 9 * * mon",
    );
    expect(serializeCron({ type: "custom" })).toBe("0 9 * * *");
  });
});
