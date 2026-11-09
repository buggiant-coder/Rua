import { request } from "../request";
import type {
  SubAgentTemplate,
  CreateSubAgentTemplateRequest,
  UpdateSubAgentTemplateRequest,
} from "../types/subagentTemplates";

export const subagentTemplatesApi = {
  list: (agentId?: string) =>
    request<SubAgentTemplate[]>(
      `/subagent-templates${agentId ? `?agent_id=${agentId}` : ""}`,
    ),

  get: (id: string) => request<SubAgentTemplate>(`/subagent-templates/${id}`),

  create: (data: CreateSubAgentTemplateRequest) =>
    request<SubAgentTemplate>("/subagent-templates", {
      method: "POST",
      body: JSON.stringify(data),
    }),

  update: (id: string, data: UpdateSubAgentTemplateRequest) =>
    request<SubAgentTemplate>(`/subagent-templates/${id}`, {
      method: "PUT",
      body: JSON.stringify(data),
    }),

  delete: (id: string) =>
    request<{ deleted: boolean }>(`/subagent-templates/${id}`, {
      method: "DELETE",
    }),
};
