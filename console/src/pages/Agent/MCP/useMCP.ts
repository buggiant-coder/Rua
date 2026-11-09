import { useCallback, useEffect, useRef, useState } from "react";
import { agentsApi } from "../../../api/modules/agents";
import type { MCPClientInfo } from "../../../api/types";
import type { MCPApplication } from "../../../api/types/mcp";
import { useTranslation } from "react-i18next";
import { useAgentStore } from "../../../stores/agentStore";
import { toast } from "@/ui";

export function useMCP() {
  const { t } = useTranslation();
  const { selectedAgent } = useAgentStore();
  const [clients, setClients] = useState<MCPClientInfo[]>([]);
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [application, setApplication] = useState<MCPApplication | null>(null);
  const [applying, setApplying] = useState(false);
  const applyingRef = useRef(false);
  const agentRef = useRef(selectedAgent);
  const loadGeneration = useRef(0);
  agentRef.current = selectedAgent;
  useEffect(() => {
    setApplication(null);
  }, [selectedAgent]);

  const loadClients = useCallback(async () => {
    if (!selectedAgent) return;
    const generation = ++loadGeneration.current;
    setLoading(true);
    setLoadError(null);
    try {
      const data = await agentsApi.listAgentMCPClients(selectedAgent);
      if (
        agentRef.current !== selectedAgent ||
        generation !== loadGeneration.current
      )
        return;
      setClients(data);
    } catch (error) {
      if (
        agentRef.current !== selectedAgent ||
        generation !== loadGeneration.current
      )
        return;
      console.error("Failed to load MCP clients:", error);
      const msg = t("mcp.loadError");
      setLoadError(msg);
      toast.error(msg);
    } finally {
      if (
        agentRef.current === selectedAgent &&
        generation === loadGeneration.current
      )
        setLoading(false);
    }
  }, [t, selectedAgent]);

  const recordApplication = useCallback(
    (result: MCPApplication | undefined, agentId: string) => {
      if (agentRef.current !== agentId) return;
      setApplication(result ?? null);
      if (result?.status === "failed") toast.error(t("mcp.applicationFailed"));
    },
    [t],
  );

  const retryApplication = useCallback(async () => {
    if (!selectedAgent || applyingRef.current) return;
    applyingRef.current = true;
    setApplying(true);
    try {
      recordApplication(
        await agentsApi.applyAgentMCP(selectedAgent),
        selectedAgent,
      );
      await loadClients();
    } catch {
      toast.error(t("mcp.reapplyFailed"));
    } finally {
      applyingRef.current = false;
      setApplying(false);
    }
  }, [selectedAgent, recordApplication, loadClients, t]);

  useEffect(() => {
    loadClients();
  }, [loadClients, selectedAgent]);

  const createClient = useCallback(
    async (
      key: string,
      clientData: {
        name: string;
        description?: string;
        command: string;
        enabled?: boolean;
        transport?: "stdio" | "streamable_http" | "sse";
        url?: string;
        headers?: Record<string, string>;
        args?: string[];
        env?: Record<string, string>;
        cwd?: string;
      },
    ) => {
      if (!selectedAgent) return false;
      try {
        const result = await agentsApi.createAgentMCPClient(selectedAgent, {
          client_key: key,
          client: clientData,
        });
        recordApplication(result.application, selectedAgent);
        if (result.application?.status !== "failed")
          toast.success(t("mcp.createSuccess"));
        await loadClients();
        return true;
      } catch (error: unknown) {
        const errorMsg =
          error instanceof Error ? error.message : t("mcp.createError");
        toast.error(errorMsg);
        return false;
      }
    },
    [t, selectedAgent, loadClients],
  );

  const updateClient = useCallback(
    async (
      key: string,
      updates: {
        name?: string;
        description?: string;
        command?: string;
        enabled?: boolean;
        transport?: "stdio" | "streamable_http" | "sse";
        url?: string;
        headers?: Record<string, string>;
        args?: string[];
        env?: Record<string, string>;
        cwd?: string;
      },
    ) => {
      if (!selectedAgent) return false;
      try {
        const result = await agentsApi.updateAgentMCPClient(
          selectedAgent,
          key,
          updates,
        );
        recordApplication(result.application, selectedAgent);
        if (result.application?.status !== "failed")
          toast.success(t("mcp.updateSuccess"));
        await loadClients();
        return true;
      } catch (error: unknown) {
        const errorMsg =
          error instanceof Error ? error.message : t("mcp.updateError");
        toast.error(errorMsg);
        return false;
      }
    },
    [t, selectedAgent, loadClients],
  );

  const toggleEnabled = useCallback(
    async (client: MCPClientInfo) => {
      if (!selectedAgent) return;
      try {
        const result = await agentsApi.toggleAgentMCPClient(
          selectedAgent,
          client.key,
        );
        recordApplication(result.application, selectedAgent);
        if (result.application?.status !== "failed")
          toast.success(
            client.enabled ? t("mcp.disableSuccess") : t("mcp.enableSuccess"),
          );
        await loadClients();
      } catch (_error) {
        toast.error(t("mcp.toggleError"));
      }
    },
    [t, selectedAgent, loadClients],
  );

  const deleteClient = useCallback(
    async (client: MCPClientInfo) => {
      if (!selectedAgent) return;
      try {
        const result = await agentsApi.deleteAgentMCPClient(
          selectedAgent,
          client.key,
        );
        recordApplication(result.application, selectedAgent);
        if (result.application?.status !== "failed")
          toast.success(t("mcp.deleteSuccess"));
        await loadClients();
      } catch (_error) {
        toast.error(t("mcp.deleteError"));
      }
    },
    [t, selectedAgent, loadClients],
  );

  return {
    clients,
    loading,
    loadError,
    loadClients,
    createClient,
    updateClient,
    toggleEnabled,
    deleteClient,
    application,
    applying,
    retryApplication,
  };
}
