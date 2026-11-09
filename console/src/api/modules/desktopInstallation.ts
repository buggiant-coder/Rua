import { request } from "../request";

export interface DesktopInstallStatus {
  platform: string;
  cliPath: string;
  cliInstalled: boolean;
  cliConflict: boolean;
  legacyRuntime: boolean;
  legacyPort: number | null;
}

export interface DesktopUpdateStatus {
  phase:
    | "unavailable"
    | "idle"
    | "checking"
    | "available"
    | "up_to_date"
    | "downloading"
    | "installing"
    | "error";
  currentVersion: string;
  availableVersion: string | null;
  body: string | null;
  downloaded: number;
  total: number | null;
  error: string | null;
}

export const getDesktopInstallStatus = () =>
  request<DesktopInstallStatus>("/platform/desktop/install");

export const installDesktopCommandLineTool = () =>
  request<DesktopInstallStatus>("/platform/desktop/cli", { method: "POST" });

export const migrateDesktopLegacyRuntime = () =>
  request<{ status: "accepted" }>("/platform/desktop/legacy/migrate", {
    method: "POST",
    body: JSON.stringify({ confirmed: true }),
  });

export const getDesktopUpdateStatus = () =>
  request<DesktopUpdateStatus>("/platform/desktop/update");

export const checkDesktopUpdate = () =>
  request<DesktopUpdateStatus>("/platform/desktop/update/check", {
    method: "POST",
  });

export const installDesktopUpdate = () =>
  request<{ status: "accepted" }>("/platform/desktop/update/install", {
    method: "POST",
  });
