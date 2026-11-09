import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const desktopMock = vi.hoisted(() => ({
  getStatus: vi.fn(),
  getUpdateStatus: vi.fn(),
  checkUpdate: vi.fn(),
  installUpdate: vi.fn(),
  installCli: vi.fn(),
  migrate: vi.fn(),
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, values?: { port?: number }) =>
      key === "desktop.legacyDescription"
        ? `Legacy Runtime on ${values?.port}`
        : key,
  }),
}));

vi.mock("@/api/modules/desktopInstallation", () => ({
  getDesktopInstallStatus: desktopMock.getStatus,
  getDesktopUpdateStatus: desktopMock.getUpdateStatus,
  checkDesktopUpdate: desktopMock.checkUpdate,
  installDesktopUpdate: desktopMock.installUpdate,
  installDesktopCommandLineTool: desktopMock.installCli,
  migrateDesktopLegacyRuntime: desktopMock.migrate,
}));

vi.mock("@/ui/Toast", () => ({
  default: {
    success: vi.fn(),
    error: vi.fn(),
  },
}));

import AboutPage from "./index";

const legacyStatus = {
  platform: "macos",
  cliPath: "/Users/test/.local/bin/rua",
  cliInstalled: false,
  cliConflict: false,
  legacyRuntime: true,
  legacyPort: 2026,
};

describe("AboutPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    desktopMock.getStatus.mockResolvedValue(legacyStatus);
    desktopMock.getUpdateStatus.mockResolvedValue({
      phase: "idle",
      currentVersion: "0.1.0",
      availableVersion: null,
      body: null,
      downloaded: 0,
      total: null,
      error: null,
    });
    desktopMock.checkUpdate.mockResolvedValue({
      phase: "available",
      currentVersion: "0.1.0",
      availableVersion: "0.2.0",
      body: "Release notes",
      downloaded: 0,
      total: null,
      error: null,
    });
    desktopMock.installUpdate.mockResolvedValue(undefined);
    desktopMock.installCli.mockResolvedValue({
      ...legacyStatus,
      cliInstalled: true,
    });
    desktopMock.migrate.mockResolvedValue({
      ...legacyStatus,
      legacyRuntime: false,
      legacyPort: null,
    });
  });

  it("shows execution-device installation and legacy Runtime status", async () => {
    render(<AboutPage />);

    expect(await screen.findByText("desktop.cliTitle")).toBeVisible();
    expect(screen.getByText("/Users/test/.local/bin/rua")).toBeVisible();
    expect(screen.getByText("Legacy Runtime on 2026")).toBeVisible();
  });

  it("installs the packaged command-line tool explicitly", async () => {
    render(<AboutPage />);

    fireEvent.click(
      await screen.findByRole("button", { name: "desktop.installCli" }),
    );

    await waitFor(() => expect(desktopMock.installCli).toHaveBeenCalledOnce());
    expect(
      await screen.findByRole("button", { name: "desktop.installed" }),
    ).toBeDisabled();
  });

  it("does not migrate the legacy Runtime before confirmation", async () => {
    render(<AboutPage />);

    fireEvent.click(
      await screen.findByRole("button", { name: "desktop.migrate" }),
    );
    expect(desktopMock.migrate).not.toHaveBeenCalled();

    fireEvent.click(
      screen.getAllByRole("button", { name: "desktop.migrate" }).at(-1)!,
    );

    await waitFor(() => expect(desktopMock.migrate).toHaveBeenCalledOnce());
    await waitFor(() =>
      expect(
        screen.queryByText("desktop.migrationConfirmTitle"),
      ).not.toBeInTheDocument(),
    );
  });

  it("checks and installs a signed Desktop update explicitly", async () => {
    render(<AboutPage />);

    fireEvent.click(
      await screen.findByRole("button", {
        name: "desktop.checkForUpdates",
      }),
    );
    await waitFor(() => expect(desktopMock.checkUpdate).toHaveBeenCalledOnce());

    fireEvent.click(
      await screen.findByRole("button", { name: "desktop.installUpdate" }),
    );
    await waitFor(() =>
      expect(desktopMock.installUpdate).toHaveBeenCalledOnce(),
    );
  });

  it("exposes CLI installation and updates in an ordinary browser", async () => {
    render(<AboutPage />);
    expect(
      await screen.findByRole("button", { name: "desktop.installCli" }),
    ).toBeEnabled();
    expect(
      screen.getByRole("button", { name: "desktop.checkForUpdates" }),
    ).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "desktop.installCli" }));
    await waitFor(() => expect(desktopMock.installCli).toHaveBeenCalledOnce());
  });
});
