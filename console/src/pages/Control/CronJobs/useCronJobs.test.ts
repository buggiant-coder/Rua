import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { CronJobSpecOutput } from "../../../api/types";
import { useCronJobs } from "./useCronJobs";

const {
  selectedAgentRef,
  listCronJobsMock,
  createCronJobMock,
  replaceCronJobMock,
  deleteCronJobMock,
  triggerCronJobMock,
  showErrorModalMock,
  toastMock,
  tMock,
} = vi.hoisted(() => ({
  selectedAgentRef: { current: "default" as string | null },
  listCronJobsMock: vi.fn(),
  createCronJobMock: vi.fn(),
  replaceCronJobMock: vi.fn(),
  deleteCronJobMock: vi.fn(),
  triggerCronJobMock: vi.fn(),
  showErrorModalMock: vi.fn(),
  toastMock: {
    success: vi.fn(),
    error: vi.fn(),
  },
  tMock: (key: string) => key,
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: tMock }),
}));

vi.mock("../../../stores/agentStore", () => ({
  useAgentStore: () => ({ selectedAgent: selectedAgentRef.current }),
}));

vi.mock("../../../api", () => ({
  default: {
    listCronJobs: listCronJobsMock,
    createCronJob: createCronJobMock,
    replaceCronJob: replaceCronJobMock,
    deleteCronJob: deleteCronJobMock,
    triggerCronJob: triggerCronJobMock,
  },
}));

vi.mock("../../../utils/error", () => ({
  showErrorModal: showErrorModalMock,
}));

vi.mock("@/ui", () => ({
  toast: toastMock,
  Modal: {},
}));

function cronJob(overrides: Partial<CronJobSpecOutput>): CronJobSpecOutput {
  return {
    id: "daily-cleanup",
    name: "Daily cleanup",
    enabled: true,
    schedule: { type: "cron", cron: "0 9 * * *", timezone: "Asia/Shanghai" },
    task_type: "text",
    text: "clean workspace",
    request: { input: "clean workspace" },
    dispatch: {
      type: "channel",
      target: { user_id: "console", session_id: "session-a" },
    },
    runtime: { timeout_seconds: 60 },
    meta: { created_at: "2026-06-01T00:00:00.000Z" },
    ...overrides,
  };
}

describe("useCronJobs", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    selectedAgentRef.current = "agent-a";
    listCronJobsMock.mockResolvedValue([
      cronJob({ id: "daily-cleanup", name: "Daily cleanup" }),
      cronJob({
        id: "weekly-report",
        name: "Weekly report",
        enabled: false,
        schedule: { type: "cron", cron: "0 18 * * 5" },
        meta: { created_at: "2026-06-10T00:00:00.000Z" },
      }),
    ]);
  });

  it("loads cron jobs through the selected Rust agent scope", async () => {
    const { result } = renderHook(() => useCronJobs());

    await waitFor(() => expect(result.current.loading).toBe(false));

    expect(listCronJobsMock).toHaveBeenCalledWith("agent-a");
    expect(result.current.jobs.map((item) => item.id)).toEqual([
      "daily-cleanup",
      "weekly-report",
    ]);
  });

  it("creates jobs under the selected agent and prepends the returned job", async () => {
    const created = cronJob({ id: "new-job", name: "New job" });
    createCronJobMock.mockResolvedValueOnce(created);
    const { result } = renderHook(() => useCronJobs());
    await waitFor(() => expect(result.current.loading).toBe(false));

    let ok = false;
    await act(async () => {
      ok = await result.current.createJob(created);
    });

    expect(ok).toBe(true);
    expect(createCronJobMock).toHaveBeenCalledWith(created, "agent-a");
    expect(result.current.jobs[0].id).toBe("new-job");
    expect(toastMock.success).toHaveBeenCalledWith("cronJobs.createSuccess");
  });

  it("reverts an optimistic update when replace fails", async () => {
    replaceCronJobMock.mockRejectedValueOnce(new Error("invalid cron"));
    const { result } = renderHook(() => useCronJobs());
    await waitFor(() => expect(result.current.loading).toBe(false));

    let ok = true;
    await act(async () => {
      ok = await result.current.updateJob(
        "daily-cleanup",
        cronJob({ id: "daily-cleanup", name: "Changed name" }),
      );
    });

    expect(ok).toBe(false);
    expect(
      result.current.jobs.find((item) => item.id === "daily-cleanup")?.name,
    ).toBe("Daily cleanup");
    expect(showErrorModalMock).toHaveBeenCalledWith(
      {},
      expect.objectContaining({ title: "cronJobs.saveFailedTitle" }),
    );
  });

  it("reverts a failed delete and runs jobs through Rust scoped actions", async () => {
    deleteCronJobMock.mockRejectedValueOnce(new Error("delete failed"));
    triggerCronJobMock.mockResolvedValueOnce(undefined);
    const { result } = renderHook(() => useCronJobs());
    await waitFor(() => expect(result.current.loading).toBe(false));

    let deleted = true;
    let executed = false;
    await act(async () => {
      deleted = await result.current.deleteJob("daily-cleanup");
      executed = await result.current.executeNow("weekly-report");
    });

    expect(deleted).toBe(false);
    expect(executed).toBe(true);
    expect(deleteCronJobMock).toHaveBeenCalledWith("daily-cleanup", "agent-a");
    expect(triggerCronJobMock).toHaveBeenCalledWith("weekly-report", "agent-a");
    expect(result.current.jobs.map((item) => item.id)).toContain(
      "daily-cleanup",
    );
    expect(toastMock.error).toHaveBeenCalledWith("cronJobs.deleteFailed");
    expect(toastMock.success).toHaveBeenCalledWith("cronJobs.executeSuccess");
  });

  it("filters and persists created-at sorting for the toolbar model", async () => {
    const { result } = renderHook(() => useCronJobs());
    await waitFor(() => expect(result.current.loading).toBe(false));

    act(() => {
      result.current.setSearchQuery("report");
      result.current.setSortField("created");
      result.current.setSortOrder("asc");
    });

    expect(result.current.filteredJobs.map((item) => item.id)).toEqual([
      "weekly-report",
    ]);
    expect(localStorage.getItem("rua.cronJobs.sortField")).toBe("created");
    expect(localStorage.getItem("rua.cronJobs.sortOrder")).toBe("asc");
  });
});
