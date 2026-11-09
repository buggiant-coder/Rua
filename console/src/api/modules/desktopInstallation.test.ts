import { beforeEach, describe, expect, it, vi } from "vitest";
const http = vi.hoisted(() => ({ request: vi.fn() }));
vi.mock("../request", () => http);
import {
  getDesktopInstallStatus,
  installDesktopCommandLineTool,
  getDesktopUpdateStatus,
  checkDesktopUpdate,
  installDesktopUpdate,
  migrateDesktopLegacyRuntime,
} from "./desktopInstallation";
import { desktopAutomationApi } from "./desktopAutomation";

describe("desktop operations use HTTP for every viewer", () => {
  beforeEach(() => vi.clearAllMocks());
  it("installs CLI and requests signed updates without a native browser bridge", async () => {
    await getDesktopInstallStatus();
    await installDesktopCommandLineTool();
    await getDesktopUpdateStatus();
    await checkDesktopUpdate();
    await installDesktopUpdate();
    await migrateDesktopLegacyRuntime();
    expect(http.request.mock.calls).toEqual([
      ["/platform/desktop/install"],
      ["/platform/desktop/cli", { method: "POST" }],
      ["/platform/desktop/update"],
      ["/platform/desktop/update/check", { method: "POST" }],
      ["/platform/desktop/update/install", { method: "POST" }],
      [
        "/platform/desktop/legacy/migrate",
        { method: "POST", body: JSON.stringify({ confirmed: true }) },
      ],
    ]);
  });
  it("requests permissions and recovery on the connected execution device", async () => {
    await desktopAutomationApi.requestComputerPermissions();
    await desktopAutomationApi.openComputerPermissionSettings(
      "screen_recording",
    );
    await desktopAutomationApi.recheckComputer();
    await desktopAutomationApi.restartComputer();
    expect(http.request.mock.calls).toEqual([
      ["/platform/computer/permissions", { method: "POST" }],
      [
        "/platform/computer/permission-settings",
        {
          method: "POST",
          body: JSON.stringify({ setting: "screen_recording" }),
        },
      ],
      ["/platform/computer/recheck", { method: "POST" }],
      ["/platform/computer/restart", { method: "POST" }],
    ]);
  });
});
