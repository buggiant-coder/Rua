import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
const api = vi.hoisted(() => ({
  computerStatus: vi.fn(),
  recheckComputer: vi.fn(),
  restartComputer: vi.fn(),
  requestComputerPermissions: vi.fn(),
  openComputerPermissionSettings: vi.fn(),
}));
vi.mock("@/api/modules/desktopAutomation", () => ({
  desktopAutomationApi: api,
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, values?: { device?: string }) => values?.device ?? key,
  }),
}));
import { ComputerUseSettings } from "./ComputerUseSettings";
const ready = {
  device_id: "execution-device",
  available: true,
  state: "ready",
  platform: "windows",
  driver_version: "0.28.2",
  permissions: {
    accessibility: true,
    screen_recording: null,
    input: true,
    integrity_level: "Medium",
  },
  diagnostics: [],
};
describe("Computer Use settings", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    api.computerStatus.mockResolvedValue(ready);
    api.recheckComputer.mockResolvedValue(ready);
    api.restartComputer.mockResolvedValue(ready);
  });
  it("shows execution-device status and rechecks after a host failure", async () => {
    api.computerStatus.mockResolvedValue({
      ...ready,
      available: false,
      state: "unavailable",
      diagnostics: ["host stopped"],
    });
    render(<ComputerUseSettings />);
    expect(
      await screen.findByText("desktop.computerUnavailableHint"),
    ).toBeVisible();
    expect(screen.getByText("host stopped")).not.toBeVisible();
    fireEvent.click(screen.getByText("desktop.technicalDetails"));
    expect(screen.getByText("host stopped")).toBeVisible();
    expect(screen.getByText("execution-device")).toBeVisible();
    fireEvent.click(
      screen.getByRole("button", { name: "desktop.browserRecheck" }),
    );
    await waitFor(() =>
      expect(screen.getByRole("status")).toHaveTextContent(
        "desktop.browserReady",
      ),
    );
    expect(screen.queryByText("host stopped")).not.toBeInTheDocument();
  });
  it("requests permissions and rechecks through the shared HTTP API", async () => {
    api.computerStatus.mockResolvedValue({
      ...ready,
      platform: "macos",
      state: "permission_required",
      permissions: {
        accessibility: true,
        screen_recording: false,
        input: true,
        integrity_level: null,
      },
    });
    render(<ComputerUseSettings />);
    const grant = await screen.findByRole("button", {
      name: "desktop.computerGrant",
    });
    expect(api.requestComputerPermissions).not.toHaveBeenCalled();
    fireEvent.click(grant);
    await waitFor(() =>
      expect(api.requestComputerPermissions).toHaveBeenCalledOnce(),
    );
    const settings = screen.getByRole("button", {
      name: "desktop.computerAccessibility",
    });
    await waitFor(() => expect(settings).toBeEnabled());
    fireEvent.click(settings);
    await waitFor(() =>
      expect(api.openComputerPermissionSettings).toHaveBeenCalledWith(
        "accessibility",
      ),
    );
    const recheck = screen.getByRole("button", {
      name: "desktop.browserRecheck",
    });
    await waitFor(() => expect(recheck).toBeEnabled());
    fireEvent.click(recheck);
    await waitFor(() => expect(api.recheckComputer).toHaveBeenCalledOnce());
  });
  it("renders partial OS permissions and preserves status when recheck fails", async () => {
    api.computerStatus.mockResolvedValue({
      ...ready,
      platform: "macos",
      state: "permission_required",
      permissions: {
        accessibility: true,
        screen_recording: false,
        input: true,
        integrity_level: null,
      },
      diagnostics: ["Screen Recording required"],
    });
    api.recheckComputer.mockRejectedValue(new Error("host disconnected"));
    render(<ComputerUseSettings />);
    expect(await screen.findByText("desktop.permissionGranted")).toBeVisible();
    expect(screen.getByText("desktop.permissionMissing")).toBeVisible();
    expect(screen.getByText("desktop.computerPermissionHint")).toBeVisible();
    fireEvent.click(
      screen.getByRole("button", { name: "desktop.browserRecheck" }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "desktop.computerRetryHint",
    );
    expect(screen.getByText("host disconnected")).not.toBeVisible();
    fireEvent.click(screen.getByText("desktop.technicalDetails"));
    expect(screen.getByText("host disconnected")).toBeVisible();
    expect(screen.getByRole("status")).toHaveTextContent(
      "desktop.computerPermissionRequired",
    );
  });
  it("restarts the execution-device driver through HTTP", async () => {
    api.computerStatus.mockResolvedValue({
      ...ready,
      state: "restart_required",
      diagnostics: [],
    });
    render(<ComputerUseSettings />);
    fireEvent.click(
      await screen.findByRole("button", { name: "desktop.computerRestart" }),
    );
    await waitFor(() =>
      expect(screen.getByRole("status")).toHaveTextContent(
        "desktop.browserReady",
      ),
    );
    expect(api.restartComputer).toHaveBeenCalledOnce();
    expect(api.recheckComputer).not.toHaveBeenCalled();
  });
  it("checks capabilities over HTTP in WebUI without a native bridge", async () => {
    render(<ComputerUseSettings />);
    await screen.findByText("desktop.browserReady");
    fireEvent.click(
      screen.getByRole("button", { name: "desktop.browserRecheck" }),
    );
    await waitFor(() => expect(api.recheckComputer).toHaveBeenCalledOnce());
  });

  it("allows an ordinary browser to request execution-device permissions", async () => {
    api.computerStatus.mockResolvedValue({
      ...ready,
      platform: "macos",
      state: "permission_required",
      permissions: {
        accessibility: false,
        screen_recording: false,
        input: false,
        integrity_level: null,
      },
    });
    render(<ComputerUseSettings />);
    expect(
      await screen.findByRole("button", { name: "desktop.computerGrant" }),
    ).toBeEnabled();
    expect(
      screen.getByRole("button", { name: "desktop.computerAccessibility" }),
    ).toBeEnabled();
    expect(api.requestComputerPermissions).not.toHaveBeenCalled();
    fireEvent.click(
      screen.getByRole("button", { name: "desktop.computerGrant" }),
    );
    await waitFor(() =>
      expect(api.requestComputerPermissions).toHaveBeenCalledOnce(),
    );
  });
});
