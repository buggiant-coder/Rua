export interface ToolInfo {
  name: string;
  enabled: boolean;
  description: string;
  async_execution: boolean;
  /**
   * Server-provided grouping key. The console renders tools by group so that
   * grouping does not depend on parsing the tool name. Falls back to `"other"`
   * when the server does not supply a value.
   */
  display_group?: string;
}

export type ToolConfig = Record<string, unknown>;
