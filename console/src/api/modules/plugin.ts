import { request } from "../request";
import type {
  PluginSummary,
  PluginDetail,
  InstallPluginResponse,
  UninstallPluginResponse,
} from "../types";

export const pluginApi = {
  listPlugins: () => request<PluginSummary[]>("/plugins"),

  getPlugin: (id: string) =>
    request<PluginDetail>(`/plugins/${encodeURIComponent(id)}`),

  installPlugin: (source: string, force = false) =>
    request<InstallPluginResponse>("/plugins/install", {
      method: "POST",
      body: JSON.stringify({ source, force }),
    }),

  uninstallPlugin: (id: string) =>
    request<UninstallPluginResponse>(`/plugins/${encodeURIComponent(id)}`, {
      method: "DELETE",
    }),
};
