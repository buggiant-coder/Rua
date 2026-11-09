import { useState, useEffect } from "react";
import { useTranslation } from "react-i18next";
import { agentsApi } from "@/api/modules/agents";
import type { AgentSummary } from "@/api/types/agents";
import { useAgentStore } from "@/stores/agentStore";
import { toast } from "@/ui";

interface UseAgentsReturn {
  agents: AgentSummary[];
  loading: boolean;
  error: Error | null;
  loadAgents: () => Promise<void>;
  deleteAgent: (agentId: string) => Promise<void>;
  toggleAgent: (agentId: string, enabled: boolean) => Promise<void>;
  setAgents: (agents: AgentSummary[]) => void;
}

export interface AgentsInitialData {
  agents: AgentSummary[];
}

export function useAgents(initialData?: AgentsInitialData): UseAgentsReturn {
  const { t } = useTranslation();
  const [agents, setAgents] = useState<AgentSummary[]>(
    initialData?.agents ?? [],
  );
  const [loading, setLoading] = useState(initialData === undefined);
  const [error, setError] = useState<Error | null>(null);
  const { setAgents: updateStoreAgents } = useAgentStore();

  const setAgentsState = (nextAgents: AgentSummary[]) => {
    setAgents(nextAgents);
    updateStoreAgents(nextAgents);
  };

  const loadAgents = async () => {
    setLoading(true);
    setError(null);
    try {
      const data = await agentsApi.listAgents();
      setAgentsState(data.agents);
    } catch (err) {
      console.error("Failed to load agents:", err);
      const errorMsg =
        err instanceof Error ? err : new Error(t("agent.loadFailed"));
      setError(errorMsg);
      toast.error(t("agent.loadFailed"));
    } finally {
      setLoading(false);
    }
  };

  const deleteAgent = async (agentId: string) => {
    try {
      await agentsApi.deleteAgent(agentId);
      toast.success(t("agent.deleteSuccess"));
      await loadAgents();
    } catch (err: unknown) {
      toast.error(err instanceof Error ? err.message : t("agent.deleteFailed"));
      throw err;
    }
  };

  const toggleAgent = async (agentId: string, enabled: boolean) => {
    try {
      await agentsApi.toggleAgentEnabled(agentId, enabled);
      const successMsg = enabled
        ? t("agent.enableSuccess")
        : t("agent.disableSuccess");
      toast.success(successMsg);
      await loadAgents();
    } catch (err: unknown) {
      toast.error(err instanceof Error ? err.message : t("agent.toggleFailed"));
      throw err;
    }
  };

  useEffect(() => {
    if (initialData) {
      setAgentsState(initialData.agents);
      setLoading(false);
      return;
    }
    void loadAgents();

    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [initialData]);

  return {
    agents,
    loading,
    error,
    loadAgents,
    deleteAgent,
    toggleAgent,
    setAgents: setAgentsState,
  };
}
