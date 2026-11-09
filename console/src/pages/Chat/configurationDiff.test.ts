import { describe, expect, it } from "vitest";
import { configurationChanges } from "./configurationDiff";

describe("configurationChanges", () => {
  it("reports changed fields without unchanged configuration", () => {
    expect(
      configurationChanges(
        { max_iters: 10, context_compact: { threshold: 80, keep: 2 } },
        { max_iters: 20, context_compact: { keep: 2, threshold: 90 } },
      ),
    ).toEqual([
      {
        category: "context_compact",
        field: "context_compact.threshold",
        before: 80,
        after: 90,
      },
      { category: "max_iters", field: "max_iters", before: 10, after: 20 },
    ]);
  });
  it("ignores record key ordering and equal lists", () => {
    expect(
      configurationChanges(
        { a: { x: 1, y: 2 }, list: [1, 2] },
        { list: [1, 2], a: { y: 2, x: 1 } },
      ),
    ).toEqual([]);
  });
  it("preserves added and removed unknown fields", () => {
    expect(configurationChanges({ old: true }, { future: null })).toEqual([
      { category: "future", field: "future", before: undefined, after: null },
      { category: "old", field: "old", before: true, after: undefined },
    ]);
  });
  it("reports capability_configuration tool set changes", () => {
    const before = {
      capability_configuration: {
        tools: ["bash", "read"],
        skills: [],
      },
      security_config: { tool_guard: { approval_mode: "ask_first" } },
    };
    const after = {
      ...before,
      capability_configuration: {
        tools: [
          "mcp__rua_browser__agent_browser_open",
          "mcp__rua_computer__interact",
        ],
        skills: [],
      },
    };
    expect(configurationChanges(before, after)).toEqual([
      {
        category: "capability_configuration",
        field: "capability_configuration.tools",
        before: before.capability_configuration.tools,
        after: after.capability_configuration.tools,
      },
    ]);
  });
});
