import { request } from "../request";

export interface BrowserUseStatus {
  device_id: string | null;
  available: boolean;
  state: "ready" | "unavailable";
  selected_path: string | null;
  resolved_path: string | null;
  driver_version: string;
  diagnostics: string[];
}

export interface ComputerUseStatus {
  device_id: string | null;
  available: boolean;
  state: "ready" | "unavailable" | "permission_required" | "restart_required";
  platform: string;
  driver_version: string;
  permissions: {
    accessibility: boolean;
    screen_recording: boolean | null;
    input: boolean;
    integrity_level: string | null;
  } | null;
  diagnostics: string[];
}

export const desktopAutomationApi = {
  restartComputer: () =>
    request<ComputerUseStatus>("/platform/computer/restart", {
      method: "POST",
    }),
  requestComputerPermissions: () =>
    request<{ accessibility: boolean; screen_recording: boolean }>(
      "/platform/computer/permissions",
      { method: "POST" },
    ),
  openComputerPermissionSettings: (
    setting: "accessibility" | "screen_recording",
  ) =>
    request<void>("/platform/computer/permission-settings", {
      method: "POST",
      body: JSON.stringify({ setting }),
    }),
  computerStatus: () => request<ComputerUseStatus>("/platform/computer"),
  recheckComputer: () =>
    request<ComputerUseStatus>("/platform/computer/recheck", {
      method: "POST",
    }),
  browserStatus: () => request<BrowserUseStatus>("/platform/browser"),
  selectBrowser: (path: string | null) =>
    request<BrowserUseStatus>("/platform/browser", {
      method: "PUT",
      body: JSON.stringify({ path }),
    }),
  recheckBrowser: () =>
    request<BrowserUseStatus>("/platform/browser/recheck", { method: "POST" }),
};
