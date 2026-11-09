import { useState, useEffect, useCallback } from "react";
import api from "../../../api";
import type { EnvVar } from "../../../api/types";

export interface EnvVarsInitialData {
  envVars: EnvVar[];
}

export function useEnvVars(initialData?: EnvVarsInitialData) {
  const [envVars, setEnvVars] = useState<EnvVar[]>(initialData?.envVars ?? []);
  const [loading, setLoading] = useState(initialData === undefined);
  const [error, setError] = useState<string | null>(null);

  const fetchAll = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const data = await api.listEnvs();
      if (data) setEnvVars(data);
    } catch (err) {
      const msg =
        err instanceof Error
          ? err.message
          : "Failed to load environment variables";
      console.error("Failed to load env vars:", err);
      setError(msg);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (initialData) {
      setEnvVars(initialData.envVars);
      setError(null);
      setLoading(false);
      return;
    }
    void fetchAll();
  }, [fetchAll, initialData]);

  return { envVars, loading, error, fetchAll };
}
