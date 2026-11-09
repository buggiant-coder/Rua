import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentSummary } from "@/api/types/agents";
import { useAgents } from "./useAgents";

const {
  listAgentsMock,
  deleteAgentMock,
  toggleAgentEnabledMock,
  updateStoreAgentsMock,
  toastMock,
  tMock,
} = vi.hoisted(() => ({
  listAgentsMock: vi.fn(),
  deleteAgentMock: vi.fn(),
  toggleAgentEnabledMock: vi.fn(),
  updateStoreAgentsMock: vi.fn(),
  toastMock: {
    success: vi.fn(),
    error: vi.fn(),
  },
  tMock: (key: string) => key,
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: tMock }),
}));

vi.mock("@/api/modules/agents", () => ({
  agentsApi: {
    listAgents: listAgentsMock,
    deleteAgent: deleteAgentMock,
    toggleAgentEnabled: toggleAgentEnabledMock,
  },
}));

vi.mock("@/stores/agentStore", () => ({
  useAgentStore: () => ({ setAgents: updateStoreAgentsMock }),
}));

vi.mock("@/ui", () => ({
  toast: toastMock,
}));

function agent(overrides: Partial<AgentSummary>): AgentSummary {
  return {
    id: "default",
    name: "Default",
    description: "Default agent",
    workspace_dir: "/tmp/default",
    enabled: true,
    ...overrides,
  };
}

describe("useAgents", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    listAgentsMock.mockResolvedValue({
      agents: [
        agent({ id: "default", name: "Default" }),
        agent({
          id: "research",
          name: "Research",
          workspace_dir: "/tmp/research",
          enabled: false,
        }),
      ],
    });
  });

  it("loads Rust agent summaries and mirrors them into the agent store", async () => {
    const { result } = renderHook(() => useAgents());

    await waitFor(() => expect(result.current.loading).toBe(false));

    expect(listAgentsMock).toHaveBeenCalledTimes(1);
    expect(result.current.agents.map((item) => item.id)).toEqual([
      "default",
      "research",
    ]);
    expect(updateStoreAgentsMock).toHaveBeenCalledWith(result.current.agents);
    expect(result.current.error).toBeNull();
  });

  it("surfaces load failures without replacing store state", async () => {
    listAgentsMock.mockRejectedValueOnce(new Error("agent list failed"));

    const { result } = renderHook(() => useAgents());

    await waitFor(() =>
      expect(result.current.error?.message).toBe("agent list failed"),
    );
    expect(result.current.loading).toBe(false);
    expect(result.current.agents).toEqual([]);
    expect(updateStoreAgentsMock).not.toHaveBeenCalled();
    expect(toastMock.error).toHaveBeenCalledWith("agent.loadFailed");
  });

  it("deletes an agent through the Rust route and reloads summaries", async () => {
    deleteAgentMock.mockResolvedValueOnce({
      success: true,
      agent_id: "research",
    });
    const { result } = renderHook(() => useAgents());
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      await result.current.deleteAgent("research");
    });

    expect(deleteAgentMock).toHaveBeenCalledWith("research");
    expect(listAgentsMock).toHaveBeenCalledTimes(2);
    expect(toastMock.success).toHaveBeenCalledWith("agent.deleteSuccess");
  });

  it("toggles agent enabled state and reloads Rust summaries", async () => {
    toggleAgentEnabledMock.mockResolvedValueOnce({
      success: true,
      agent_id: "research",
      enabled: true,
    });
    const { result } = renderHook(() => useAgents());
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      await result.current.toggleAgent("research", true);
    });

    expect(toggleAgentEnabledMock).toHaveBeenCalledWith("research", true);
    expect(listAgentsMock).toHaveBeenCalledTimes(2);
    expect(toastMock.success).toHaveBeenCalledWith("agent.enableSuccess");
  });

  it("allows page-level ordering edits to update local and store state together", async () => {
    const { result } = renderHook(() => useAgents());
    await waitFor(() => expect(result.current.loading).toBe(false));
    const nextAgents = [
      agent({ id: "research", name: "Research" }),
      agent({ id: "default", name: "Default" }),
    ];

    act(() => {
      result.current.setAgents(nextAgents);
    });

    expect(result.current.agents.map((item) => item.id)).toEqual([
      "research",
      "default",
    ]);
    expect(updateStoreAgentsMock).toHaveBeenLastCalledWith(nextAgents);
  });
});
