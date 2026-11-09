import { useState, useEffect, useCallback } from "react";
import api from "../../../api";
import type {
  ToolGuardConfig,
  ToolGuardRule,
} from "../../../api/modules/security";

export interface MergedRule extends ToolGuardRule {
  source: "builtin" | "custom";
  disabled: boolean;
}

export interface ToolGuardInitialData {
  agentId: string;
  config: ToolGuardConfig;
  builtinRules: ToolGuardRule[];
}

export function useToolGuard(
  agentId: string | null,
  initialData?: ToolGuardInitialData,
) {
  const [config, setConfig] = useState<ToolGuardConfig | null>(
    initialData?.config ?? null,
  );
  const [builtinRules, setBuiltinRules] = useState<ToolGuardRule[]>(
    initialData?.builtinRules ?? [],
  );
  const [customRules, setCustomRules] = useState<ToolGuardRule[]>(
    initialData?.config.custom_rules ?? [],
  );
  const [disabledRules, setDisabledRules] = useState<Set<string>>(
    () => new Set(initialData?.config.disabled_rules ?? []),
  );
  const [enabled, setEnabled] = useState(initialData?.config.enabled ?? true);
  const [loading, setLoading] = useState(
    initialData === undefined && agentId !== null,
  );
  const [error, setError] = useState<string | null>(null);

  const fetchAll = useCallback(async () => {
    if (agentId === null) return;
    setLoading(true);
    setError(null);
    try {
      const [cfg, builtin] = await Promise.all([
        api.getAgentToolGuard(agentId),
        api.getAgentBuiltinRules(agentId),
      ]);
      setConfig(cfg);
      setEnabled(cfg.enabled);
      setBuiltinRules(builtin);
      setCustomRules(cfg.custom_rules ?? []);
      setDisabledRules(new Set(cfg.disabled_rules ?? []));
    } catch (err) {
      const msg =
        err instanceof Error ? err.message : "Failed to load security config";
      console.error("Failed to load tool guard:", err);
      setError(msg);
    } finally {
      setLoading(false);
    }
  }, [agentId]);

  useEffect(() => {
    if (initialData) {
      setConfig(initialData.config);
      setEnabled(initialData.config.enabled);
      setBuiltinRules(initialData.builtinRules);
      setCustomRules(initialData.config.custom_rules ?? []);
      setDisabledRules(new Set(initialData.config.disabled_rules ?? []));
      setError(null);
      setLoading(false);
      return;
    }
    void fetchAll();
  }, [fetchAll, initialData]);

  const toggleRule = useCallback(
    (ruleId: string, currentlyDisabled: boolean) => {
      setDisabledRules((prev) => {
        const next = new Set(prev);
        if (currentlyDisabled) {
          next.delete(ruleId);
        } else {
          next.add(ruleId);
        }
        return next;
      });
    },
    [],
  );

  const deleteCustomRule = useCallback((ruleId: string) => {
    setCustomRules((prev) => prev.filter((r) => r.id !== ruleId));
    setDisabledRules((prev) => {
      const next = new Set(prev);
      next.delete(ruleId);
      return next;
    });
  }, []);

  const addCustomRule = useCallback((rule: ToolGuardRule) => {
    setCustomRules((prev) => [...prev, rule]);
  }, []);

  const updateCustomRule = useCallback(
    (ruleId: string, rule: ToolGuardRule) => {
      setCustomRules((prev) => prev.map((r) => (r.id === ruleId ? rule : r)));
    },
    [],
  );

  const mergedRules: MergedRule[] = [
    ...builtinRules.map((r) => ({
      ...r,
      source: "builtin" as const,
      disabled: disabledRules.has(r.id),
    })),
    ...customRules.map((r) => ({
      ...r,
      source: "custom" as const,
      disabled: disabledRules.has(r.id),
    })),
  ];

  const buildSaveBody = useCallback((): ToolGuardConfig => {
    return {
      enabled,
      custom_rules: customRules,
      disabled_rules: Array.from(disabledRules),
    };
  }, [enabled, customRules, disabledRules]);

  const resetToBuiltin = useCallback(async () => {
    if (agentId === null) return;
    await api.resetAgentToolGuardToBuiltin(agentId);
    await fetchAll();
  }, [agentId, fetchAll]);

  return {
    config,
    builtinRules,
    customRules,
    disabledRules,
    enabled,
    mergedRules,
    loading,
    error,
    fetchAll,
    toggleRule,
    deleteCustomRule,
    addCustomRule,
    updateCustomRule,
    buildSaveBody,
    resetToBuiltin,
  };
}
