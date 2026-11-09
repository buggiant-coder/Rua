import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { MemoryRouter } from "react-router-dom";

import DeviceSyncPage from "./index";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (
      _key: string,
      fallback?: string | Record<string, unknown>,
      values?: Record<string, unknown>,
    ) => {
      if (typeof fallback === "string") {
        return fallback.replace(/\{\{(\w+)\}\}/g, (_match, name: string) =>
          String(values?.[name] ?? ""),
        );
      }
      if (typeof fallback === "object" && values === undefined) return _key;
      return _key;
    },
  }),
}));

const mockGetStatus = vi.fn();
const mockGetConfig = vi.fn();
const mockTriggerSyncCycle = vi.fn();
const mockGetPowerPolicy = vi.fn();
const mockUpdatePowerPolicy = vi.fn();
const mockListDevices = vi.fn();
const mockGetRecent = vi.fn();
const mockGetConflictPolicy = vi.fn();

vi.mock("../../../api/modules/sync", () => ({
  syncApi: {
    getConfig: () => mockGetConfig(),
    getStatus: () => mockGetStatus(),
    triggerSyncCycle: () => mockTriggerSyncCycle(),
    getPowerPolicy: () => mockGetPowerPolicy(),
    updatePowerPolicy: (policy: unknown) => mockUpdatePowerPolicy(policy),
  },
  devicesApi: {
    listDevices: () => mockListDevices(),
    removeDevice: vi.fn(),
    renameDevice: vi.fn(),
  },
  conflictsApi: {
    getRecent: () => mockGetRecent(),
  },
  conflictPolicyApi: {
    get: () => mockGetConflictPolicy(),
    set: vi.fn(),
  },
  pairingApi: {
    initiate: vi.fn(),
    cancel: vi.fn(),
    status: vi.fn(),
  },
}));

vi.mock("../../../api", () => ({
  default: {
    exportNetworkSecretBackup: vi.fn(),
    renderNetworkSecretBackupQr: vi
      .fn()
      .mockResolvedValue({ svg: "<svg></svg>" }),
  },
}));

const balancedPolicy = {
  host_profile: "mobile-apk",
  stored_policy: {
    mode: "balanced",
    periodic_interval_secs: 300,
  },
  effective_policy: {
    mode: "balanced",
    periodic_interval_secs: 300,
    background_flush_grace_secs: 60,
    allow_background_runtime: false,
    allow_background_scheduled_sync: true,
  },
};

function renderPage() {
  return render(
    <MemoryRouter initialEntries={["/settings/device-sync"]}>
      <DeviceSyncPage />
    </MemoryRouter>,
  );
}

describe("DeviceSyncPage power policy", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    window.localStorage.clear();
    mockGetConfig.mockResolvedValue({ enabled: true });
    mockGetStatus.mockResolvedValue({
      enabled: true,
      paired: true,
      peer_count: 1,
    });
    mockListDevices.mockResolvedValue([]);
    mockGetRecent.mockResolvedValue({ conflicts: [], count: 0 });
    mockGetConflictPolicy.mockResolvedValue({ policy: "silent" });
    mockGetPowerPolicy.mockResolvedValue(balancedPolicy);
  });

  it("renders mode-first policy copy and hides custom fields for preset modes", async () => {
    renderPage();

    expect(await screen.findByText("Sync Power Policy")).toBeInTheDocument();
    expect(await screen.findByText(/mobile-apk/)).toBeInTheDocument();
    expect(
      screen.getByText("system-scheduled background sync, not realtime"),
    ).toBeInTheDocument();
    expect(screen.getByText("Effective policy")).toBeInTheDocument();
    expect(
      screen.queryByLabelText("Periodic interval seconds"),
    ).not.toBeInTheDocument();
  });

  it("reveals custom controls only in Custom mode and saves through the backend", async () => {
    mockUpdatePowerPolicy.mockResolvedValueOnce({
      host_profile: "mobile-apk",
      stored_policy: {
        mode: "custom",
        periodic_interval_secs: 120,
        background_flush_grace_secs: 30,
        allow_background_runtime: true,
        allow_background_scheduled_sync: false,
      },
      effective_policy: {
        mode: "custom",
        periodic_interval_secs: 120,
        background_flush_grace_secs: 30,
        allow_background_runtime: true,
        allow_background_scheduled_sync: false,
      },
    });

    renderPage();

    await screen.findByText(/mobile-apk/);
    fireEvent.click(screen.getByLabelText("Custom"));
    const interval = screen.getByLabelText("Periodic interval seconds");
    fireEvent.change(interval, { target: { value: "120" } });
    fireEvent.change(screen.getByLabelText("Background flush grace seconds"), {
      target: { value: "30" },
    });
    fireEvent.click(screen.getByLabelText("Allow background runtime"));
    fireEvent.click(screen.getByLabelText("Allow scheduled background sync"));
    fireEvent.click(screen.getByRole("button", { name: "Save policy" }));

    await waitFor(() => {
      expect(mockUpdatePowerPolicy).toHaveBeenCalledWith({
        mode: "custom",
        periodic_interval_secs: 120,
        background_flush_grace_secs: 30,
        allow_background_runtime: true,
        allow_background_scheduled_sync: false,
      });
    });
    expect(await screen.findByText(/Interval: 120s/)).toBeInTheDocument();
  });

  it("shows truthful warnings for Power Saver and Realtime", async () => {
    renderPage();

    fireEvent.click(await screen.findByLabelText("Power Saver"));
    expect(
      screen.getByText(
        "lower battery use; remote changes may arrive only after reopening the app",
      ),
    ).toBeInTheDocument();

    fireEvent.click(screen.getByLabelText("Realtime"));
    expect(
      screen.getByText(
        "may increase battery use and heat, and Android background limits can still apply",
      ),
    ).toBeInTheDocument();
  });
});
