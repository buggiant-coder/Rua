export interface SubAgentTemplate {
  id: string;
  name: string;
  description: string;
  system_prompt: string;
  tools: string[];
  visible_to: string[];
  builtin: boolean;
}

export interface CreateSubAgentTemplateRequest {
  name: string;
  description?: string;
  system_prompt: string;
  tools?: string[];
  visible_to?: string[];
}

export interface UpdateSubAgentTemplateRequest {
  name?: string;
  description?: string;
  system_prompt?: string;
  tools?: string[];
  visible_to?: string[];
}
