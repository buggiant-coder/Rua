export interface PluginSummary {
  id: string;
  name: string;
  version: string;
  description: string;
  author: string;
  enabled: boolean;
  provider_count: number;
  hook_count: number;
  command_count: number;
}

export interface PluginDetail extends PluginSummary {
  entry_point: string;
  source_path: string;
  min_rua_version: string;
  meta: Record<string, unknown>;
  providers: string[];
  startup_hooks: string[];
  shutdown_hooks: string[];
  commands: string[];
  diagnostics: string[];
}

export interface InstallPluginRequest {
  source: string;
  force?: boolean;
}

export interface InstallPluginResponse {
  plugin_id: string;
  plugin_name: string;
  source_path: string;
  installed: boolean;
}

export interface UninstallPluginResponse {
  plugin_id: string;
  removed: boolean;
  requires_restart: boolean;
}
