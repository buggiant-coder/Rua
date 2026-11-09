import type { ToolInfo } from "../../../api/modules/tools";

export interface ToolGroup {
  id: string;
  domain: string;
  tools: ToolInfo[];
}

const DOMAIN_LABELS: Record<string, string> = {
  shell: "Shell",
  file: "File",
  media: "Media",
  time: "Time",
  usage: "Usage",
  runtime: "Runtime",
  conversation: "Conversation",
  tool: "Tool",
  web: "Web",
  task: "Task",
  memory: "Memory",
  plan: "Plan",
  chat: "Chat",
  user: "User",
  sync: "Sync",
};

function humanizeDomain(key: string): string {
  if (!key) return "";
  if (DOMAIN_LABELS[key]) return DOMAIN_LABELS[key];
  return key.charAt(0).toUpperCase() + key.slice(1);
}

/**
 * Groups tools by their server-provided `display_group` metadata. Grouping is
 * decoupled from naming: `bash` and `python` share the `shell` group;
 * `list_tools` and `recall_tool_result` share `tool`; `update_plan` stands
 * alone in `plan`. Tools without a group fall into an `other` bucket. Group
 * insertion order is preserved from the input tool order.
 */
export function groupTools(tools: ToolInfo[]): ToolGroup[] {
  const groups = new Map<string, ToolGroup>();

  for (const tool of tools) {
    const key = (tool.display_group ?? "").toLowerCase();
    const id = key ? `domain:${key}` : "other";
    const domain = key ? humanizeDomain(key) : "";
    const group = groups.get(id);
    if (group) {
      group.tools.push(tool);
    } else {
      groups.set(id, { id, domain, tools: [tool] });
    }
  }

  return [...groups.values()];
}
