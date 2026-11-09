import { useState, useEffect, useMemo } from "react";
import { useTranslation } from "react-i18next";
import api from "../../../api";
import type { CronJobSpecOutput } from "../../../api/types";
import { useAgentStore } from "../../../stores/agentStore";
import { showErrorModal } from "../../../utils/error";
import { computeNextRun } from "./components/nextRunTime";
import { toast, Modal } from "@/ui";

type CronJob = CronJobSpecOutput;

export interface CronJobsInitialData {
  agentId: string;
  jobs: CronJobSpecOutput[];
}

export type CronJobSortField = "default" | "name" | "nextRun" | "created";
export type SortOrder = "asc" | "desc";

const SORT_FIELD_KEY = "rua.cronJobs.sortField";
const SORT_ORDER_KEY = "rua.cronJobs.sortOrder";

function readSortField(): CronJobSortField {
  try {
    const v = localStorage.getItem(SORT_FIELD_KEY);
    if (v === "default" || v === "name" || v === "nextRun" || v === "created") {
      return v;
    }
  } catch {}
  return "default";
}

function readSortOrder(): SortOrder {
  try {
    const v = localStorage.getItem(SORT_ORDER_KEY);
    if (v === "asc" || v === "desc") return v;
  } catch {}
  return "desc";
}

function getCreatedAt(job: CronJob): number {
  const meta = (job.meta || {}) as Record<string, unknown>;
  const raw = meta.created_at;
  if (typeof raw === "number" && Number.isFinite(raw)) return raw;
  if (typeof raw === "string") {
    const parsed = Date.parse(raw);
    if (!Number.isNaN(parsed)) return parsed;
  }
  return 0;
}

function getNextRunTime(job: CronJob): { valueOf: () => number } | null {
  const schedule = job.schedule as any;
  if (!job.enabled || !schedule) return null;
  if (schedule.type === "once") {
    if (!schedule.run_at) return null;
    const next = new Date(schedule.run_at);
    return Number.isNaN(next.valueOf()) ? null : next;
  }
  return computeNextRun(schedule.cron || "");
}

export function useCronJobs(initialData?: CronJobsInitialData) {
  const { t } = useTranslation();
  const { selectedAgent } = useAgentStore();
  const [jobs, setJobs] = useState<CronJob[]>(initialData?.jobs ?? []);
  const [loading, setLoading] = useState(initialData === undefined);

  const [searchQuery, setSearchQuery] = useState("");
  const [sortField, setSortFieldState] =
    useState<CronJobSortField>(readSortField);
  const [sortOrder, setSortOrderState] = useState<SortOrder>(readSortOrder);

  const setSortField = (value: CronJobSortField) => {
    setSortFieldState(value);
    try {
      localStorage.setItem(SORT_FIELD_KEY, value);
    } catch {}
  };

  const setSortOrder = (value: SortOrder) => {
    setSortOrderState(value);
    try {
      localStorage.setItem(SORT_ORDER_KEY, value);
    } catch {}
  };

  const fetchJobs = async () => {
    setLoading(true);
    try {
      const data = await api.listCronJobs(selectedAgent || undefined);
      if (data) {
        setJobs(data as CronJob[]);
      }
    } catch (error) {
      console.error("Failed to load cron jobs", error);
      toast.error(t("cronJobs.loadFailed"));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    if (initialData) {
      setJobs(initialData.jobs);
      setLoading(false);
      return;
    }
    let mounted = true;

    const loadJobs = async () => {
      await fetchJobs();
    };

    if (mounted) {
      loadJobs();
    }

    return () => {
      mounted = false;
    };

    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [initialData, selectedAgent]);

  const createJob = async (values: CronJob) => {
    try {
      const created = await api.createCronJob(
        values,
        selectedAgent || undefined,
      );
      setJobs((prev) => [created as CronJob, ...prev]);
      toast.success(t("cronJobs.createSuccess"));
      return true;
    } catch (error) {
      console.error("Failed to create cron job", error);
      showErrorModal(Modal, {
        title: t("cronJobs.saveFailedTitle"),
        error,
        detailLabel: t("common.errorDetails"),
      });
      return false;
    }
  };

  const updateJob = async (jobId: string, values: CronJob) => {
    const original = jobs.find((j) => j.id === jobId);
    const optimisticUpdate = { ...original, ...values };
    setJobs((prev) => prev.map((j) => (j.id === jobId ? optimisticUpdate : j)));

    try {
      const updated = await api.replaceCronJob(
        jobId,
        values,
        selectedAgent || undefined,
      );
      setJobs((prev) =>
        prev.map((j) => (j.id === jobId ? (updated as CronJob) : j)),
      );
      toast.success(t("cronJobs.updateSuccess"));
      return true;
    } catch (error) {
      console.error("Failed to update cron job", error);
      if (original) {
        setJobs((prev) => prev.map((j) => (j.id === jobId ? original : j)));
      }
      showErrorModal(Modal, {
        title: t("cronJobs.saveFailedTitle"),
        error,
        detailLabel: t("common.errorDetails"),
      });
      return false;
    }
  };

  const deleteJob = async (jobId: string) => {
    const original = jobs.find((j) => j.id === jobId);
    setJobs((prev) => prev.filter((j) => j.id !== jobId));

    try {
      await api.deleteCronJob(jobId, selectedAgent || undefined);
      toast.success(t("cronJobs.deleteSuccess"));
      return true;
    } catch (error) {
      console.error("Failed to delete cron job", error);
      if (original) {
        setJobs((prev) => [...prev, original]);
      }
      toast.error(t("cronJobs.deleteFailed"));
      return false;
    }
  };

  const toggleEnabled = async (job: CronJob) => {
    const updated = { ...job, enabled: !job.enabled };
    setJobs((prev) => prev.map((j) => (j.id === job.id ? updated : j)));

    try {
      const returned = await api.replaceCronJob(
        job.id,
        updated,
        selectedAgent || undefined,
      );
      setJobs((prev) =>
        prev.map((j) => (j.id === job.id ? (returned as CronJob) : j)),
      );
      toast.success(
        updated.enabled ? t("cronJobs.enabledMsg") : t("cronJobs.disabledMsg"),
      );
      return true;
    } catch (error) {
      console.error("Failed to toggle cron job", error);
      setJobs((prev) => prev.map((j) => (j.id === job.id ? job : j)));
      toast.error(t("cronJobs.operationFailed"));
      return false;
    }
  };

  const executeNow = async (jobId: string) => {
    try {
      await api.triggerCronJob(jobId, selectedAgent || undefined);
      toast.success(t("cronJobs.executeSuccess"));
      return true;
    } catch (error) {
      console.error("Failed to execute cron job", error);
      toast.error(t("cronJobs.executeFailed"));
      return false;
    }
  };

  const filteredJobs = useMemo(() => {
    const q = searchQuery.trim().toLowerCase();
    const filtered = q
      ? jobs.filter((j) => (j.name || "").toLowerCase().includes(q))
      : jobs;
    if (sortField === "default") return filtered;

    const sign = sortOrder === "asc" ? 1 : -1;
    return filtered.slice().sort((a, b) => {
      let cmp = 0;
      if (sortField === "name") {
        cmp = (a.name || "").localeCompare(b.name || "");
      } else if (sortField === "nextRun") {
        const na = getNextRunTime(a);
        const nb = getNextRunTime(b);
        const ta = na ? na.valueOf() : Number.POSITIVE_INFINITY;
        const tb = nb ? nb.valueOf() : Number.POSITIVE_INFINITY;
        if (ta === tb) return 0;
        return ta < tb ? -1 * sign : 1 * sign;
      } else if (sortField === "created") {
        const ca = getCreatedAt(a);
        const cb = getCreatedAt(b);
        cmp = ca - cb;
      }
      return cmp * sign;
    });
  }, [jobs, searchQuery, sortField, sortOrder]);

  return {
    jobs,
    filteredJobs,
    loading,
    fetchJobs,
    createJob,
    updateJob,
    deleteJob,
    toggleEnabled,
    executeNow,

    searchQuery,
    setSearchQuery,
    sortField,
    setSortField,
    sortOrder,
    setSortOrder,
  };
}
