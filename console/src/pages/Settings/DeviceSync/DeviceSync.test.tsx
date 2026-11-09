import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { MemoryRouter } from "react-router-dom";
import DeviceSyncPage from "./index";
import type { SyncDiagnosticsResponse } from "../../../api/modules/sync";

const mockMessage = { success: vi.fn(), error: vi.fn() };
vi.mock("../../../hooks/useAppMessage", () => ({
  useAppMessage: () => ({ message: mockMessage }),
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, fallback?: string | Record<string, unknown>) => {
      if (typeof fallback === "string") return fallback;
      return key;
    },
  }),
}));

const mockGetStatus = vi.fn();
const mockGetConfig = vi.fn();
const mockGetDiagnostics = vi.fn();
const mockEnable = vi.fn();
const mockDisable = vi.fn();
const mockGetPowerPolicy = vi.fn();
const mockUpdatePowerPolicy = vi.fn();
const mockListDevices = vi.fn();
const mockWatchDeviceChanges = vi.fn();
const mockRemoveDevice = vi.fn();
const mockRenameDevice = vi.fn();
const mockGetRecent = vi.fn();
const mockExportNetworkSecretBackup = vi.fn();
const mockPairingInitiate = vi.fn();
const mockPairingCancel = vi.fn();
const mockPairingStatus = vi.fn();

const mockGetConflictPolicy = vi.fn();
const mockSetConflictPolicy = vi.fn();

vi.mock("../../../api/modules/sync", () => ({
  syncApi: {
    getConfig: () => mockGetConfig(),
    getStatus: () => mockGetStatus(),
    getDiagnostics: () => mockGetDiagnostics(),
    enable: () => mockEnable(),
    disable: () => mockDisable(),
    getPowerPolicy: () => mockGetPowerPolicy(),
    updatePowerPolicy: (policy: unknown) => mockUpdatePowerPolicy(policy),
  },
  devicesApi: {
    listDevices: () => mockListDevices(),
    watchChanges: (listener: () => void) => mockWatchDeviceChanges(listener),
    removeDevice: (id: string) => mockRemoveDevice(id),
    renameDevice: (id: string, name: string) => mockRenameDevice(id, name),
  },
  conflictsApi: {
    getRecent: () => mockGetRecent(),
  },
  conflictPolicyApi: {
    get: () => mockGetConflictPolicy(),
    set: (p: string) => mockSetConflictPolicy(p),
  },
  pairingApi: {
    initiate: () => mockPairingInitiate(),
    cancel: () => mockPairingCancel(),
    status: () => mockPairingStatus(),
  },
}));

vi.mock("../../../api", () => ({
  default: {
    exportNetworkSecretBackup: (p: string) => mockExportNetworkSecretBackup(p),
    renderNetworkSecretBackupQr: vi
      .fn()
      .mockResolvedValue({ svg: "<svg></svg>" }),
  },
}));

function renderPage() {
  return render(
    <MemoryRouter initialEntries={["/settings/device-sync"]}>
      <DeviceSyncPage />
    </MemoryRouter>,
  );
}

function makeDiagnostics(
  overrides: Partial<SyncDiagnosticsResponse> = {},
): SyncDiagnosticsResponse {
  const now = Date.now();
  const base: SyncDiagnosticsResponse = {
    host_profile: "mobile-apk",
    effective_policy: {
      mode: "power_saver",
      periodic_interval_secs: 600,
      background_flush_grace_secs: 60,
      allow_background_runtime: false,
      allow_background_scheduled_sync: false,
    },
    runtime_state: {
      state: "active",
      pending_grace: false,
      background_grace: { pending: false },
      warm_suspended: false,
    },
    inbox: {
      counts: { pending: 0, retryable: 0, dormant: 0, applied: 3, failed: 0 },
    },
    progress: {
      state: "up_to_date",
      is_caught_up: true,
      pending_inbound: 0,
      retryable_inbound: 0,
      dormant_inbound: 0,
      failed_inbound: 0,
      pending_conflicts: 0,
    },
    transport: {
      last_cycle: {
        completed_at_ms: now,
        peer_node_id: "peer-1",
        status: "success",
        events_received: 3,
        events_sent: 1,
        connect_ms: 20,
        process_ms: 40,
        duration_ms: 60,
        route: "direct",
        direct_tx_bytes: 2048,
        direct_rx_bytes: 1024,
        relay_tx_bytes: 0,
        relay_rx_bytes: 0,
        direct_rtt_ms: 24,
      },
      rolling: {
        cycles: 1,
        failures: 0,
        events_received: 3,
        events_sent: 1,
        direct_tx_bytes: 2048,
        direct_rx_bytes: 1024,
        relay_tx_bytes: 0,
        relay_rx_bytes: 0,
      },
    },
    peers: {
      total_peers: 1,
      selectable_peers: 1,
      peers_in_cooldown: 0,
      peers: [
        {
          node_id: "peer-1",
          device_id_hex: "dev-1",
          last_seen_or_added_ms: now,
          selectable: true,
          consecutive_failures: 0,
          last_success_ms: now,
          sync_timeout_ms: 30000,
          last_route: "direct",
          last_direct_bytes: 3072,
          last_relay_bytes: 0,
          last_rtt_ms: 24,
        },
      ],
    },
  };

  return {
    ...base,
    ...overrides,
    effective_policy: {
      ...base.effective_policy,
      ...overrides.effective_policy,
    },
    runtime_state: {
      ...base.runtime_state,
      ...overrides.runtime_state,
      background_grace: {
        ...base.runtime_state.background_grace,
        ...overrides.runtime_state?.background_grace,
      },
    },
    inbox: {
      ...base.inbox,
      ...overrides.inbox,
      counts: {
        ...base.inbox.counts,
        ...overrides.inbox?.counts,
      },
    },
    progress: {
      ...base.progress,
      ...overrides.progress,
    },
    transport: {
      ...base.transport,
      ...overrides.transport,
      rolling: {
        ...base.transport.rolling,
        ...overrides.transport?.rolling,
      },
    },
    peers: {
      ...base.peers,
      ...overrides.peers,
    },
  };
}

describe("DeviceSyncPage", () => {
  beforeEach(() => {
    mockGetStatus.mockReset();
    mockGetConfig.mockReset();
    mockGetDiagnostics.mockReset();
    mockEnable.mockReset();
    mockDisable.mockReset();
    mockGetPowerPolicy.mockReset();
    mockUpdatePowerPolicy.mockReset();
    mockListDevices.mockReset();
    mockWatchDeviceChanges.mockReset();
    mockRemoveDevice.mockReset();
    mockRenameDevice.mockReset();
    mockGetRecent.mockReset();
    mockExportNetworkSecretBackup.mockReset();
    mockPairingInitiate.mockReset();
    mockPairingCancel.mockReset();
    mockPairingStatus.mockReset();
    mockGetConflictPolicy.mockReset();
    mockSetConflictPolicy.mockReset();
    window.localStorage.clear();
    vi.useFakeTimers({ shouldAdvanceTime: true });
    mockGetConfig.mockResolvedValue({ enabled: true });
    mockGetStatus.mockResolvedValue({
      enabled: true,
      paired: true,
      peer_count: 2,
    });
    mockGetDiagnostics.mockResolvedValue(makeDiagnostics());
    mockPairingInitiate.mockResolvedValue({
      pin: "12345678",
      expires_in_secs: 300,
    });
    mockPairingStatus.mockResolvedValue({ status: "active" });
    mockPairingCancel.mockResolvedValue({ cancelled: true });
    mockGetPowerPolicy.mockResolvedValue({
      host_profile: "mobile-apk",
      stored_policy: null,
      effective_policy: {
        mode: "power_saver",
        periodic_interval_secs: 600,
        background_flush_grace_secs: 60,
        allow_background_runtime: false,
        allow_background_scheduled_sync: false,
      },
    });
    mockListDevices.mockResolvedValue([
      {
        device_id: "dev-1",
        public_key: "peer-1",
        friendly_name: "Phone",
        label: "android",
        added_at_ms: Date.now(),
        last_seen_ms: Date.now(),
        is_self: false,
        connectivity_state: "connected",
        session_path: "relay",
        session_rtt_ms: 77,
      },
    ]);
    mockWatchDeviceChanges.mockReturnValue(() => {});
    mockGetRecent.mockResolvedValue({ conflicts: [], count: 0 });
    mockGetConflictPolicy.mockResolvedValue({ policy: "silent" });
  });

  it("renders all 5 sections when sync enabled", async () => {
    renderPage();

    await waitFor(() => {
      expect(screen.getByRole("switch")).toBeInTheDocument();
    });

    await waitFor(() => {
      expect(screen.getByText("Sync Power Policy")).toBeInTheDocument();
      expect(screen.getByText("Devices")).toBeInTheDocument();
      expect(screen.getByText("Conflict Policy")).toBeInTheDocument();
      expect(screen.getByText("Sync Conflicts")).toBeInTheDocument();
    });
  });

  it("renders sync progress while using Device Session route stats for each Device", async () => {
    renderPage();

    await waitFor(() => {
      expect(screen.getByText("Up to date")).toBeInTheDocument();
      expect(screen.getByText("本设备当前已补全")).toBeInTheDocument();
      expect(screen.queryByText("所有设备已同步完成")).not.toBeInTheDocument();
      expect(screen.getAllByText("Direct").length).toBeGreaterThan(0);
      expect(screen.getByText("Relay")).toBeInTheDocument();
      expect(screen.getByText("Direct ratio")).toBeInTheDocument();
      expect(screen.getByText(/3 received \/ 1 sent/)).toBeInTheDocument();
      expect(
        screen.getByText(/3.0 KB direct \/ 3.0 KB total/),
      ).toBeInTheDocument();
      expect(screen.getByText(/77 ms RTT/)).toBeInTheDocument();
    });
  });

  it("renders applying changes when transfer finished but inbound apply is pending", async () => {
    mockGetDiagnostics.mockResolvedValue(
      makeDiagnostics({
        inbox: {
          counts: {
            pending: 2,
            retryable: 1,
            dormant: 0,
            applied: 4,
            failed: 0,
          },
        },
        progress: {
          state: "applying_changes",
          is_caught_up: false,
          pending_inbound: 2,
          retryable_inbound: 1,
          dormant_inbound: 0,
          failed_inbound: 0,
          pending_conflicts: 0,
        },
      }),
    );

    renderPage();

    await waitFor(() => {
      expect(screen.getByText("Applying changes")).toBeInTheDocument();
      expect(screen.getByText(/3 pending/)).toBeInTheDocument();
      expect(screen.getByText(/0 waiting for content/)).toBeInTheDocument();
      expect(screen.getByText(/0 failed, 0 conflicts/)).toBeInTheDocument();
    });
  });

  it("renders waiting for content separately from pending apply work", async () => {
    mockGetDiagnostics.mockResolvedValue(
      makeDiagnostics({
        inbox: {
          counts: {
            pending: 0,
            retryable: 0,
            dormant: 328,
            applied: 4,
            failed: 0,
          },
        },
        progress: {
          state: "waiting_for_content",
          is_caught_up: false,
          pending_inbound: 0,
          retryable_inbound: 0,
          dormant_inbound: 328,
          failed_inbound: 0,
          pending_conflicts: 0,
        },
      }),
    );

    renderPage();

    await waitFor(() => {
      expect(screen.getByText("Waiting for content")).toBeInTheDocument();
      expect(screen.getByText(/0 pending/)).toBeInTheDocument();
      expect(screen.getByText(/328 waiting for content/)).toBeInTheDocument();
    });
  });

  it("renders needs attention for failed inbound work and conflicts", async () => {
    mockGetDiagnostics.mockResolvedValue(
      makeDiagnostics({
        inbox: {
          counts: {
            pending: 0,
            retryable: 0,
            dormant: 0,
            applied: 4,
            failed: 1,
          },
        },
        progress: {
          state: "needs_attention",
          is_caught_up: false,
          pending_inbound: 0,
          retryable_inbound: 0,
          dormant_inbound: 0,
          failed_inbound: 1,
          pending_conflicts: 2,
          last_cycle_status: "failure",
        },
        transport: {
          last_cycle: {
            completed_at_ms: Date.now(),
            peer_node_id: "peer-1",
            status: "failure",
            error: "peer unreachable",
            events_received: 0,
            events_sent: 0,
            connect_ms: 0,
            process_ms: 0,
            duration_ms: 30000,
            route: "unknown",
            direct_tx_bytes: 0,
            direct_rx_bytes: 0,
            relay_tx_bytes: 0,
            relay_rx_bytes: 0,
          },
        },
      }),
    );

    renderPage();

    await waitFor(() => {
      expect(screen.getByText("Needs attention")).toBeInTheDocument();
      expect(screen.getByText(/1 failed, 2 conflicts/)).toBeInTheDocument();
      expect(screen.getByText("Not measured")).toBeInTheDocument();
    });
  });

  it("renders peer unavailability as waiting while the local device is caught up", async () => {
    mockGetDiagnostics.mockResolvedValue(
      makeDiagnostics({
        progress: {
          state: "waiting_for_device",
          is_caught_up: true,
          pending_inbound: 0,
          retryable_inbound: 0,
          dormant_inbound: 0,
          failed_inbound: 0,
          pending_conflicts: 0,
          last_cycle_status: "failure",
        },
        transport: {
          last_cycle: {
            completed_at_ms: Date.now(),
            peer_node_id: "peer-1",
            status: "failure",
            error: "peer unreachable: connection refused",
            failure_kind: "peer_unavailable",
            events_received: 0,
            events_sent: 0,
            connect_ms: 0,
            process_ms: 0,
            duration_ms: 30000,
            route: "unknown",
            direct_tx_bytes: 0,
            direct_rx_bytes: 0,
            relay_tx_bytes: 0,
            relay_rx_bytes: 0,
          },
        },
      }),
    );

    renderPage();

    await waitFor(() => {
      expect(
        screen.getByText("Waiting for another device"),
      ).toBeInTheDocument();
      expect(screen.queryByText("Needs attention")).not.toBeInTheDocument();
      expect(
        screen.getByText(/本设备当前已补全，正在等待其他设备上线/),
      ).toBeInTheDocument();
    });
  });

  it("renders not paired as a neutral setup state", async () => {
    mockGetDiagnostics.mockResolvedValue(
      makeDiagnostics({
        progress: {
          state: "not_paired",
          is_caught_up: false,
          pending_inbound: 0,
          retryable_inbound: 0,
          dormant_inbound: 0,
          failed_inbound: 0,
          pending_conflicts: 0,
        },
        peers: {
          total_peers: 0,
          selectable_peers: 0,
          peers_in_cooldown: 0,
          peers: [],
        },
      }),
    );

    renderPage();

    await waitFor(() => {
      expect(screen.getByText("Not paired")).toBeInTheDocument();
      expect(screen.queryByText("Needs attention")).not.toBeInTheDocument();
    });
  });

  it("renders mixed route and direct-heavy traffic ratio", async () => {
    mockGetDiagnostics.mockResolvedValue(
      makeDiagnostics({
        transport: {
          last_cycle: {
            completed_at_ms: Date.now(),
            peer_node_id: "peer-1",
            status: "success",
            events_received: 8,
            events_sent: 5,
            connect_ms: 80,
            process_ms: 240,
            duration_ms: 320,
            route: "mixed",
            direct_tx_bytes: 512,
            direct_rx_bytes: 512,
            relay_tx_bytes: 1536,
            relay_rx_bytes: 1536,
            direct_rtt_ms: 42,
            relay_rtt_ms: 120,
          },
          rolling: {
            cycles: 4,
            failures: 0,
            events_received: 8,
            events_sent: 5,
            direct_tx_bytes: 512,
            direct_rx_bytes: 512,
            relay_tx_bytes: 1536,
            relay_rx_bytes: 1536,
          },
        },
        peers: {
          total_peers: 1,
          selectable_peers: 1,
          peers_in_cooldown: 0,
          peers: [
            {
              node_id: "peer-1",
              device_id_hex: "dev-1",
              last_seen_or_added_ms: Date.now(),
              selectable: true,
              consecutive_failures: 0,
              last_success_ms: Date.now(),
              sync_timeout_ms: 30000,
              last_route: "mixed",
              last_direct_bytes: 1024,
              last_relay_bytes: 3072,
              last_rtt_ms: 42,
            },
          ],
        },
      }),
    );

    renderPage();

    await waitFor(() => {
      expect(screen.getAllByText("Mixed").length).toBeGreaterThan(0);
      expect(screen.getByText("25%")).toBeInTheDocument();
      expect(
        screen.getByText(/1.0 KB direct \/ 4.0 KB total/),
      ).toBeInTheDocument();
    });
  });

  it("renders never-synced devices as not measured", async () => {
    mockListDevices.mockResolvedValue([
      {
        device_id: "dev-1",
        public_key: "peer-1",
        friendly_name: "Phone",
        label: "android",
        added_at_ms: Date.now(),
        last_seen_ms: Date.now(),
        is_self: false,
        connectivity_state: "not_connected",
        session_path: null,
        session_rtt_ms: null,
      },
    ]);
    mockGetDiagnostics.mockResolvedValue(
      makeDiagnostics({
        transport: {
          rolling: {
            cycles: 0,
            failures: 0,
            events_received: 0,
            events_sent: 0,
            direct_tx_bytes: 0,
            direct_rx_bytes: 0,
            relay_tx_bytes: 0,
            relay_rx_bytes: 0,
          },
          last_cycle: undefined,
        },
        peers: {
          total_peers: 0,
          selectable_peers: 0,
          peers_in_cooldown: 0,
          peers: [],
        },
      }),
    );

    renderPage();

    await waitFor(() => {
      expect(screen.getAllByText("Not measured").length).toBeGreaterThan(0);
      expect(screen.getByText("Never synced")).toBeInTheDocument();
      expect(
        screen.getByText("actual path not measured yet"),
      ).toBeInTheDocument();
    });
  });

  it("shows sync enabled from config while runtime status is still loading", async () => {
    let resolveStatus!: (status: {
      enabled: boolean;
      paired: boolean;
      peer_count: number;
    }) => void;
    mockGetStatus.mockReturnValue(
      new Promise((resolve) => {
        resolveStatus = resolve;
      }),
    );

    renderPage();

    await waitFor(() => {
      expect(screen.getByRole("switch")).toBeChecked();
    });
    expect(screen.getByText("Sync Power Policy")).toBeInTheDocument();

    resolveStatus({ enabled: true, paired: true, peer_count: 2 });
  });

  it("hides sections 2-5 when sync disabled (shows overlay)", async () => {
    mockGetConfig.mockResolvedValue({ enabled: false });
    mockGetStatus.mockResolvedValue({
      enabled: false,
      paired: false,
      peer_count: 0,
    });

    renderPage();

    await waitFor(() => {
      expect(screen.getByRole("switch")).toBeInTheDocument();
    });

    await waitFor(() => {
      expect(screen.getByTestId("disabled-overlay")).toBeInTheDocument();
    });

    expect(screen.getByText("Disabled")).toBeInTheDocument();
    expect(screen.queryByText("Sync Power Policy")).not.toBeInTheDocument();
    expect(screen.queryByText("Devices")).not.toBeInTheDocument();
  });

  it("keeps gated sections hidden until status reports sync enabled", async () => {
    mockGetConfig.mockResolvedValue({ enabled: false });
    mockGetStatus
      .mockResolvedValueOnce({ enabled: false, paired: false, peer_count: 0 })
      .mockResolvedValueOnce({ enabled: false, paired: false, peer_count: 0 })
      .mockResolvedValue({ enabled: true, paired: true, peer_count: 1 });

    renderPage();

    await waitFor(() => {
      expect(screen.getByTestId("disabled-overlay")).toBeInTheDocument();
    });

    await vi.advanceTimersByTimeAsync(3500);

    expect(screen.queryByText("Sync Power Policy")).not.toBeInTheDocument();
  });

  it("keeps the switch on from cached config before config refresh returns", async () => {
    window.localStorage.setItem("rua.deviceSync.enabled", "true");
    mockGetConfig.mockReturnValue(new Promise(() => {}));
    mockGetStatus.mockReturnValue(new Promise(() => {}));

    renderPage();

    expect(screen.getByRole("switch")).toBeChecked();
    expect(screen.queryByTestId("disabled-overlay")).not.toBeInTheDocument();
  });

  it("renders cached enabled page content without blocking loading masks", () => {
    window.localStorage.setItem("rua.deviceSync.enabled", "true");
    mockGetConfig.mockReturnValue(new Promise(() => {}));
    mockGetStatus.mockReturnValue(new Promise(() => {}));
    mockGetPowerPolicy.mockReturnValue(new Promise(() => {}));
    mockListDevices.mockReturnValue(new Promise(() => {}));
    mockGetRecent.mockReturnValue(new Promise(() => {}));
    mockGetConflictPolicy.mockReturnValue(new Promise(() => {}));

    const { container } = renderPage();

    expect(screen.getByRole("switch")).toBeChecked();
    expect(screen.getByText("Sync Power Policy")).toBeInTheDocument();
    expect(screen.getByText("Devices")).toBeInTheDocument();
    expect(screen.getByText("Conflict Policy")).toBeInTheDocument();
    expect(screen.getByText("Sync Conflicts")).toBeInTheDocument();
    expect(container.querySelector(".rua-loading-mask")).toBeNull();
  });

  it("power policy section renders after sync is enabled", async () => {
    renderPage();

    await waitFor(() => {
      expect(screen.getByText("Sync Power Policy")).toBeInTheDocument();
    });

    expect(await screen.findByText(/mobile-apk/)).toBeInTheDocument();
  });

  it("devices table renders device list", async () => {
    renderPage();

    await waitFor(() => {
      expect(screen.getByText("Phone")).toBeInTheDocument();
    });

    expect(screen.getByText(/android/)).toBeInTheDocument();
    expect(screen.getByText("Connected")).toBeInTheDocument();
  });

  it("allows renaming the local device without showing remove", async () => {
    mockListDevices.mockResolvedValue([
      {
        device_id: "self",
        public_key: "peer-1",
        friendly_name: "This Mac",
        label: "desktop",
        added_at_ms: Date.now(),
        last_seen_ms: Date.now(),
        is_self: true,
        connectivity_state: "connected",
      },
    ]);
    mockRenameDevice.mockResolvedValue({ renamed: true });

    renderPage();

    const edit = await screen.findByTestId("edit-device-self");
    expect(screen.queryByTestId("remove-device-self")).not.toBeInTheDocument();
    expect(screen.queryByText("Connected")).not.toBeInTheDocument();

    fireEvent.click(edit);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(mockRenameDevice).toHaveBeenCalledWith("self", "This Mac");
    });
  });

  it("devices section has Add Device button", async () => {
    renderPage();

    await waitFor(() => {
      expect(
        screen.getByRole("button", { name: "Add Device" }),
      ).toBeInTheDocument();
    });
  });

  it("shows finalizing state when pairing PIN is consumed before final commit", async () => {
    mockPairingStatus.mockResolvedValue({ status: "consumed" });

    renderPage();

    fireEvent.click(await screen.findByRole("button", { name: "Add Device" }));

    expect(await screen.findByText("12345678")).toBeInTheDocument();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(2100);
    });

    await waitFor(() => {
      expect(mockPairingStatus).toHaveBeenCalled();
    });

    expect(
      await screen.findByText("Device connected. Finishing setup..."),
    ).toBeInTheDocument();
    expect(screen.queryByText("12345678")).not.toBeInTheDocument();
  });

  it("conflicts table shows 24h hint", async () => {
    renderPage();

    await waitFor(() => {
      expect(
        screen.getByText("Showing conflicts from the last 24 hours."),
      ).toBeInTheDocument();
    });
  });
});
