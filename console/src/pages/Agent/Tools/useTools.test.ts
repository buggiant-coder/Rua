import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ToolInfo } from "../../../api/modules/tools";
import { useTools } from "./useTools";

const {
  selectedAgentRef,
  listAgentToolsMock,
  toggleAgentToolMock,
  updateAgentToolAsyncExecutionMock,
  toastMock,
  tMock,
} = vi.hoisted(() => ({
  selectedAgentRef: { current: "default" },
  listAgentToolsMock: vi.fn(),
  toggleAgentToolMock: vi.fn(),
  updateAgentToolAsyncExecutionMock: vi.fn(),
  toastMock: {
    success: vi.fn(),
    error: vi.fn(),
    info: vi.fn(),
  },
  tMock: (key: string) => key,
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: tMock }),
}));

vi.mock("../../../stores/agentStore", () => ({
  useAgentStore: () => ({ selectedAgent: selectedAgentRef.current }),
}));

vi.mock("../../../api/modules/agents", () => ({
  agentsApi: {
    listAgentTools: listAgentToolsMock,
    toggleAgentTool: toggleAgentToolMock,
    updateAgentToolAsyncExecution: updateAgentToolAsyncExecutionMock,
  },
}));

vi.mock("@/ui", () => ({
  toast: toastMock,
}));

function tool(overrides: Partial<ToolInfo>): ToolInfo {
  return {
    name: "Bash",
    enabled: true,
    description: "Run shell commands",
    async_execution: false,
    ...overrides,
  };
}

describe("useTools", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    selectedAgentRef.current = "agent-a";
    listAgentToolsMock.mockResolvedValue([
      tool({ name: "Bash", enabled: true, async_execution: false }),
      tool({
        name: "Read",
        enabled: false,
        description: "Read files",
        async_execution: true,
      }),
    ]);
  });

  it("loads Rust-canonical tool names from the selected agent route", async () => {
    const { result } = renderHook(() => useTools());

    await waitFor(() => expect(result.current.loading).toBe(false));

    expect(listAgentToolsMock).toHaveBeenCalledWith("agent-a");
    expect(result.current.tools.map((item) => item.name)).toEqual([
      "Bash",
      "Read",
    ]);
  });

  it("toggles enabled state optimistically and applies the server result", async () => {
    toggleAgentToolMock.mockResolvedValueOnce(
      tool({ name: "Read", enabled: true, async_execution: true }),
    );
    const { result } = renderHook(() => useTools());
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      await result.current.toggleEnabled(
        tool({ name: "Read", enabled: false, async_execution: true }),
      );
    });

    expect(toggleAgentToolMock).toHaveBeenCalledWith("agent-a", "Read");
    expect(
      result.current.tools.find((item) => item.name === "Read")?.enabled,
    ).toBe(true);
    expect(toastMock.success).toHaveBeenCalledWith("tools.enableSuccess");
  });

  it("reverts enabled optimistic state when the Rust route rejects", async () => {
    toggleAgentToolMock.mockRejectedValueOnce(new Error("boom"));
    const { result } = renderHook(() => useTools());
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      await result.current.toggleEnabled(
        tool({ name: "Bash", enabled: true, async_execution: false }),
      );
    });

    expect(
      result.current.tools.find((item) => item.name === "Bash")?.enabled,
    ).toBe(true);
    expect(toastMock.error).toHaveBeenCalledWith("tools.toggleError");
  });

  it("toggles async execution through the Rust async-execution field", async () => {
    updateAgentToolAsyncExecutionMock.mockResolvedValueOnce(
      tool({ name: "Bash", enabled: true, async_execution: true }),
    );
    const { result } = renderHook(() => useTools());
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      await result.current.toggleAsyncExecution(
        tool({ name: "Bash", enabled: true, async_execution: false }),
      );
    });

    expect(updateAgentToolAsyncExecutionMock).toHaveBeenCalledWith(
      "agent-a",
      "Bash",
      true,
    );
    expect(
      result.current.tools.find((item) => item.name === "Bash")
        ?.async_execution,
    ).toBe(true);
    expect(toastMock.success).toHaveBeenCalledWith(
      "tools.asyncExecutionEnabled",
    );
  });

  it("skips capability-disabled Tools during bulk enable and preserves their restrictions", async () => {
    const blocked = tool({
      name: "phone_observe",
      enabled: false,
      capability_enabled: false,
    });
    const editable = tool({ name: "Read", enabled: false });
    listAgentToolsMock.mockResolvedValueOnce([blocked, editable]);
    toggleAgentToolMock.mockResolvedValueOnce({ ...editable, enabled: true });
    const { result } = renderHook(() => useTools());
    await waitFor(() => expect(result.current.loading).toBe(false));
    await act(async () => {
      await result.current.enableAll();
    });
    expect(toggleAgentToolMock).toHaveBeenCalledTimes(1);
    expect(toggleAgentToolMock).toHaveBeenCalledWith("agent-a", "Read");
    expect(
      result.current.tools.find((entry) => entry.name === blocked.name)
        ?.enabled,
    ).toBe(false);
  });

  it("does not claim bulk success when every disabled Tool belongs to a disabled capability", async () => {
    listAgentToolsMock.mockResolvedValueOnce([
      tool({
        name: "phone_observe",
        enabled: false,
        capability_enabled: false,
      }),
    ]);
    const { result } = renderHook(() => useTools());
    await waitFor(() => expect(result.current.loading).toBe(false));
    await act(async () => {
      await result.current.enableAll();
    });
    expect(toggleAgentToolMock).not.toHaveBeenCalled();
    expect(toastMock.success).not.toHaveBeenCalled();
    expect(toastMock.info).toHaveBeenCalledWith(
      "tools.selectionPolicy.capabilityDisabled",
    );
  });

  it.each(["enableAll", "disableAll"] as const)(
    "serializes %s mutations of one policy revision",
    async (operation) => {
      const enabled = operation === "disableAll";
      const first = tool({ name: "Bash", enabled });
      const second = tool({ name: "Read", enabled });
      listAgentToolsMock.mockResolvedValueOnce([first, second]);
      let finishFirst!: (value: ToolInfo) => void;
      toggleAgentToolMock.mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            finishFirst = resolve;
          }),
      );
      toggleAgentToolMock.mockResolvedValueOnce({
        ...second,
        enabled: !enabled,
      });
      const { result } = renderHook(() => useTools());
      await waitFor(() => expect(result.current.loading).toBe(false));
      let pending!: Promise<void>;
      act(() => {
        pending = result.current[operation]();
      });
      expect(toggleAgentToolMock).toHaveBeenCalledTimes(1);
      expect(toggleAgentToolMock).toHaveBeenCalledWith("agent-a", "Bash");
      await act(async () => {
        finishFirst({ ...first, enabled: !enabled });
        await pending;
      });
      expect(toggleAgentToolMock).toHaveBeenCalledTimes(2);
      expect(toggleAgentToolMock).toHaveBeenLastCalledWith("agent-a", "Read");
      expect(
        result.current.tools.every((entry) => entry.enabled === !enabled),
      ).toBe(true);
    },
  );

  it("batch-enables only disabled tools while preserving async execution state", async () => {
    toggleAgentToolMock.mockResolvedValueOnce(
      tool({ name: "Read", enabled: true, async_execution: false }),
    );
    const { result } = renderHook(() => useTools());
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      await result.current.enableAll();
    });

    expect(toggleAgentToolMock).toHaveBeenCalledTimes(1);
    expect(toggleAgentToolMock).toHaveBeenCalledWith("agent-a", "Read");
    expect(
      result.current.tools.find((item) => item.name === "Read")
        ?.async_execution,
    ).toBe(true);
    expect(toastMock.success).toHaveBeenCalledWith("tools.enableAllSuccess");
  });

  it("batch-disables only enabled tools while preserving async execution state", async () => {
    toggleAgentToolMock.mockResolvedValueOnce(
      tool({ name: "Bash", enabled: false, async_execution: false }),
    );
    const { result } = renderHook(() => useTools());
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      await result.current.disableAll();
    });

    expect(toggleAgentToolMock).toHaveBeenCalledTimes(1);
    expect(toggleAgentToolMock).toHaveBeenCalledWith("agent-a", "Bash");
    expect(result.current.tools.map((item) => item.enabled)).toEqual([
      false,
      false,
    ]);
    expect(
      result.current.tools.find((item) => item.name === "Read")
        ?.async_execution,
    ).toBe(true);
    expect(toastMock.success).toHaveBeenCalledWith("tools.disableAllSuccess");
  });

  it("reports no-op disable-all without calling Rust mutation routes", async () => {
    const { result } = renderHook(() => useTools());
    await waitFor(() => expect(result.current.loading).toBe(false));

    listAgentToolsMock.mockResolvedValueOnce([
      tool({ name: "Bash", enabled: false, async_execution: false }),
      tool({
        name: "Read",
        enabled: false,
        description: "Read files",
        async_execution: true,
      }),
    ]);
    await act(async () => {
      await result.current.loadTools();
    });

    await act(async () => {
      await result.current.disableAll();
    });

    expect(toggleAgentToolMock).not.toHaveBeenCalled();
    expect(toastMock.info).toHaveBeenCalledWith("tools.allDisabled");
  });

  it("reloads from Rust when batch mutation fails", async () => {
    toggleAgentToolMock.mockRejectedValueOnce(new Error("route failed"));
    listAgentToolsMock
      .mockResolvedValueOnce([
        tool({ name: "Bash", enabled: true, async_execution: false }),
        tool({
          name: "Read",
          enabled: false,
          description: "Read files",
          async_execution: true,
        }),
      ])
      .mockResolvedValueOnce([
        tool({ name: "Bash", enabled: true, async_execution: false }),
        tool({
          name: "Read",
          enabled: false,
          description: "Read files",
          async_execution: true,
        }),
      ]);
    const { result } = renderHook(() => useTools());
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      await result.current.enableAll();
    });

    expect(toastMock.error).toHaveBeenCalledWith("tools.toggleError");
    expect(listAgentToolsMock).toHaveBeenCalledTimes(2);
    expect(result.current.tools.map((item) => item.enabled)).toEqual([
      true,
      false,
    ]);
    expect(result.current.batchLoading).toBe(false);
  });
});
