import { describe, expect, it } from "vitest";

import type { ProjectGitCommit } from "@/api/types/project";

import { layoutGitGraph } from "./gitHistoryLayout";

const commit = (oid: string, parent_oids: string[]): ProjectGitCommit => ({
  oid,
  parent_oids,
  summary: oid,
  authored_at: "2026-01-01T00:00:00Z",
  refs: [],
});

describe("layoutGitGraph", () => {
  it("keeps side branches in separate lanes until a merge", () => {
    const rows = layoutGitGraph([
      commit("merge", ["main", "feature"]),
      commit("feature", ["base"]),
      commit("main", ["base"]),
      commit("base", []),
    ]);

    expect(rows[0].outgoing.map(({ oid }) => oid)).toEqual(["main", "feature"]);
    expect(rows[1].lane).toBe(1);
    expect(rows[2].lane).toBe(0);
    expect(rows[3].incoming.map(({ oid }) => oid)).toEqual(["base"]);
  });

  it("marks a branch tip as starting a lane without an incoming edge", () => {
    const rows = layoutGitGraph([
      commit("head", ["parent"]),
      commit("parent", []),
    ]);

    expect(rows[0].startsLane).toBe(true);
    expect(rows[1].startsLane).toBe(false);
    expect(rows[0].incoming[0].color).toBe(rows[0].outgoing[0].color);
    expect(rows[1].incoming[0].color).toBe(rows[0].incoming[0].color);
  });

  it("keeps an independently discovered branch tip disconnected above", () => {
    const rows = layoutGitGraph([
      commit("main-head", ["base"]),
      commit("branch-head", ["base"]),
      commit("base", []),
    ]);

    expect(rows[1].startsLane).toBe(true);
    expect(rows[1].incoming[rows[1].lane].oid).toBe("branch-head");
  });
});
