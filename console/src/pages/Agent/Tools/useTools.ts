import { useCallback, useEffect, useState } from "react";
import type { ToolInfo } from "../../../api/modules/tools";
import { agentsApi } from "../../../api/modules/agents";
import { useTranslation } from "react-i18next";
import { useAgentStore } from "../../../stores/agentStore";
import { toast } from "@/ui";

export interface ToolsInitialData {
  agentId: string;
  tools: ToolInfo[];
}

export function useTools(initialData?: ToolsInitialData) {
  const { t } = useTranslation();
  const { selectedAgent } = useAgentStore();
  const [tools, setTools] = useState<ToolInfo[]>(initialData?.tools ?? []);
  const [loading, setLoading] = useState(initialData === undefined);
  const [batchLoading, setBatchLoading] = useState(false);

  const loadTools = useCallback(async () => {
    setLoading(true);
    try {
      const data = await agentsApi.listAgentTools(selectedAgent);
      setTools(data);
    } catch (error) {
      console.error("Failed to load tools:", error);
      toast.error(t("tools.loadError"));
    } finally {
      setLoading(false);
    }
  }, [t, selectedAgent]);

  useEffect(() => {
    if (initialData) {
      setTools(initialData.tools);
      setLoading(false);
      return;
    }
    void loadTools();
  }, [initialData, loadTools, selectedAgent]);

  const toggleEnabled = useCallback(
    async (tool: ToolInfo) => {
      setTools((prev) =>
        prev.map((t) =>
          t.name === tool.name ? { ...t, enabled: !t.enabled } : t,
        ),
      );

      try {
        const result = await agentsApi.toggleAgentTool(
          selectedAgent,
          tool.name,
        );
        toast.success(
          tool.enabled ? t("tools.disableSuccess") : t("tools.enableSuccess"),
        );

        setTools((prev) =>
          prev.map((t) => (t.name === result.name ? result : t)),
        );
      } catch (_error) {
        setTools((prev) =>
          prev.map((t) =>
            t.name === tool.name ? { ...t, enabled: tool.enabled } : t,
          ),
        );
        toast.error(t("tools.toggleError"));
      }
    },
    [t, selectedAgent],
  );

  const toggleAsyncExecution = useCallback(
    async (tool: ToolInfo) => {
      setTools((prev) =>
        prev.map((t) =>
          t.name === tool.name
            ? { ...t, async_execution: !t.async_execution }
            : t,
        ),
      );

      try {
        const result = await agentsApi.updateAgentToolAsyncExecution(
          selectedAgent,
          tool.name,
          !tool.async_execution,
        );
        toast.success(
          result.async_execution
            ? t("tools.asyncExecutionEnabled")
            : t("tools.asyncExecutionDisabled"),
        );

        setTools((prev) =>
          prev.map((t) => (t.name === result.name ? result : t)),
        );
      } catch (_error) {
        setTools((prev) =>
          prev.map((t) =>
            t.name === tool.name
              ? { ...t, async_execution: tool.async_execution }
              : t,
          ),
        );
        toast.error(t("tools.toggleError"));
      }
    },
    [t, selectedAgent],
  );

  const enableAll = useCallback(async () => {
    const disabledTools = tools.filter((tool) => !tool.enabled);
    if (disabledTools.length === 0) {
      toast.info(t("tools.allEnabled"));
      return;
    }

    setTools((prev) => prev.map((t) => ({ ...t, enabled: true })));

    setBatchLoading(true);
    try {
      const results = await Promise.all(
        disabledTools.map((tool) =>
          agentsApi.toggleAgentTool(selectedAgent, tool.name),
        ),
      );
      toast.success(t("tools.enableAllSuccess"));

      setTools((prev) =>
        prev.map((t) => {
          const result = results.find((r) => r.name === t.name);
          return result ? { ...result, async_execution: t.async_execution } : t;
        }),
      );
    } catch (_error) {
      toast.error(t("tools.toggleError"));

      await loadTools();
    } finally {
      setBatchLoading(false);
    }
  }, [tools, t, loadTools, selectedAgent]);

  const disableAll = useCallback(async () => {
    const enabledTools = tools.filter((tool) => tool.enabled);
    if (enabledTools.length === 0) {
      toast.info(t("tools.allDisabled"));
      return;
    }

    setTools((prev) => prev.map((t) => ({ ...t, enabled: false })));

    setBatchLoading(true);
    try {
      const results = await Promise.all(
        enabledTools.map((tool) =>
          agentsApi.toggleAgentTool(selectedAgent, tool.name),
        ),
      );
      toast.success(t("tools.disableAllSuccess"));

      setTools((prev) =>
        prev.map((t) => {
          const result = results.find((r) => r.name === t.name);
          return result ? { ...result, async_execution: t.async_execution } : t;
        }),
      );
    } catch (_error) {
      toast.error(t("tools.toggleError"));

      await loadTools();
    } finally {
      setBatchLoading(false);
    }
  }, [tools, t, loadTools, selectedAgent]);

  return {
    tools,
    loading,
    batchLoading,
    loadTools,
    toggleEnabled,
    toggleAsyncExecution,
    enableAll,
    disableAll,
  };
}
