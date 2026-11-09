import { useCallback, useEffect, useState } from "react";
import { sharedMcpApi } from "../../../api";
import type { MCPClientInfo } from "../../../api/types";
import { useTranslation } from "react-i18next";
import { toast } from "@/ui";

export function useSharedMcp() {
  const { t } = useTranslation();
  const [clients, setClients] = useState<MCPClientInfo[]>([]);
  const [loading, setLoading] = useState(false);

  const loadClients = useCallback(async () => {
    setLoading(true);
    try {
      const data = await sharedMcpApi.list();
      setClients(data);
    } catch (error) {
      console.error("Failed to load shared MCP clients:", error);
      toast.error(t("sharedMcp.loadError"));
    } finally {
      setLoading(false);
    }
  }, [t]);

  useEffect(() => {
    loadClients();
  }, [loadClients]);

  const createClient = useCallback(
    async (
      key: string,
      clientData: {
        name: string;
        description?: string;
        enabled?: boolean;
        transport?: "stdio" | "streamable_http" | "sse";
        url?: string;
        headers?: Record<string, string>;
        command?: string;
        args?: string[];
        env?: Record<string, string>;
        cwd?: string;
      },
    ) => {
      try {
        await sharedMcpApi.create(key, clientData);
        toast.success(t("sharedMcp.createSuccess"));
        await loadClients();
        return true;
      } catch (error: unknown) {
        const errorMsg =
          error instanceof Error ? error.message : t("sharedMcp.createError");
        toast.error(errorMsg);
        return false;
      }
    },
    [t, loadClients],
  );

  const updateClient = useCallback(
    async (key: string, updates: Partial<MCPClientInfo>) => {
      try {
        await sharedMcpApi.update(key, updates);
        toast.success(t("sharedMcp.updateSuccess"));
        await loadClients();
        return true;
      } catch (error: unknown) {
        const errorMsg =
          error instanceof Error ? error.message : t("sharedMcp.updateError");
        toast.error(errorMsg);
        return false;
      }
    },
    [t, loadClients],
  );

  const deleteClient = useCallback(
    async (client: MCPClientInfo) => {
      try {
        await sharedMcpApi.delete(client.key);
        toast.success(t("sharedMcp.deleteSuccess"));
        await loadClients();
      } catch (_error) {
        toast.error(t("sharedMcp.deleteError"));
      }
    },
    [t, loadClients],
  );

  return {
    clients,
    loading,
    loadClients,
    createClient,
    updateClient,
    deleteClient,
  };
}
