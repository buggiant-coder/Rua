import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "@/i18n";
import AirdropTransferPage from "./index";

const syncApiMock = vi.hoisted(() => ({
  getStatus: vi.fn(),
}));

const devicesApiMock = vi.hoisted(() => ({
  listDevices: vi.fn(),
}));

const syncTransferApiMock = vi.hoisted(() => ({
  listJobs: vi.fn(),
  listDirectory: vi.fn(),
  send: vi.fn(),
  cancelJob: vi.fn(),
}));

vi.mock("@/api/modules/sync", () => ({
  syncApi: syncApiMock,
  devicesApi: devicesApiMock,
}));

vi.mock("@/api/modules/syncTransfer", () => ({
  syncTransferApi: syncTransferApiMock,
}));

const localDevice = {
  device_id: "local-device",
  public_key: "pk-local",
  friendly_name: "MacBook",
  label: "MacBook",
  added_at_ms: 1,
  last_seen_ms: 2,
  is_self: true,
  connectivity_state: "connected",
};

const remoteDevice = {
  device_id: "remote-device",
  public_key: "pk-remote",
  friendly_name: "Phone",
  label: "Phone",
  added_at_ms: 1,
  last_seen_ms: 2,
  is_self: false,
  connectivity_state: "not_connected",
  session_path: null,
  session_rtt_ms: null,
};

function renderPage() {
  return render(
    <MemoryRouter initialEntries={["/settings/airdrop-transfer"]}>
      <Routes>
        <Route
          path="/settings/airdrop-transfer"
          element={<AirdropTransferPage />}
        />
        <Route
          path="/settings/device-sync"
          element={<div>Device Sync Page</div>}
        />
      </Routes>
    </MemoryRouter>,
  );
}

describe("AirdropTransferPage", () => {
  beforeEach(async () => {
    vi.clearAllMocks();
    await i18n.changeLanguage("zh");
    syncApiMock.getStatus.mockResolvedValue({
      enabled: true,
      paired: true,
      peer_count: 1,
    });
    devicesApiMock.listDevices.mockResolvedValue([localDevice, remoteDevice]);
    syncTransferApiMock.listJobs.mockResolvedValue([]);
    syncTransferApiMock.listDirectory.mockResolvedValue({
      path: "~",
      entries: [],
    });
  });

  it("shows a pairing empty state without loading transfer data", async () => {
    devicesApiMock.listDevices.mockResolvedValue([localDevice]);

    renderPage();

    expect(
      await screen.findByText(
        "请先开启设备同步并配对至少一台其他设备，之后才能使用隔空传送。",
      ),
    ).toBeVisible();
    expect(screen.queryByText("源设备")).not.toBeInTheDocument();
    expect(syncTransferApiMock.listJobs).not.toHaveBeenCalled();
    expect(syncTransferApiMock.listDirectory).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "前往设备同步" }));
    expect(await screen.findByText("Device Sync Page")).toBeVisible();
  });

  it("renders the transfer workspace after pairing another device", async () => {
    renderPage();

    expect(await screen.findByText("源设备")).toBeVisible();
    expect(screen.getByText("目标设备")).toBeVisible();
    expect(screen.getAllByText("未连接").length).toBeGreaterThan(0);
    expect(
      screen.queryByText(
        "请先开启设备同步并配对至少一台其他设备，之后才能使用隔空传送。",
      ),
    ).not.toBeInTheDocument();
    await waitFor(() => {
      expect(syncTransferApiMock.listJobs).toHaveBeenCalled();
      expect(syncTransferApiMock.listDirectory).toHaveBeenCalledWith(
        "local-device",
        "~",
      );
      expect(syncTransferApiMock.listDirectory).toHaveBeenCalledWith(
        "remote-device",
        "~",
      );
    });
  });
});
