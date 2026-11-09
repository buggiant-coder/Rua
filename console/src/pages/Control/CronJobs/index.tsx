/* eslint-disable react-refresh/only-export-components */
import { useState, useEffect, useRef } from "react";
import { useLoaderData } from "react-router-dom";
import type { CronJobSpecOutput } from "../../../api/types";
import { useTranslation } from "react-i18next";
import dayjs from "dayjs";
import api from "../../../api";
import {
  JobDrawer,
  JobList,
  useCronJobs,
  DEFAULT_FORM_VALUES,
} from "./components";
import type { CronJobSortField } from "./useCronJobs";
import type { CronJobsInitialData } from "./useCronJobs";
import { serializeCron } from "./components/parseCron";
import { PageHeader } from "@/components/PageHeader";
import { Page, PageBody } from "@/components/Page";
import { isMobile } from "@/utils/isMobile";
import styles from "./index.module.less";
import { ArrowUpNarrowWide, Plus } from "lucide-react";
import { Btn, Dropdown, Modal, TextInput, Tooltip } from "@/ui";
import { routeAgentId } from "@/routes/routeAgent";

type CronJob = CronJobSpecOutput;

interface CronJobsPageProps {
  initialData?: CronJobsInitialData;
}

export async function loader(): Promise<CronJobsInitialData> {
  const agentId = routeAgentId();
  return {
    agentId,
    jobs: await api.listCronJobs(agentId),
  };
}

export function RouteComponent() {
  return <CronJobsPage initialData={useLoaderData() as CronJobsInitialData} />;
}

function CronJobsPage({ initialData }: CronJobsPageProps) {
  const { t } = useTranslation();
  const {
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
  } = useCronJobs(initialData);
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [editingJob, setEditingJob] = useState<CronJob | null>(null);
  const [saving, setSaving] = useState(false);
  const userTimezoneRef = useRef("UTC");

  useEffect(() => {
    api
      .getUserTimezone()
      .then((res) => {
        if (res.timezone) userTimezoneRef.current = res.timezone;
      })
      .catch((err) => console.error("Failed to fetch user timezone:", err));
  }, []);

  const handleCreate = () => {
    setEditingJob(null);
    setDrawerOpen(true);
  };

  const handleEdit = (job: CronJob) => {
    setEditingJob(job);
    setDrawerOpen(true);
  };

  const handleDelete = (jobId: string) => {
    const job = jobs.find((j) => j.id === jobId);
    const wasEnabled = job?.enabled;
    Modal.confirm({
      title: t("cronJobs.confirmDelete"),
      content: wasEnabled
        ? t("cronJobs.deleteConfirmEnabled")
        : t("cronJobs.deleteConfirm"),
      okText: t("cronJobs.deleteText"),
      okType: "primary",
      cancelText: t("cronJobs.cancelText"),
      onOk: async () => {
        await deleteJob(jobId);
      },
    });
  };

  const handleToggleEnabled = async (job: CronJob) => {
    await toggleEnabled(job);
  };

  const handleExecuteNow = async (job: CronJob) => {
    Modal.confirm({
      title: t("cronJobs.executeNowTitle"),
      content: t("cronJobs.executeNowContent", { name: job.name }),
      okText: t("cronJobs.executeNowConfirm"),
      okType: "primary",
      cancelText: t("cronJobs.cancelText"),
      onOk: async () => {
        await executeNow(job.id);
      },
    });
  };

  const handleDrawerClose = () => {
    setDrawerOpen(false);
    setEditingJob(null);
  };

  const handleSubmit = async (values: any) => {
    values = {
      ...DEFAULT_FORM_VALUES,
      ...values,
      schedule: { ...DEFAULT_FORM_VALUES.schedule, ...(values.schedule ?? {}) },
      dispatch: {
        ...DEFAULT_FORM_VALUES.dispatch,
        ...(values.dispatch ?? {}),
        target: {
          ...DEFAULT_FORM_VALUES.dispatch.target,
          ...(values.dispatch?.target ?? {}),
        },
      },
      runtime: { ...DEFAULT_FORM_VALUES.runtime, ...(values.runtime ?? {}) },
    };

    let schedule: any;
    if (values.scheduleMode === "once") {
      const runAt = dayjs(`${values.onceDate} ${values.onceTime || "00:00"}`);
      schedule = {
        type: "once",
        run_at: runAt.toISOString(),
        timezone: values.schedule?.timezone || userTimezoneRef.current || "UTC",
      };
      if (values.repeatEnabled) {
        schedule.repeat_every_days = values.repeatEveryDays || 1;
        schedule.repeat_end_type = values.repeatEndType || "never";
        if (schedule.repeat_end_type === "until" && values.repeatUntilDate) {
          schedule.repeat_until = dayjs(
            `${values.repeatUntilDate} 23:59`,
          ).toISOString();
        }
        if (schedule.repeat_end_type === "count" && values.repeatCount) {
          schedule.repeat_count = values.repeatCount;
        }
      }
    } else {
      const cronParts: any = {
        type: values.cronType || "daily",
      };

      if (values.cronType === "daily" || values.cronType === "weekly") {
        if (values.cronTime) {
          const hour =
            typeof values.cronTime.hour === "function"
              ? values.cronTime.hour()
              : Number(String(values.cronTime).split(":")[0] || 0);
          const minute =
            typeof values.cronTime.minute === "function"
              ? values.cronTime.minute()
              : Number(String(values.cronTime).split(":")[1] || 0);
          cronParts.hour = hour;
          cronParts.minute = minute;
        }
      }

      if (values.cronType === "weekly" && values.cronDaysOfWeek) {
        cronParts.daysOfWeek = values.cronDaysOfWeek;
      }

      if (values.cronType === "custom" && values.cronCustom) {
        cronParts.rawCron = values.cronCustom;
      }

      schedule = {
        ...values.schedule,
        type: "cron",
        cron: serializeCron(cronParts),
        run_at: undefined,
        repeat_every_days: undefined,
        repeat_end_type: undefined,
        repeat_until: undefined,
        repeat_count: undefined,
      };
    }

    let processedValues = {
      ...values,
      id: editingJob ? editingJob.id : "",
      schedule,
    };

    if (processedValues.task_type === "text") {
      const { request: _ignored, ...withoutRequest } = processedValues as any;
      processedValues = withoutRequest;
    } else if (
      values.request?.input &&
      typeof values.request.input === "string"
    ) {
      try {
        processedValues = {
          ...processedValues,
          request: {
            input: JSON.parse(values.request.input as any),
          },
        };
      } catch (error) {
        console.error("❌ Failed to parse request.input JSON:", error);
      }
    }

    const performSave = async () => {
      let success = false;
      setSaving(true);
      try {
        if (editingJob) {
          success = await updateJob(editingJob.id, processedValues);
        } else {
          success = await createJob(processedValues);
        }
      } finally {
        setSaving(false);
      }
      if (success) {
        setDrawerOpen(false);
      }
    };

    if (editingJob && editingJob.enabled) {
      Modal.confirm({
        title: t("cronJobs.rescheduleConfirmTitle"),
        content: t("cronJobs.rescheduleConfirmContent"),
        okText: t("common.save"),
        okType: "primary",
        cancelText: t("cronJobs.cancelText"),
        onOk: performSave,
      });
      return;
    }

    await performSave();
  };

  const isMobileMode = isMobile();

  return (
    <Page>
      <PageHeader
        agentScoped
        items={[{ title: t("cronJobs.title") }]}
        onRefresh={fetchJobs}
        refreshing={loading}
        extra={
          <div style={{ display: "flex", gap: 8 }}>
            <Tooltip title={t("cronJobs.createJob")}>
              <Btn variant="filled" icon={<Plus />} onClick={handleCreate} />
            </Tooltip>
          </div>
        }
      />

      <PageBody>
        {!loading && jobs.length > 0 && (
          <div className={styles.toolbar}>
            <TextInput
              className={styles.searchInput}
              placeholder={t("cronJobs.searchPlaceholder")}
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
            />
            <Dropdown
              menu={{
                items: [
                  { key: "default-asc", label: t("cronJobs.sortDefault") },
                  { key: "name-asc", label: t("cronJobs.sortByName") + " A→Z" },
                  {
                    key: "name-desc",
                    label: t("cronJobs.sortByName") + " Z→A",
                  },
                  {
                    key: "nextRun-asc",
                    label: t("cronJobs.sortByNextRun") + " ↑",
                  },
                  {
                    key: "nextRun-desc",
                    label: t("cronJobs.sortByNextRun") + " ↓",
                  },
                  {
                    key: "created-desc",
                    label: t("cronJobs.sortByCreated") + " ↓",
                  },
                  {
                    key: "created-asc",
                    label: t("cronJobs.sortByCreated") + " ↑",
                  },
                ],
                selectedKeys: [`${sortField}-${sortOrder}`],
                onClick: ({ key }: { key: string }) => {
                  const idx = key.lastIndexOf("-");
                  setSortField(key.slice(0, idx) as CronJobSortField);
                  setSortOrder(key.slice(idx + 1) as "asc" | "desc");
                },
              }}
              trigger={["click"]}
            >
              <Btn variant="outline" icon={<ArrowUpNarrowWide />} />
            </Dropdown>
          </div>
        )}
        <JobList
          jobs={filteredJobs}
          loading={loading}
          compact={isMobileMode}
          onToggleEnabled={handleToggleEnabled}
          onExecuteNow={handleExecuteNow}
          onEdit={handleEdit}
          onDelete={handleDelete}
          t={t}
        />
      </PageBody>

      <JobDrawer
        open={drawerOpen}
        editingJob={editingJob}
        saving={saving}
        onClose={handleDrawerClose}
        onSubmit={handleSubmit}
        defaultTimezone={userTimezoneRef.current}
      />
    </Page>
  );
}

export default CronJobsPage;
