import { describe, expect, it } from "vitest";
import type { ToolInfo } from "../../../api/modules/tools";
import { groupTools } from "./toolGroups";

const tool = (name: string, display_group?: string): ToolInfo => ({
  name,
  enabled: true,
  description: name,
  async_execution: false,
  display_group,
});

describe("groupTools", () => {
  it("groups by the server-provided display_group and preserves insertion order", () => {
    const tools = [
      tool("read", "file"),
      tool("chat_send", "chat"),
      tool("edit", "file"),
      tool("web_search", "web"),
    ];
    expect(groupTools(tools)).toEqual([
      { id: "domain:file", domain: "File", tools: [tools[0], tools[2]] },
      { id: "domain:chat", domain: "Chat", tools: [tools[1]] },
      { id: "domain:web", domain: "Web", tools: [tools[3]] },
    ]);
  });

  it("groups bash and python under the shell domain via metadata", () => {
    const tools = [tool("python", "shell"), tool("bash", "shell")];
    const groups = groupTools(tools);
    expect(groups).toHaveLength(1);
    expect(groups[0].id).toBe("domain:shell");
    expect(groups[0].domain).toBe("Shell");
    expect(groups[0].tools).toEqual(tools);
  });

  it("groups list_tools and recall_tool_result under Tool via metadata", () => {
    const tools = [
      tool("recall_tool_result", "tool"),
      tool("list_tools", "tool"),
      tool("update_plan", "plan"),
    ];
    expect(groupTools(tools)).toEqual([
      { id: "domain:tool", domain: "Tool", tools: [tools[0], tools[1]] },
      { id: "domain:plan", domain: "Plan", tools: [tools[2]] },
    ]);
  });

  it("uses the 'other' bucket when display_group is missing or empty", () => {
    const tools = [tool("read", "file"), tool("standalone"), tool("blank", "")];
    expect(groupTools(tools).map(({ id }) => id)).toEqual([
      "domain:file",
      "other",
    ]);
    expect(groupTools([tools[1]])[0].id).toBe("other");
    expect(groupTools([])).toEqual([]);
  });

  it("humanizes unknown domain keys with a title-cased label", () => {
    const tools = [tool("custom_tool", "novel")];
    const [group] = groupTools(tools);
    expect(group.domain).toBe("Novel");
    expect(group.id).toBe("domain:novel");
  });
});
