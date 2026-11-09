import { describe, expect, it } from "vitest";
import { formatTime, type Session } from "./constants";
import {
  filterAndSortSessions,
  normalizeIsoTimestamp,
  timestampValue,
} from "./sessionListModel";

const baseSession = (overrides: Partial<Session>): Session => ({
  id: "session",
  user_id: "user",
  channel: "console",
  name: "Session",
  created_at: "2026-06-18T00:00:00Z",
  updated_at: "2026-06-18T00:00:00Z",
  meta: {},
  status: "idle",
  ...overrides,
});

const ids = (sessions: Session[]) => sessions.map((session) => session.id);

describe("session list model", () => {
  it("normalizes timezone-less ISO timestamps as UTC", () => {
    expect(normalizeIsoTimestamp("2026-06-18T12:30:00")).toBe(
      "2026-06-18T12:30:00Z",
    );
    expect(normalizeIsoTimestamp("2026-06-18T12:30:00Z")).toBe(
      "2026-06-18T12:30:00Z",
    );
    expect(normalizeIsoTimestamp("2026-06-18T12:30:00+08:00")).toBe(
      "2026-06-18T12:30:00+08:00",
    );
    expect(timestampValue("not-a-date")).toBe(0);
  });

  it("formats timezone-less and explicit UTC session timestamps consistently", () => {
    expect(formatTime("2026-06-18T12:30:00")).toBe(
      formatTime("2026-06-18T12:30:00Z"),
    );
    expect(formatTime(null)).toBe("N/A");
  });

  it("sorts default sessions by updated_at activity with newest first", () => {
    const sessions = [
      baseSession({
        id: "created-newer-but-stale",
        created_at: "2026-06-18T00:00:00Z",
        updated_at: "2026-06-10T00:00:00Z",
      }),
      baseSession({
        id: "created-older-but-active",
        created_at: "2026-06-01T00:00:00Z",
        updated_at: "2026-06-18T12:00:00",
      }),
      baseSession({
        id: "missing-activity",
        updated_at: null,
      }),
    ];

    expect(ids(filterAndSortSessions(sessions, "", "default", "desc"))).toEqual(
      [
        "created-older-but-active",
        "created-newer-but-stale",
        "missing-activity",
      ],
    );
    expect(ids(filterAndSortSessions(sessions, "", "default", "asc"))).toEqual([
      "missing-activity",
      "created-newer-but-stale",
      "created-older-but-active",
    ]);
  });

  it("filters by title case-insensitively before sorting", () => {
    const sessions = [
      baseSession({
        id: "alpha-2",
        name: "Alpha 2",
        updated_at: "2026-06-10T00:00:00Z",
      }),
      baseSession({
        id: "beta",
        name: "Beta",
        updated_at: "2026-06-19T00:00:00Z",
      }),
      baseSession({
        id: "alpha-10",
        name: "alpha 10",
        updated_at: "2026-06-18T00:00:00Z",
      }),
    ];

    expect(
      ids(filterAndSortSessions(sessions, " ALPHA ", "name", "asc")),
    ).toEqual(["alpha-2", "alpha-10"]);
  });

  it("sorts user and channel fields with numeric collation", () => {
    const sessions = [
      baseSession({ id: "user-10", user_id: "user10", channel: "telegram" }),
      baseSession({ id: "user-2", user_id: "user2", channel: "console" }),
      baseSession({ id: "user-1", user_id: "user1", channel: "dingtalk" }),
    ];

    expect(ids(filterAndSortSessions(sessions, "", "user_id", "asc"))).toEqual([
      "user-1",
      "user-2",
      "user-10",
    ]);
    expect(ids(filterAndSortSessions(sessions, "", "channel", "desc"))).toEqual(
      ["user-10", "user-1", "user-2"],
    );
  });
});
