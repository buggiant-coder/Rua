import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ACPRunnerInfo } from "../../../api/types";

const {
  listRunnersMock,
  checkRunnerMock,
  installRunnerMock,
  startRunnerInstallMock,
  getRunnerInstallTaskMock,
  createRunnerMock,
  updateRunnerMock,
  toggleRunnerMock,
  deleteRunnerMock,
  toastMock,
  tMock,
} = vi.hoisted(() => ({
  listRunnersMock: vi.fn(),
  checkRunnerMock: vi.fn(),
  installRunnerMock: vi.fn(),
  startRunnerInstallMock: vi.fn(),
  getRunnerInstallTaskMock: vi.fn(),
  createRunnerMock: vi.fn(),
  updateRunnerMock: vi.fn(),
  toggleRunnerMock: vi.fn(),
  deleteRunnerMock: vi.fn(),
  toastMock: {
    success: vi.fn(),
    error: vi.fn(),
    warning: vi.fn(),
  },
  tMock: (key: string) => key,
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: tMock }),
}));

vi.mock("../../../api/modules/acp", () => ({
  acpApi: {
    listRunners: listRunnersMock,
    checkRunner: checkRunnerMock,
    installRunner: installRunnerMock,
    startRunnerInstall: startRunnerInstallMock,
    getRunnerInstallTask: getRunnerInstallTaskMock,
    createRunner: createRunnerMock,
    updateRunner: updateRunnerMock,
    toggleRunner: toggleRunnerMock,
    deleteRunner: deleteRunnerMock,
  },
}));

vi.mock("@/ui", () => ({
  toast: toastMock,
}));

import { useACP } from "./useACP";

function runner(overrides: Partial<ACPRunnerInfo>): ACPRunnerInfo {
  return {
    key: "codex",
    enabled: true,
    command: "npx",
    args: ["-y", "@agentclientprotocol/codex-acp"],
    env: {},
    trusted: false,
    tool_parse_mode: "update_detail",
    stdio_buffer_limit_bytes: null,
    is_builtin: true,
    ...overrides,
  };
}

describe("useACP", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    listRunnersMock.mockResolvedValue([
      runner({
        key: "codex",
      }),
    ]);
  });

  it("loads configured runners without performing availability checks", async () => {
    const { result } = renderHook(() => useACP());

    await waitFor(() => expect(result.current.loading).toBe(false));

    expect(listRunnersMock).toHaveBeenCalledTimes(1);
    expect(result.current.runners.map((item) => item.key)).toEqual(["codex"]);
    expect(checkRunnerMock).not.toHaveBeenCalled();
    expect(result.current.loadError).toBeNull();
  });

  it("toggles a disabled runner without a preflight handshake", async () => {
    toggleRunnerMock.mockResolvedValueOnce(runner({ enabled: true }));
    const { result } = renderHook(() => useACP());
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      await result.current.toggleEnabled(
        runner({ key: "claude", enabled: false }),
      );
    });

    expect(checkRunnerMock).not.toHaveBeenCalled();
    expect(toggleRunnerMock).toHaveBeenCalledWith("claude");
    expect(toastMock.success).toHaveBeenCalledWith("acp.enableSuccess");
  });

  it("installs a runner, records the returned check result, and refreshes runners", async () => {
    startRunnerInstallMock.mockResolvedValueOnce({
      task_id: "task-1",
      agent_id: "agent-a",
      runner_key: "codex",
      status: "completed",
      stage: "completed",
      installed: true,
      message: "installed",
      check_result: {
        ok: true,
        status: "ok",
        message: "available",
        requires_auth: false,
      },
      created_at: 1,
      updated_at: 2,
    });
    const { result } = renderHook(() => useACP());
    await waitFor(() => expect(result.current.loading).toBe(false));

    let installed = false;
    await act(async () => {
      installed = await result.current.installRunner("codex");
    });

    expect(installed).toBe(true);
    expect(startRunnerInstallMock).toHaveBeenCalledWith("codex");
    expect(getRunnerInstallTaskMock).not.toHaveBeenCalled();
    expect(result.current.installTasks.codex).toMatchObject({
      task_id: "task-1",
      status: "completed",
      stage: "completed",
    });
    expect(result.current.installingKeys.codex).toBe(false);
    expect(toastMock.success).toHaveBeenCalledWith("acp.installSuccess");
    expect(listRunnersMock).toHaveBeenCalledTimes(2);
  });

  it("mutations call Rust ACP APIs, refresh runners, and return success flags", async () => {
    createRunnerMock.mockResolvedValueOnce(runner({ key: "custom" }));
    updateRunnerMock.mockResolvedValueOnce(runner({ key: "custom" }));
    deleteRunnerMock.mockResolvedValueOnce({ message: "deleted" });
    const { result } = renderHook(() => useACP());
    await waitFor(() => expect(result.current.loading).toBe(false));

    let created = false;
    let updated = false;
    await act(async () => {
      created = await result.current.createRunner({
        key: "custom",
        command: "custom-acp",
      });
      updated = await result.current.updateRunner("custom", {
        enabled: false,
      });
      await result.current.deleteRunner(runner({ key: "custom" }));
    });

    expect(created).toBe(true);
    expect(updated).toBe(true);
    expect(createRunnerMock).toHaveBeenCalledWith({
      key: "custom",
      command: "custom-acp",
    });
    expect(updateRunnerMock).toHaveBeenCalledWith("custom", {
      enabled: false,
    });
    expect(deleteRunnerMock).toHaveBeenCalledWith("custom");
    expect(listRunnersMock).toHaveBeenCalledTimes(4);
  });
});
