import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { act, renderHook } from "@testing-library/react";
import { notifyMultiDeviceFeaturesChanged } from "./useMultiDeviceFeatureAvailability";

vi.mock("react-router-dom", () => ({
  useNavigate: () => vi.fn(),
}));

const mockToast = {
  warning: vi.fn(),
};
vi.mock("@/ui", () => ({
  toast: mockToast,
}));

const mockGetRecent = vi.fn();
const mockGetStatus = vi.fn();
vi.mock("../api/modules/sync", () => ({
  conflictsApi: {
    getRecent: (...args: unknown[]) => mockGetRecent(...args),
  },
  syncApi: {
    getStatus: (...args: unknown[]) => mockGetStatus(...args),
  },
}));

describe("useSyncConflictToast", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.clearAllMocks();
    mockGetStatus.mockResolvedValue({ enabled: true });
    localStorage.clear();
    Object.defineProperty(document, "hidden", {
      configurable: true,
      value: false,
    });
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("keeps one polling lifecycle across unrelated renders", async () => {
    mockGetRecent.mockResolvedValue({ conflicts: [], count: 0 });
    const { useSyncConflictToast } = await import("./useSyncConflictToast");
    const { rerender, unmount } = renderHook(() => useSyncConflictToast());
    await vi.waitFor(() => expect(mockGetRecent).toHaveBeenCalledTimes(1));
    rerender();
    await act(async () => {});
    expect(mockGetStatus).toHaveBeenCalledTimes(1);
    unmount();
  });

  it("suspends hidden polling and checks once on return without overlapping a slow request", async () => {
    mockGetRecent.mockResolvedValue({ conflicts: [], count: 0 });
    const { useSyncConflictToast } = await import("./useSyncConflictToast");
    const { unmount } = renderHook(() => useSyncConflictToast());
    try {
      await vi.waitFor(() => expect(mockGetRecent).toHaveBeenCalledTimes(1));
      Object.defineProperty(document, "hidden", {
        configurable: true,
        value: true,
      });
      act(() => document.dispatchEvent(new Event("visibilitychange")));
      await act(async () => {
        await vi.advanceTimersByTimeAsync(90_000);
      });
      expect(mockGetRecent).toHaveBeenCalledTimes(1);
      mockGetRecent.mockReturnValue(new Promise(() => {}));
      Object.defineProperty(document, "hidden", {
        configurable: true,
        value: false,
      });
      act(() => document.dispatchEvent(new Event("visibilitychange")));
      await act(async () => {
        await vi.advanceTimersByTimeAsync(90_000);
      });
      expect(mockGetRecent).toHaveBeenCalledTimes(2);
    } finally {
      unmount();
    }
  });

  it("starts and stops conflict checks when sync settings change", async () => {
    mockGetStatus.mockResolvedValue({ enabled: false });
    mockGetRecent.mockResolvedValue({ conflicts: [], count: 0 });
    const { useSyncConflictToast } = await import("./useSyncConflictToast");
    const { unmount } = renderHook(() => useSyncConflictToast());
    try {
      await vi.waitFor(() => expect(mockGetStatus).toHaveBeenCalledTimes(1));
      expect(mockGetRecent).not.toHaveBeenCalled();
      mockGetStatus.mockResolvedValue({ enabled: true });
      await act(async () => notifyMultiDeviceFeaturesChanged());
      expect(mockGetRecent).toHaveBeenCalledTimes(1);
      mockGetStatus.mockResolvedValue({ enabled: false });
      await act(async () => notifyMultiDeviceFeaturesChanged());
      await act(async () => {
        await vi.advanceTimersByTimeAsync(90_000);
      });
      expect(mockGetRecent).toHaveBeenCalledTimes(1);
    } finally {
      unmount();
    }
  });

  it("shows notification.warning when new conflicts found", async () => {
    mockGetRecent.mockResolvedValue({
      conflicts: [
        {
          id: 1,
          op_kind: "chat_rename",
          target_id: "chat:abc",
          before_value: "old",
          after_value: "new",
          originating_device: "dev1",
          hlc: "0001",
          observed_at: Date.now(),
        },
        {
          id: 2,
          op_kind: "chat_delete",
          target_id: "chat:def",
          before_value: "",
          after_value: "[deleted]",
          originating_device: "dev2",
          hlc: "0002",
          observed_at: Date.now(),
        },
      ],
      count: 2,
    });

    const { useSyncConflictToast } = await import("./useSyncConflictToast");
    renderHook(() => useSyncConflictToast());

    await vi.waitFor(() => {
      expect(mockGetRecent).toHaveBeenCalled();
    });
    await vi.waitFor(() => {
      expect(mockToast.warning).toHaveBeenCalledTimes(1);
    });
    expect(mockToast.warning).toHaveBeenCalledWith(
      "2 条改动因冲突被合并 — 点击查看详情",
    );
  });

  it("does not poll conflicts when sync is disabled", async () => {
    mockGetStatus.mockResolvedValueOnce({ enabled: false });

    const { useSyncConflictToast } = await import("./useSyncConflictToast");
    renderHook(() => useSyncConflictToast());

    await vi.waitFor(() => {
      expect(mockGetStatus).toHaveBeenCalledTimes(1);
    });
    expect(mockGetRecent).not.toHaveBeenCalled();
    expect(mockToast.warning).not.toHaveBeenCalled();
  });

  it("does not re-toast when last_seen_id covers all conflicts", async () => {
    localStorage.setItem("sync_conflict_last_seen_id", "5");
    mockGetRecent.mockResolvedValue({
      conflicts: [
        {
          id: 3,
          op_kind: "chat_rename",
          target_id: "chat:abc",
          before_value: "old",
          after_value: "new",
          originating_device: "dev1",
          hlc: "0001",
          observed_at: Date.now(),
        },
      ],
      count: 1,
    });

    const { useSyncConflictToast } = await import("./useSyncConflictToast");
    renderHook(() => useSyncConflictToast());

    await vi.waitFor(() => {
      expect(mockGetRecent).toHaveBeenCalled();
    });
    expect(mockToast.warning).not.toHaveBeenCalled();
  });

  it("does not toast when conflicts array is empty (append-only ops filtered by backend)", async () => {
    mockGetRecent.mockResolvedValue({
      conflicts: [],
      count: 0,
    });

    const { useSyncConflictToast } = await import("./useSyncConflictToast");
    renderHook(() => useSyncConflictToast());

    await vi.waitFor(() => {
      expect(mockGetRecent).toHaveBeenCalled();
    });
    expect(mockToast.warning).not.toHaveBeenCalled();
  });

  it("updates localStorage last_seen_id after toasting", async () => {
    mockGetRecent.mockResolvedValue({
      conflicts: [
        {
          id: 10,
          op_kind: "agent_config_set",
          target_id: "agent_config:key",
          before_value: "a",
          after_value: "b",
          originating_device: "dev1",
          hlc: "000a",
          observed_at: Date.now(),
        },
      ],
      count: 1,
    });

    const { useSyncConflictToast } = await import("./useSyncConflictToast");
    renderHook(() => useSyncConflictToast());

    await vi.waitFor(() => {
      expect(mockToast.warning).toHaveBeenCalled();
    });
    expect(localStorage.getItem("sync_conflict_last_seen_id")).toBe("10");
  });
});
