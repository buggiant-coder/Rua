import { useCallback, useEffect, useState } from "react";
import { acpApi } from "../../../api/modules/acp";
import type {
  ACPRunnerInfo,
  ACPRunnerInstallTask,
  ACPRunnerCreateRequest,
  ACPRunnerUpdateRequest,
} from "../../../api/types";
import { useTranslation } from "react-i18next";
import { toast } from "@/ui";

const INSTALL_POLL_MS = 1000;
const INSTALL_TERMINAL_STATUSES = new Set(["completed", "failed"]);

function sleep(ms: number) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

export function useACP() {
  const { t } = useTranslation();
  const [runners, setRunners] = useState<ACPRunnerInfo[]>([]);
  const [installingKeys, setInstallingKeys] = useState<Record<string, boolean>>(
    {},
  );
  const [installTasks, setInstallTasks] = useState<
    Record<string, ACPRunnerInstallTask>
  >({});
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);

  const loadRunners = useCallback(async () => {
    setLoading(true);
    setLoadError(null);
    try {
      const data = await acpApi.listRunners();
      setRunners(data);
    } catch (error) {
      console.error("Failed to load ACP runners:", error);
      const msg = t("acp.loadError");
      setLoadError(msg);
      toast.error(msg);
    } finally {
      setLoading(false);
    }
  }, [t]);

  useEffect(() => {
    loadRunners();
  }, [loadRunners]);

  const createRunner = useCallback(
    async (body: ACPRunnerCreateRequest) => {
      try {
        await acpApi.createRunner(body);
        toast.success(t("acp.createSuccess"));
        await loadRunners();
        return true;
      } catch (error: unknown) {
        const errorMsg =
          error instanceof Error ? error.message : t("acp.createError");
        toast.error(errorMsg);
        return false;
      }
    },
    [t, loadRunners],
  );

  const updateRunner = useCallback(
    async (key: string, updates: ACPRunnerUpdateRequest) => {
      try {
        await acpApi.updateRunner(key, updates);
        toast.success(t("acp.updateSuccess"));
        await loadRunners();
        return true;
      } catch (error: unknown) {
        const errorMsg =
          error instanceof Error ? error.message : t("acp.updateError");
        toast.error(errorMsg);
        return false;
      }
    },
    [t, loadRunners],
  );

  const toggleEnabled = useCallback(
    async (runner: ACPRunnerInfo) => {
      try {
        await acpApi.toggleRunner(runner.key);
        toast.success(
          runner.enabled ? t("acp.disableSuccess") : t("acp.enableSuccess"),
        );
        await loadRunners();
      } catch (error: unknown) {
        toast.error(
          error instanceof Error ? error.message : t("acp.toggleError"),
        );
      }
    },
    [t, loadRunners],
  );

  const installRunner = useCallback(
    async (key: string) => {
      setInstallingKeys((prev) => ({ ...prev, [key]: true }));
      try {
        let task = await acpApi.startRunnerInstall(key);
        setInstallTasks((prev) => ({ ...prev, [key]: task }));
        while (!INSTALL_TERMINAL_STATUSES.has(task.status)) {
          await sleep(INSTALL_POLL_MS);
          task = await acpApi.getRunnerInstallTask(task.task_id);
          setInstallTasks((prev) => ({ ...prev, [key]: task }));
        }

        await loadRunners();
        if (task.status !== "completed") {
          toast.error(task.detail || task.message || t("acp.installError"));
          return false;
        }
        if (task.check_result?.ok) {
          toast.success(t("acp.installSuccess"));
        } else if (task.check_result?.requires_auth) {
          toast.warning(t("acp.installAuthRequired"));
        } else {
          toast.warning(task.check_result?.message || task.message);
        }
        return true;
      } catch (error: unknown) {
        toast.error(
          error instanceof Error ? error.message : t("acp.installError"),
        );
        return false;
      } finally {
        setInstallingKeys((prev) => ({ ...prev, [key]: false }));
      }
    },
    [t, loadRunners],
  );

  const deleteRunner = useCallback(
    async (runner: ACPRunnerInfo) => {
      try {
        await acpApi.deleteRunner(runner.key);
        toast.success(t("acp.deleteSuccess"));
        await loadRunners();
      } catch (_error) {
        toast.error(t("acp.deleteError"));
      }
    },
    [t, loadRunners],
  );

  return {
    runners,
    installingKeys,
    installTasks,
    loading,
    loadError,
    loadRunners,
    installRunner,
    createRunner,
    updateRunner,
    toggleEnabled,
    deleteRunner,
  };
}
