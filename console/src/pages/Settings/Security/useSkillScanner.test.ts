import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type {
  BlockedSkillRecord,
  SkillScannerConfig,
} from "../../../api/modules/security";
import { useSkillScanner } from "./useSkillScanner";

const {
  getSkillScannerMock,
  getBlockedHistoryMock,
  updateSkillScannerMock,
  addToWhitelistMock,
  removeFromWhitelistMock,
  removeBlockedEntryMock,
  clearBlockedHistoryMock,
} = vi.hoisted(() => ({
  getSkillScannerMock: vi.fn(),
  getBlockedHistoryMock: vi.fn(),
  updateSkillScannerMock: vi.fn(),
  addToWhitelistMock: vi.fn(),
  removeFromWhitelistMock: vi.fn(),
  removeBlockedEntryMock: vi.fn(),
  clearBlockedHistoryMock: vi.fn(),
}));

vi.mock("../../../api", () => ({
  default: {
    getSkillScanner: getSkillScannerMock,
    getBlockedHistory: getBlockedHistoryMock,
    updateSkillScanner: updateSkillScannerMock,
    addToWhitelist: addToWhitelistMock,
    removeFromWhitelist: removeFromWhitelistMock,
    removeBlockedEntry: removeBlockedEntryMock,
    clearBlockedHistory: clearBlockedHistoryMock,
  },
}));

const scannerConfig: SkillScannerConfig = {
  mode: "block",
  timeout: 30,
  whitelist: [
    {
      skill_name: "trusted",
      content_hash: "abc",
      added_at: "2026-06-01T00:00:00.000Z",
    },
  ],
};

const blockedHistory: BlockedSkillRecord[] = [
  {
    skill_name: "danger",
    blocked_at: "2026-06-01T00:00:00.000Z",
    max_severity: "high",
    findings: [],
    content_hash: "bad",
    action: "blocked",
    source: "workspace",
  },
];

describe("useSkillScanner", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    getSkillScannerMock.mockResolvedValue(scannerConfig);
    getBlockedHistoryMock.mockResolvedValue(blockedHistory);
    updateSkillScannerMock.mockResolvedValue({
      ...scannerConfig,
      mode: "warn",
    });
    addToWhitelistMock.mockResolvedValue({ whitelisted: true });
    removeFromWhitelistMock.mockResolvedValue({ removed: true });
    removeBlockedEntryMock.mockResolvedValue({ removed: true });
    clearBlockedHistoryMock.mockResolvedValue({ cleared: true });
  });

  it("loads scanner config and blocked history from Rust security routes", async () => {
    const { result } = renderHook(() => useSkillScanner());

    await waitFor(() => expect(result.current.loading).toBe(false));

    expect(getSkillScannerMock).toHaveBeenCalledTimes(1);
    expect(getBlockedHistoryMock).toHaveBeenCalledTimes(1);
    expect(result.current.config).toEqual(scannerConfig);
    expect(result.current.blockedHistory).toEqual(blockedHistory);
    expect(result.current.whitelist).toEqual(scannerConfig.whitelist);
    expect(result.current.error).toBeNull();
  });

  it("surfaces load failures without mutating scanner state", async () => {
    getSkillScannerMock.mockRejectedValueOnce(
      new Error("security unavailable"),
    );

    const { result } = renderHook(() => useSkillScanner());

    await waitFor(() =>
      expect(result.current.error).toBe("security unavailable"),
    );
    expect(result.current.config).toBeNull();
    expect(result.current.blockedHistory).toEqual([]);
    expect(result.current.loading).toBe(false);
  });

  it("updates scanner config by merging with the currently loaded config", async () => {
    const { result } = renderHook(() => useSkillScanner());
    await waitFor(() => expect(result.current.loading).toBe(false));

    let updated = false;
    await act(async () => {
      updated = Boolean(await result.current.updateConfig({ mode: "warn" }));
    });

    expect(updated).toBe(true);
    expect(updateSkillScannerMock).toHaveBeenCalledWith({
      ...scannerConfig,
      mode: "warn",
    });
    expect(result.current.config?.mode).toBe("warn");
  });

  it("refreshes scanner state after whitelist and history mutations", async () => {
    const { result } = renderHook(() => useSkillScanner());
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      await result.current.addToWhitelist("trusted", "abc");
      await result.current.removeFromWhitelist("trusted");
      await result.current.removeBlockedEntry(0);
    });

    expect(addToWhitelistMock).toHaveBeenCalledWith("trusted", "abc");
    expect(removeFromWhitelistMock).toHaveBeenCalledWith("trusted");
    expect(removeBlockedEntryMock).toHaveBeenCalledWith(0);
    expect(getSkillScannerMock).toHaveBeenCalledTimes(4);
    expect(getBlockedHistoryMock).toHaveBeenCalledTimes(4);
  });

  it("clears blocked history locally after the Rust clear route succeeds", async () => {
    const { result } = renderHook(() => useSkillScanner());
    await waitFor(() => expect(result.current.loading).toBe(false));

    let cleared = false;
    await act(async () => {
      cleared = Boolean(await result.current.clearBlockedHistory());
    });

    expect(cleared).toBe(true);
    expect(clearBlockedHistoryMock).toHaveBeenCalledTimes(1);
    expect(result.current.blockedHistory).toEqual([]);
  });
});
