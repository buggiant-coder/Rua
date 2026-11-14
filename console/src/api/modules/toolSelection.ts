import { request } from "../request";

export interface ToolIdentity {
  kind: "builtin" | "mcp" | "plugin";
  source?: string;
  name: string;
  [key: string]: unknown;
}

export type ToolSelectionPolicy =
  | {
      mode: "default_enabled";
      disabled_tools: ToolIdentity[];
      disabled_capabilities: string[];
      [key: string]: unknown;
    }
  | {
      mode: "restricted";
      allowed_tools: ToolIdentity[];
      [key: string]: unknown;
    };

export interface ToolSelectionPolicyResponse {
  selection_policy: ToolSelectionPolicy;
}

export interface ToolSelectionPolicyPreview {
  expected: unknown;
  current: ToolSelectionPolicy;
  candidate: ToolSelectionPolicy;
}

const path = (agentId: string) =>
  `/agents/${encodeURIComponent(agentId)}/tools/selection-policy`;

export const toolSelectionApi = {
  get: (agentId: string) => request<ToolSelectionPolicyResponse>(path(agentId)),
  preview: (agentId: string, selectionPolicy: ToolSelectionPolicy) =>
    request<ToolSelectionPolicyPreview>(`${path(agentId)}/preview`, {
      method: "POST",
      body: JSON.stringify({ selection_policy: selectionPolicy }),
    }),
  apply: (agentId: string, preview: ToolSelectionPolicyPreview) =>
    request<ToolSelectionPolicyResponse>(path(agentId), {
      method: "PUT",
      body: JSON.stringify({
        expected: preview.expected,
        candidate: preview.candidate,
      }),
    }),
};
