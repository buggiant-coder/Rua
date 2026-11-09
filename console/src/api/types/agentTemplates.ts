export interface AgentTemplate {
  id: string;
  name: string;
  description: string;
  prompt_files: string[];
  builtin: boolean;
  prompt_file_count: number;
  created_at?: string;
  updated_at?: string;
  files_status?: { filename: string; exists: boolean }[];
}

export interface CreateTemplateRequest {
  name: string;
  description?: string;
  prompt_files: string[];
}

export interface UpdateTemplateRequest {
  name?: string;
  description?: string;
  prompt_files?: string[];
}
