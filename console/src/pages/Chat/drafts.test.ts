import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  DRAFT_KEY,
  clear,
  clearByPrefix,
  flush,
  load,
  loadDraft,
  save,
  scheduleSave,
  saveSubmitted,
  loadSubmitted,
  clearSubmitted,
} from "./drafts";

describe("chat drafts", () => {
  beforeEach(() => sessionStorage.clear());

  it("retains the exact submitted payload across reload reads and isolates addresses", () => {
    const submitted = {
      submissionId: "submission-original",
      text: "original input",
      attachments: [],
      request: {
        submission_id: "submission-original",
        initial_acp: { runner: "codex" },
      },
      targetDeviceId: "target",
    };
    saveSubmitted("agent-a:chat-1", submitted);
    expect(loadSubmitted("agent-a:chat-1")).toEqual(submitted);
    expect(loadSubmitted("project:agent-a:chat-1")).toBeNull();
    clearSubmitted("agent-a:chat-1");
    expect(loadSubmitted("agent-a:chat-1")).toBeNull();
  });

  it("continues to load legacy text-only drafts", () => {
    sessionStorage.setItem(DRAFT_KEY("legacy"), "old text");
    sessionStorage.setItem(DRAFT_KEY("legacy-brace"), "{not json");

    expect(load("legacy")).toBe("old text");
    expect(loadDraft("legacy")).toEqual({ text: "old text", attachments: [] });
    expect(load("legacy-brace")).toBe("{not json");
  });

  it("stores stable uploaded attachments and rejects blob URLs", () => {
    save("stable", "[Image #1]", [
      {
        id: "one",
        name: "one.png",
        type: "image",
        url: "/uploads/one.png",
        marker: "[Image #1]",
      },
    ]);
    expect(loadDraft("stable").attachments).toHaveLength(1);

    sessionStorage.setItem(
      DRAFT_KEY("blob"),
      JSON.stringify({
        text: "[Image #1]",
        attachments: [
          {
            id: "one",
            name: "one.png",
            type: "image",
            url: "blob:temporary",
            marker: "[Image #1]",
          },
        ],
      }),
    );
    expect(loadDraft("blob").attachments).toEqual([]);
  });
});

describe("pending chat drafts", () => {
  beforeEach(() => {
    clearByPrefix("");
    sessionStorage.clear();
    vi.useFakeTimers();
  });
  afterEach(() => {
    clearByPrefix("");
    vi.useRealTimers();
  });

  it("saves during sustained typing without waiting for a pause", () => {
    scheduleSave("chat", "one");
    vi.advanceTimersByTime(200);
    scheduleSave("chat", "one two");
    vi.advanceTimersByTime(50);
    expect(sessionStorage.getItem(DRAFT_KEY("chat"))).toBe("one two");
    scheduleSave("chat", "one two three");
    expect(load("chat")).toBe("one two three");
    flush("chat");
    expect(sessionStorage.getItem(DRAFT_KEY("chat"))).toBe("one two three");
  });

  it("does not restore a submitted or explicitly replaced draft when its timer fires", () => {
    scheduleSave("submitted", "send me");
    clear("submitted");
    scheduleSave("replaced", "old");
    save("replaced", "recalled draft");
    vi.advanceTimersByTime(1_000);
    expect(load("submitted")).toBe("");
    expect(load("replaced")).toBe("recalled draft");
  });

  it("discards both stored and pending drafts for a removed scope", () => {
    save("project:one:stored", "stored");
    scheduleSave("project:one:pending", "pending");
    scheduleSave("project:two:pending", "keep");
    clearByPrefix("project:one:");
    vi.advanceTimersByTime(1_000);
    expect(load("project:one:stored")).toBe("");
    expect(load("project:one:pending")).toBe("");
    expect(load("project:two:pending")).toBe("keep");
  });
});
