import { useState, useEffect, useCallback } from "react";
import { pluginApi } from "../../../api/modules/plugin";
import type { PluginSummary } from "../../../api/types";

export interface PluginsInitialData {
  plugins: PluginSummary[];
  error: string | null;
}

export function usePlugins(initialData?: PluginsInitialData) {
  const [plugins, setPlugins] = useState<PluginSummary[]>(
    initialData?.plugins ?? [],
  );
  const [loading, setLoading] = useState(initialData === undefined);
  const [error, setError] = useState<string | null>(initialData?.error ?? null);

  const fetchPlugins = useCallback(async (showLoading = true) => {
    if (showLoading) {
      setLoading(true);
    }
    setError(null);
    try {
      const data = await pluginApi.listPlugins();
      setPlugins(Array.isArray(data) ? data : []);
    } catch (err) {
      const msg = err instanceof Error ? err.message : "Failed to load plugins";
      console.error("Failed to load plugins:", err);
      setError(msg);
    } finally {
      if (showLoading) {
        setLoading(false);
      }
    }
  }, []);

  useEffect(() => {
    if (initialData) {
      setPlugins(initialData.plugins);
      setError(initialData.error);
      setLoading(false);
      return;
    }
    void fetchPlugins();
  }, [fetchPlugins, initialData]);

  return {
    plugins,
    loading,
    error,
    refresh: fetchPlugins,
  };
}
