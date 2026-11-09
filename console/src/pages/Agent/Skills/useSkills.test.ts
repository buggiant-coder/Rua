import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SkillSpec } from "../../../api/types";
import { __resetSkillListCacheForTests, useSkills } from "./useSkills";
import api from "../../../api";

const {
  selectedAgentRef,
  listAgentSkillsMock,
  refreshAgentSkillsMock,
  createAgentSkillMock,
  enableAgentSkillMock,
  disableAgentSkillMock,
  deleteAgentSkillMock,
  getBlockedHistoryMock,
  getSkillScannerMock,
  checkScanWarningsMock,
  modalConfirmMock,
  toastMock,
  tMock,
} = vi.hoisted(() => ({
  selectedAgentRef: { current: "default" as string | null },
  listAgentSkillsMock: vi.fn(),
  refreshAgentSkillsMock: vi.fn(),
  createAgentSkillMock: vi.fn(),
  enableAgentSkillMock: vi.fn(),
  disableAgentSkillMock: vi.fn(),
  deleteAgentSkillMock: vi.fn(),
  getBlockedHistoryMock: vi.fn(),
  getSkillScannerMock: vi.fn(),
  checkScanWarningsMock: vi.fn(),
  modalConfirmMock: vi.fn(),
  toastMock: {
    success: vi.fn(),
    error: vi.fn(),
    warning: vi.fn(),
  },
  tMock: (key: string, params?: Record<string, unknown>) =>
    params ? `${key}:${JSON.stringify(params)}` : key,
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: tMock }),
}));

vi.mock("../../../stores/agentStore", () => ({
  useAgentStore: () => ({ selectedAgent: selectedAgentRef.current }),
}));

vi.mock("../../../api", () => ({
  default: {
    startHubSkillInstall: vi.fn(),
    getHubSkillInstallStatus: vi.fn(),
    cancelHubSkillInstall: vi.fn().mockResolvedValue({}),
    getBlockedHistory: getBlockedHistoryMock,
    getSkillScanner: getSkillScannerMock,
  },
  agentsApi: {
    listAgentSkills: listAgentSkillsMock,
    refreshAgentSkills: refreshAgentSkillsMock,
    createAgentSkill: createAgentSkillMock,
    enableAgentSkill: enableAgentSkillMock,
    disableAgentSkill: disableAgentSkillMock,
    deleteAgentSkill: deleteAgentSkillMock,
  },
}));

vi.mock("../../../utils/error", () => ({
  parseErrorDetail: (error: { detail?: Record<string, unknown> }) =>
    error?.detail,
}));

vi.mock("../../../utils/scanError", () => ({
  handleScanError: vi.fn(() => false),
  checkScanWarnings: checkScanWarningsMock,
  showScanErrorModal: vi.fn(),
}));

vi.mock("@/ui", () => ({
  Modal: {
    confirm: modalConfirmMock,
  },
  toast: toastMock,
}));

function skill(overrides: Partial<SkillSpec>): SkillSpec {
  return {
    name: "docs",
    description: "Docs skill",
    content: "# Docs",
    source: "workspace",
    enabled: true,
    config: {},
    ...overrides,
  };
}

describe("useSkills", () => {
  it("keeps installation polling and cancellation bound to the starting Agent", async () => {
    let complete!: (value: { status: string; name: string }) => void;
    vi.mocked(api.startHubSkillInstall).mockResolvedValue({
      task_id: "task-1",
      status: "pending",
    } as never);
    vi.mocked(api.getHubSkillInstallStatus).mockReturnValue(
      new Promise((resolve) => {
        complete = resolve as typeof complete;
      }),
    );
    const { result, rerender } = renderHook(() => useSkills());
    let installation!: Promise<unknown>;
    act(() => {
      installation = result.current.importFromHub(
        "https://example.test/skill.zip",
      );
    });
    await waitFor(() =>
      expect(api.getHubSkillInstallStatus).toHaveBeenCalledWith(
        "task-1",
        "agent-a",
      ),
    );
    selectedAgentRef.current = "agent-b";
    rerender();
    act(() => result.current.cancelImport());
    expect(api.cancelHubSkillInstall).toHaveBeenCalledWith("task-1", "agent-a");
    await act(async () => {
      complete({ status: "cancelled", name: "docs" });
      await installation;
    });
    await waitFor(() => expect(result.current.importing).toBe(false));
  });

  beforeEach(() => {
    vi.clearAllMocks();
    __resetSkillListCacheForTests();
    selectedAgentRef.current = "agent-a";
    listAgentSkillsMock.mockResolvedValue([
      skill({ name: "docs", enabled: true }),
      skill({ name: "review", enabled: false }),
    ]);
    refreshAgentSkillsMock.mockResolvedValue([skill({ name: "refreshed" })]);
    checkScanWarningsMock.mockResolvedValue(undefined);
    modalConfirmMock.mockImplementation(({ onOk }) => onOk?.());
  });

  it("loads agent-scoped Rust skills for the selected agent", async () => {
    const { result } = renderHook(() => useSkills());

    await waitFor(() => expect(result.current.loading).toBe(false));

    expect(listAgentSkillsMock).toHaveBeenCalledWith("agent-a");
    expect(result.current.skills.map((item) => item.name)).toEqual([
      "docs",
      "review",
    ]);
    expect(result.current.loadError).toBeNull();
  });

  it("shows cached agent skills across remounts while revalidating in the background", async () => {
    const { result, unmount } = renderHook(() => useSkills());
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(listAgentSkillsMock).toHaveBeenCalledTimes(1);

    unmount();
    const { result: remount } = renderHook(() => useSkills());

    expect(remount.current.loading).toBe(false);
    expect(remount.current.skills.map((item) => item.name)).toEqual([
      "docs",
      "review",
    ]);
    await waitFor(() => expect(listAgentSkillsMock).toHaveBeenCalledTimes(2));
  });

  it("creates a workspace skill, refreshes, and checks scanner warnings", async () => {
    createAgentSkillMock.mockResolvedValueOnce({ created: true, name: "docs" });
    const { result } = renderHook(() => useSkills());
    await waitFor(() => expect(result.current.loading).toBe(false));

    let response: Awaited<ReturnType<typeof result.current.createSkill>>;
    await act(async () => {
      response = await result.current.createSkill(
        "docs",
        "# Docs",
        { command: "read" },
        true,
      );
    });

    expect(response!).toEqual({ success: true, name: "docs" });
    expect(createAgentSkillMock).toHaveBeenCalledWith(
      "agent-a",
      "docs",
      "# Docs",
      { command: "read" },
      true,
    );
    expect(listAgentSkillsMock).toHaveBeenCalledTimes(2);
    expect(checkScanWarningsMock).toHaveBeenCalledWith(
      "docs",
      getBlockedHistoryMock,
      getSkillScannerMock,
      tMock,
    );
    expect(toastMock.success).toHaveBeenCalledWith(
      "skills.createdSuccessfully",
    );
  });

  it("returns Rust conflict details from create without showing a generic toast", async () => {
    createAgentSkillMock.mockRejectedValueOnce({
      detail: { suggested_name: "docs-2" },
    });
    const { result } = renderHook(() => useSkills());
    await waitFor(() => expect(result.current.loading).toBe(false));

    let response: Awaited<ReturnType<typeof result.current.createSkill>>;
    await act(async () => {
      response = await result.current.createSkill("docs", "# Docs");
    });

    expect(response!).toEqual({
      success: false,
      conflict: { suggested_name: "docs-2" },
    });
    expect(toastMock.error).not.toHaveBeenCalled();
  });

  it("toggles skills through Rust enable and disable routes", async () => {
    enableAgentSkillMock.mockResolvedValueOnce(undefined);
    disableAgentSkillMock.mockResolvedValueOnce(undefined);
    const { result } = renderHook(() => useSkills());
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      await result.current.toggleEnabled(
        skill({ name: "review", enabled: false }),
      );
      await result.current.toggleEnabled(
        skill({ name: "docs", enabled: true }),
      );
    });

    expect(enableAgentSkillMock).toHaveBeenCalledWith("agent-a", "review");
    expect(disableAgentSkillMock).toHaveBeenCalledWith("agent-a", "docs");
    expect(
      result.current.skills.find((item) => item.name === "review")?.enabled,
    ).toBe(true);
    expect(
      result.current.skills.find((item) => item.name === "docs")?.enabled,
    ).toBe(false);
    expect(checkScanWarningsMock).toHaveBeenCalledWith(
      "review",
      getBlockedHistoryMock,
      getSkillScannerMock,
      tMock,
    );
  });

  it("deletes confirmed skills and hard-refreshes through agent-scoped routes", async () => {
    deleteAgentSkillMock.mockResolvedValueOnce({ deleted: true });
    const { result } = renderHook(() => useSkills());
    await waitFor(() => expect(result.current.loading).toBe(false));

    let deleted = false;
    await act(async () => {
      deleted = await result.current.deleteSkill(skill({ name: "docs" }));
      await result.current.hardRefresh();
    });

    expect(deleted).toBe(true);
    expect(modalConfirmMock).toHaveBeenCalled();
    expect(deleteAgentSkillMock).toHaveBeenCalledWith("agent-a", "docs");
    expect(refreshAgentSkillsMock).toHaveBeenCalledWith("agent-a");
    expect(result.current.skills.map((item) => item.name)).toEqual([
      "refreshed",
    ]);
  });
});
