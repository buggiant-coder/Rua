import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { describe, it, expect, vi, beforeEach } from "vitest";
import SyncNowButton from "./index";

const mockToast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn() }));
vi.mock("@/ui", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ui")>();
  return {
    ...actual,
    toast: mockToast,
  };
});

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, fallback?: string | Record<string, unknown>) => {
      if (typeof fallback === "string") return fallback;
      return key;
    },
  }),
}));

const mockTriggerSyncCycle = vi.fn();
const mockGetStatus = vi
  .fn()
  .mockResolvedValue({ enabled: true, paired: false, peer_count: 0 });
vi.mock("../../api/modules/sync", () => ({
  syncApi: {
    triggerSyncCycle: (...args: unknown[]) => mockTriggerSyncCycle(...args),
    getStatus: () => mockGetStatus(),
  },
}));

describe("SyncNowButton", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("shows success toast on successful sync", async () => {
    mockTriggerSyncCycle.mockResolvedValueOnce({
      events_received: 3,
      events_sent: 1,
      peer_node_id: "abc123",
      duration_ms: 1200,
      transport_duration_ms: 700,
      local_apply_duration_ms: 500,
      conflicts_detected: 0,
      repair: {
        scanned: 3,
        recorded: 2,
        already_applied: 1,
        errors: [],
        duration_ms: 400,
      },
    });

    render(<SyncNowButton />);
    const btn = await screen.findByRole("button");
    fireEvent.click(btn);

    await waitFor(() => {
      expect(mockToast.success).toHaveBeenCalledWith(
        expect.stringContaining("本设备已补全"),
      );
    });
  });

  it("shows not_paired toast", async () => {
    const err = new Error("not paired") as Error & { reason: string };
    err.reason = "not_paired";
    mockTriggerSyncCycle.mockRejectedValueOnce(err);

    render(<SyncNowButton />);
    const btn = await screen.findByRole("button");
    fireEvent.click(btn);

    await waitFor(() => {
      expect(mockToast.error).toHaveBeenCalledWith("未配对设备");
    });
  });

  it("shows peer_unreachable toast", async () => {
    const err = new Error("unreachable") as Error & { reason: string };
    err.reason = "peer_unreachable";
    mockTriggerSyncCycle.mockRejectedValueOnce(err);

    render(<SyncNowButton />);
    const btn = await screen.findByRole("button");
    fireEvent.click(btn);

    await waitFor(() => {
      expect(mockToast.error).toHaveBeenCalledWith("对端设备不可达");
    });
  });

  it("shows local apply failure toast", async () => {
    const err = new Error("drain failed") as Error & { reason: string };
    err.reason = "inbox_drain_failed";
    mockTriggerSyncCycle.mockRejectedValueOnce(err);

    render(<SyncNowButton />);
    const btn = await screen.findByRole("button");
    fireEvent.click(btn);

    await waitFor(() => {
      expect(mockToast.error).toHaveBeenCalledWith("本机应用同步数据失败");
    });
  });

  it("shows signature_failed toast", async () => {
    const err = new Error("sig") as Error & { reason: string };
    err.reason = "signature_failed";
    mockTriggerSyncCycle.mockRejectedValueOnce(err);

    render(<SyncNowButton />);
    const btn = await screen.findByRole("button");
    fireEvent.click(btn);

    await waitFor(() => {
      expect(mockToast.error).toHaveBeenCalledWith("签名验证失败");
    });
  });

  it("shows decrypt_failed toast", async () => {
    const err = new Error("dec") as Error & { reason: string };
    err.reason = "decrypt_failed";
    mockTriggerSyncCycle.mockRejectedValueOnce(err);

    render(<SyncNowButton />);
    const btn = await screen.findByRole("button");
    fireEvent.click(btn);

    await waitFor(() => {
      expect(mockToast.error).toHaveBeenCalledWith("解密失败");
    });
  });

  it("shows timeout toast", async () => {
    const err = new Error("timeout") as Error & { reason: string };
    err.reason = "timeout";
    mockTriggerSyncCycle.mockRejectedValueOnce(err);

    render(<SyncNowButton />);
    const btn = await screen.findByRole("button");
    fireEvent.click(btn);

    await waitFor(() => {
      expect(mockToast.error).toHaveBeenCalledWith("同步超时");
    });
  });
});
