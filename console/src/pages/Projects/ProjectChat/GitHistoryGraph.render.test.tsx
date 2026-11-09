import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import type { ProjectGitCommit } from "@/api/types/project";

import { GitHistoryGraph } from "./GitHistoryGraph";

const commit = (oid: string, parent_oids: string[]): ProjectGitCommit => ({
  oid,
  parent_oids,
  summary: oid,
  authored_at: "2026-01-01T00:00:00Z",
  refs: [],
});

describe("GitHistoryGraph", () => {
  it("does not draw an incoming half-edge above a branch tip", () => {
    const { container } = render(
      <GitHistoryGraph commits={[commit("tip", ["parent"])]} />,
    );

    const graph = container.querySelector("svg");
    expect(graph?.querySelectorAll("circle")).toHaveLength(1);
    expect(graph?.querySelectorAll("path")).toHaveLength(1);
    expect(graph?.querySelector("path")?.getAttribute("d")).not.toContain(
      " 0 L ",
    );
  });

  it("colors a first-parent branch edge like its source commit", () => {
    const { container } = render(
      <GitHistoryGraph
        commits={[
          commit("merge", ["branch", "base"]),
          commit("branch", ["base"]),
        ]}
      />,
    );

    const branchGraph = container.querySelectorAll("svg")[1];
    const circle = branchGraph.querySelector("circle");
    const circleColor = circle?.getAttribute("stroke");
    const nodeX = circle?.getAttribute("cx");
    const outgoingColor = Array.from(branchGraph.querySelectorAll("path"))
      .find((path) => path.getAttribute("d")?.startsWith(`M ${nodeX} 19`))
      ?.getAttribute("stroke");

    expect(outgoingColor).toBe(circleColor);
  });
});
