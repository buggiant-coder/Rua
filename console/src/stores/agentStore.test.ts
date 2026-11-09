import { beforeEach, describe, expect, it, vi } from "vitest";
import { useAgentStore } from "./agentStore";
import type { AgentSummary } from "../api/types/agents";

const { listAgentsMock } = vi.hoisted(() => ({
  listAgentsMock: vi.fn(),
}));

vi.mock("../api/modules/agents", () => ({
  agentsApi: {
    listAgents: listAgentsMock,
  },
}));

const mockAgent = (id: string, name = `Agent ${id}`): AgentSummary =>
  ({
    id,
    name,
    description: `${name} description`,
    workspace_dir: `/tmp/${id}`,
    enabled: true,
  }) as AgentSummary;

function resetStore() {
  sessionStorage.clear();
  useAgentStore.setState({
    selectedAgent: "default",
    agents: [],
    catalogLoaded: false,
    catalogLoading: false,
    catalogError: null,
  });
}

describe("agentStore", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    resetStore();
  });

  it("starts with the Rust default agent and no loaded agent summaries", () => {
    expect(useAgentStore.getState().selectedAgent).toBe("default");
    expect(useAgentStore.getState().agents).toEqual([]);
    expect(useAgentStore.getState().catalogLoaded).toBe(false);
  });

  it("loads the shared catalog without changing the normal Chat selection", async () => {
    const agents = [mockAgent("project-agent")];
    listAgentsMock.mockResolvedValueOnce({ agents });
    useAgentStore.getState().setSelectedAgent("normal-chat-agent");

    await useAgentStore.getState().ensureCatalogLoaded();

    expect(useAgentStore.getState().agents).toEqual(agents);
    expect(useAgentStore.getState().catalogLoaded).toBe(true);
    expect(useAgentStore.getState().selectedAgent).toBe("normal-chat-agent");
  });

  it("deduplicates concurrent catalog requests and reuses loaded data", async () => {
    const agents = [mockAgent("agent-1")];
    let resolveRequest!: (value: { agents: AgentSummary[] }) => void;
    listAgentsMock.mockReturnValueOnce(
      new Promise((resolve) => {
        resolveRequest = resolve;
      }),
    );

    const first = useAgentStore.getState().ensureCatalogLoaded();
    const second = useAgentStore.getState().ensureCatalogLoaded();
    expect(listAgentsMock).toHaveBeenCalledTimes(1);

    resolveRequest({ agents });
    await expect(Promise.all([first, second])).resolves.toEqual([
      agents,
      agents,
    ]);

    await expect(
      useAgentStore.getState().ensureCatalogLoaded(),
    ).resolves.toEqual(agents);
    expect(listAgentsMock).toHaveBeenCalledTimes(1);
  });

  it("updates the selected agent and persists only the selected id", () => {
    useAgentStore.getState().setAgents([mockAgent("agent-1")]);
    useAgentStore.getState().setSelectedAgent("agent-1");

    expect(useAgentStore.getState().selectedAgent).toBe("agent-1");
    expect(JSON.parse(sessionStorage.getItem("rua-agent-storage")!)).toEqual({
      state: { selectedAgent: "agent-1" },
      version: 0,
    });
  });

  it("replaces, appends, updates, and removes loaded agent summaries", () => {
    useAgentStore.getState().setAgents([mockAgent("agent-1")]);
    useAgentStore.getState().addAgent(mockAgent("agent-2"));
    useAgentStore.getState().updateAgent("agent-1", {
      name: "Renamed Agent",
    });

    expect(useAgentStore.getState().agents.map((agent) => agent.id)).toEqual([
      "agent-1",
      "agent-2",
    ]);
    expect(useAgentStore.getState().agents[0].name).toBe("Renamed Agent");

    useAgentStore.getState().removeAgent("agent-2");

    expect(useAgentStore.getState().agents.map((agent) => agent.id)).toEqual([
      "agent-1",
    ]);
  });

  it("falls back to default when the selected agent is removed", () => {
    useAgentStore.getState().setAgents([mockAgent("agent-1")]);
    useAgentStore.getState().setSelectedAgent("agent-1");

    useAgentStore.getState().removeAgent("agent-1");

    expect(useAgentStore.getState().selectedAgent).toBe("default");
    expect(useAgentStore.getState().agents).toEqual([]);
  });

  it("ignores updates and removals for unknown agent ids", () => {
    const agent = mockAgent("agent-1");
    useAgentStore.getState().setAgents([agent]);

    expect(() =>
      useAgentStore.getState().updateAgent("missing", { name: "Ghost" }),
    ).not.toThrow();
    useAgentStore.getState().removeAgent("missing");

    expect(useAgentStore.getState().agents).toEqual([agent]);
  });

  it("drops corrupt persisted selected-agent state instead of throwing", () => {
    sessionStorage.setItem("rua-agent-storage", "not-json");
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});

    const stored = useAgentStore.persist
      .getOptions()
      .storage?.getItem?.("rua-agent-storage");

    expect(stored).toBeNull();
    expect(sessionStorage.getItem("rua-agent-storage")).toBeNull();
    expect(errorSpy).toHaveBeenCalledWith(
      expect.stringContaining(
        'Failed to parse agent storage "rua-agent-storage"',
      ),
      expect.any(SyntaxError),
    );
  });
});
