import { request } from "../request";
import type {
  AgentTemplate,
  CreateTemplateRequest,
  UpdateTemplateRequest,
} from "../types/agentTemplates";

export const agentTemplatesApi = {
  list: () => request<AgentTemplate[]>("/agent-templates/"),

  get: (templateId: string) =>
    request<AgentTemplate>(`/agent-templates/${templateId}`),

  create: (data: CreateTemplateRequest) =>
    request<AgentTemplate>("/agent-templates/", {
      method: "POST",
      body: JSON.stringify(data),
    }),

  update: (templateId: string, data: UpdateTemplateRequest) =>
    request<AgentTemplate>(`/agent-templates/${templateId}`, {
      method: "PUT",
      body: JSON.stringify(data),
    }),

  delete: (templateId: string) =>
    request<{ deleted: boolean }>(`/agent-templates/${templateId}`, {
      method: "DELETE",
    }),
};
