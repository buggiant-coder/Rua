import { useState, useEffect, useCallback } from "react";
import api from "../../../api";
import type { ProviderInfo, ActiveModelsInfo } from "../../../api/types";
import { useAgentStore } from "../../../stores/agentStore";

export interface ProvidersInitialData {
  providers: ProviderInfo[];
  activeModels: ActiveModelsInfo | null;
}

export function useProviders(initialData?: ProvidersInitialData) {
  const initialProviders = initialData?.providers;
  const initialActiveModels = initialData?.activeModels;
  const [providers, setProviders] = useState<ProviderInfo[]>(
    initialProviders ?? [],
  );
  const [activeModels, setActiveModels] = useState<ActiveModelsInfo | null>(
    initialActiveModels ?? null,
  );
  const [loading, setLoading] = useState(initialData === undefined);
  const [error, setError] = useState<string | null>(null);
  const { selectedAgent } = useAgentStore();

  const fetchAll = useCallback(async (showLoading = true) => {
    if (showLoading) {
      setLoading(true);
    }
    setError(null);
    try {
      const [provData, activeData] = await Promise.all([
        api.listProviders(),
        api.getActiveModels({ scope: "global" }),
      ]);
      if (!Array.isArray(provData)) {
        throw new Error(
          "Unexpected API response. Is VITE_API_BASE_URL configured correctly?",
        );
      }
      setProviders(provData);
      if (activeData) setActiveModels(activeData);
    } catch (err) {
      const msg =
        err instanceof Error ? err.message : "Failed to load provider data";
      console.error("Failed to load providers:", err);
      setError(msg);
    } finally {
      if (showLoading) {
        setLoading(false);
      }
    }
  }, []);

  useEffect(() => {
    if (initialProviders) {
      setProviders(initialProviders);
      setActiveModels(initialActiveModels ?? null);
      setError(null);
      setLoading(false);
      return;
    }
    void fetchAll();
  }, [fetchAll, initialActiveModels, initialProviders, selectedAgent]);

  return {
    providers,
    activeModels,
    loading,
    error,
    fetchAll,
  };
}
