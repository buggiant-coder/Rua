import { useState, useEffect, useCallback, useRef } from "react";
import { Modal } from "@/ui";
import api from "../../../api";
import { agentsApi } from "../../../api";
import type { SecurityScanErrorResponse } from "../../../api/modules/security";
import type { SkillSpec } from "../../../api/types";
import { useTranslation } from "react-i18next";
import { useAgentStore } from "../../../stores/agentStore";
import { parseErrorDetail } from "../../../utils/error";
import {
  handleScanError,
  checkScanWarnings as checkScanWarningsShared,
  showScanErrorModal,
} from "../../../utils/scanError";
import { toast } from "@/ui";

type SkillConflictDetail = {
  reason?: string;
  skill_name: string;
  suggested_name: string;
};

type SkillConflictResult = {
  suggested_name?: string;
  conflicts?: SkillConflictDetail[];
};

function toSkillConflictResult(value: {
  suggested_name?: string;
  conflicts?: Array<{
    reason?: string;
    skill_name?: string;
    suggested_name?: string;
  }>;
}): SkillConflictResult {
  return {
    suggested_name: value.suggested_name,
    conflicts: value.conflicts
      ?.filter((item) => item.skill_name && item.suggested_name)
      .map((item) => ({
        reason: item.reason,
        skill_name: item.skill_name!,
        suggested_name: item.suggested_name!,
      })),
  };
}

type SkillActionResult =
  | { success: true; name?: string; imported?: string[] }
  | { success: false; conflict?: SkillConflictResult };

const skillListCache = new Map<string, SkillSpec[]>();

const skillCacheKey = (agentId: string | null | undefined) =>
  agentId || "__default__";

export function __resetSkillListCacheForTests() {
  skillListCache.clear();
}

export function useSkills() {
  const { t } = useTranslation();
  const { selectedAgent } = useAgentStore();
  const cacheKey = skillCacheKey(selectedAgent);
  const [skills, setSkills] = useState<SkillSpec[]>(
    () => skillListCache.get(cacheKey) ?? [],
  );
  const skillsRef = useRef<SkillSpec[]>(skills);
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [uploading, setUploading] = useState(false);
  const [importing, setImporting] = useState(false);
  const importTaskIdRef = useRef<{ taskId: string; agentId: string } | null>(
    null,
  );
  const importCancelReasonRef = useRef<"manual" | "timeout" | null>(null);

  const handleError = useCallback(
    (error: unknown, defaultMsg: string): boolean => {
      if (handleScanError(error, t)) return true;
      const msg =
        error instanceof Error && error.message ? error.message : defaultMsg;
      console.error(defaultMsg, error);
      toast.error(msg);
      return false;
    },
    [t],
  );

  const checkScanWarnings = useCallback(
    (skillName: string) =>
      checkScanWarningsShared(
        skillName,
        api.getBlockedHistory,
        api.getSkillScanner,
        t,
      ),
    [t],
  );

  const applySkills = useCallback(
    (next: SkillSpec[]) => {
      skillListCache.set(cacheKey, next);
      skillsRef.current = next;
      setSkills(next);
    },
    [cacheKey],
  );

  const updateCachedSkills = useCallback(
    (updater: (prev: SkillSpec[]) => SkillSpec[]) => {
      setSkills((prev) => {
        const next = updater(prev);
        skillListCache.set(cacheKey, next);
        skillsRef.current = next;
        return next;
      });
    },
    [cacheKey],
  );

  const fetchSkills = useCallback(
    async (forceReload = false) => {
      let hasCached = false;
      if (!forceReload) {
        const cached = skillListCache.get(cacheKey);
        if (cached) {
          hasCached = true;
          skillsRef.current = cached;
          setSkills(cached);
          setLoadError(null);
        }
      }

      setLoading(!hasCached && skillsRef.current.length === 0);
      setLoadError(null);
      try {
        const data = await agentsApi.listAgentSkills(selectedAgent);
        applySkills(data || []);
      } catch (error) {
        console.error(t("skills.loadFailed"), error);
        const msg =
          error instanceof Error && error.message
            ? error.message
            : t("skills.loadFailed");
        setLoadError(msg);
        toast.error(msg);
      } finally {
        setLoading(false);
      }
    },
    [applySkills, cacheKey, selectedAgent, t],
  );

  const hardRefresh = useCallback(async () => {
    setLoading(skills.length === 0);
    setLoadError(null);
    try {
      const data = await agentsApi.refreshAgentSkills(selectedAgent);
      applySkills(data || []);
    } catch (error) {
      console.error(t("skills.refreshFailed"), error);
      const msg =
        error instanceof Error && error.message
          ? error.message
          : t("skills.refreshFailed");
      setLoadError(msg);
      toast.error(msg);
    } finally {
      setLoading(false);
    }
  }, [applySkills, selectedAgent, skills.length, t]);

  useEffect(() => {
    const cached = skillListCache.get(cacheKey);
    skillsRef.current = cached ?? [];
    setSkills(cached ?? []);
    void fetchSkills();
  }, [cacheKey, fetchSkills]);

  const createSkill = async (
    name: string,
    content: string,
    config?: Record<string, unknown>,
    enable?: boolean,
  ): Promise<SkillActionResult> => {
    try {
      const result = await agentsApi.createAgentSkill(
        selectedAgent,
        name,
        content,
        config,
        enable,
      );
      toast.success(t("skills.createdSuccessfully"));

      await fetchSkills(true);
      await checkScanWarnings(result.name);
      return { success: true, name: result.name };
    } catch (error) {
      const detail = parseErrorDetail(error);
      if (detail?.suggested_name) {
        return { success: false, conflict: detail };
      }
      handleError(error, t("skills.saveFailed"));
      return { success: false };
    }
  };

  const uploadSkill = async (
    file: File,
    targetName?: string,
    renameMap?: Record<string, string>,
  ): Promise<SkillActionResult> => {
    try {
      setUploading(true);
      const result = await agentsApi.uploadAgentSkill(selectedAgent, file, {
        enable: true,
        target_name: targetName,
        rename_map: renameMap,
      });
      if (result?.count > 0) {
        toast.success(
          t("skills.uploadSuccess") + `: ${result.imported.join(", ")}`,
        );

        await fetchSkills(true);
        for (const name of result.imported) {
          await checkScanWarnings(name);
        }
      }
      if (!result?.count) {
        toast.warning(t("skills.uploadNoChange"));
      }
      await fetchSkills(true);
      return { success: true, imported: result?.imported || [] };
    } catch (error) {
      const detail = parseErrorDetail(error);
      if (Array.isArray(detail?.conflicts) && detail.conflicts.length > 0) {
        return { success: false, conflict: detail };
      }
      handleError(error, t("skills.uploadFailed"));
      return { success: false };
    } finally {
      setUploading(false);
    }
  };

  const importFromHub = async (
    input: string,
    targetName?: string,
  ): Promise<SkillActionResult> => {
    const text = (input || "").trim();
    if (!text) {
      toast.warning(t("skills.provideUrl"));
      return { success: false };
    }
    if (!text.startsWith("http://") && !text.startsWith("https://")) {
      toast.warning(t("skills.validUrl"));
      return { success: false };
    }
    const timeoutMs = 90_000;
    const pollMs = 1_000;
    const startedAt = Date.now();
    try {
      setImporting(true);
      importCancelReasonRef.current = null;
      const payload = {
        bundle_url: text,
        enable: true,
        target_name: targetName,
      };
      const agentId = selectedAgent;
      const task = await api.startHubSkillInstall(payload, agentId);
      importTaskIdRef.current = { taskId: task.task_id, agentId };

      while (importTaskIdRef.current) {
        const status = await api.getHubSkillInstallStatus(
          task.task_id,
          agentId,
        );

        if (status.status === "completed" && status.result?.installed) {
          toast.success(
            t("skills.importedSkill", { name: status.result.name }),
          );

          await fetchSkills(true);
          if (status.result.name) {
            await checkScanWarnings(status.result.name);
          }
          return { success: true, name: String(status.result.name || "") };
        }

        if (status.status === "failed") {
          if (
            Array.isArray(status.result?.conflicts) &&
            status.result.conflicts.length > 0
          ) {
            return {
              success: false,
              conflict: toSkillConflictResult(status.result),
            };
          }
          const hubResult = status.result as
            | SecurityScanErrorResponse
            | null
            | undefined;
          if (hubResult?.type === "security_scan_failed") {
            showScanErrorModal(hubResult, t);
            return { success: false };
          }
          throw new Error(status.error || t("skills.importFailed"));
        }

        if (status.status === "cancelled") {
          toast.warning(
            t(
              importCancelReasonRef.current === "timeout"
                ? "skills.importTimeout"
                : "skills.importCancelled",
            ),
          );
          return { success: false };
        }

        if (Date.now() - startedAt >= timeoutMs) {
          importCancelReasonRef.current = "timeout";
          await api.cancelHubSkillInstall(task.task_id, agentId);
        }

        await new Promise((resolve) => window.setTimeout(resolve, pollMs));
      }

      return { success: false };
    } catch (error) {
      handleError(error, t("skills.importFailed"));
      return { success: false };
    } finally {
      importTaskIdRef.current = null;
      importCancelReasonRef.current = null;
      setImporting(false);
    }
  };

  const cancelImport = useCallback(() => {
    if (!importing) return;
    importCancelReasonRef.current = "manual";
    const taskId = importTaskIdRef.current;
    if (!taskId) return;
    void api.cancelHubSkillInstall(taskId.taskId, taskId.agentId);
  }, [importing]);

  const toggleEnabled = async (skill: SkillSpec) => {
    try {
      if (skill.enabled) {
        await agentsApi.disableAgentSkill(selectedAgent, skill.name);
        updateCachedSkills((prev) =>
          prev.map((s) =>
            s.name === skill.name ? { ...s, enabled: false } : s,
          ),
        );
        toast.success(t("skills.disabledSuccessfully"));
      } else {
        await agentsApi.enableAgentSkill(selectedAgent, skill.name);
        updateCachedSkills((prev) =>
          prev.map((s) =>
            s.name === skill.name ? { ...s, enabled: true } : s,
          ),
        );
        toast.success(t("skills.enabledSuccessfully"));
        await checkScanWarnings(skill.name);
      }

      return true;
    } catch (error) {
      handleError(error, t("skills.operationFailed"));
      return false;
    }
  };

  const deleteSkill = async (skill: SkillSpec) => {
    const confirmed = await new Promise<boolean>((resolve) => {
      Modal.confirm({
        title: t("common.confirm"),
        content: t("skills.deleteConfirm"),
        okText: t("common.delete"),
        okType: "danger",
        cancelText: t("common.cancel"),
        onOk: () => resolve(true),
        onCancel: () => resolve(false),
      });
    });

    if (!confirmed) return false;

    try {
      const result = await agentsApi.deleteAgentSkill(
        selectedAgent,
        skill.name,
      );
      if (result.deleted) {
        toast.success(t("skills.deleteSuccess"));

        await fetchSkills(true);
        return true;
      }
    } catch (error) {
      console.error(t("skills.deleteFailed"), error);
      toast.error(t("skills.deleteFailed"));
    }
    return false;
  };

  return {
    skills,
    loading,
    loadError,
    uploading,
    importing,
    createSkill,
    uploadSkill,
    importFromHub,
    cancelImport,
    toggleEnabled,
    deleteSkill,
    refreshSkills: () => fetchSkills(true),
    hardRefresh,
  };
}
