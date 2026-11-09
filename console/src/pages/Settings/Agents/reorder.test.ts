import { describe, expect, it } from "vitest";
import type { AgentSummary } from "@/api/types/agents";
import { reorderAgents } from "./reorder";

function agent(id: string): AgentSummary {
  return {
    id,
    name: id,
    description: "",
    workspace_dir: `/tmp/${id}`,
    enabled: true,
  };
}

describe("reorderAgents", () => {
  it("moves the active Rust agent before the drop target", () => {
    const agents = [agent("default"), agent("coder"), agent("research")];

    const result = reorderAgents(agents, "research", "default");

    expect(result.map((item) => item.id)).toEqual([
      "research",
      "default",
      "coder",
    ]);
    expect(agents.map((item) => item.id)).toEqual([
      "default",
      "coder",
      "research",
    ]);
  });

  it("keeps ordering stable for self-drops and missing ids", () => {
    const agents = [agent("default"), agent("coder"), agent("research")];

    expect(reorderAgents(agents, "coder", "coder")).toBe(agents);
    expect(reorderAgents(agents, "missing", "coder")).toBe(agents);
    expect(reorderAgents(agents, "coder", "missing")).toBe(agents);
  });
});
