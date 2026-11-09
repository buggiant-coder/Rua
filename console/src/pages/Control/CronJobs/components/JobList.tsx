import type { TFunction } from "i18next";
import type { CronJobSpecOutput } from "../../../../api/types";
import { EmptyState } from "@/components/EmptyState";
import { parseCron } from "./parseCron";
import { formatNextRun } from "./nextRunTime";
import styles from "../index.module.less";
import { Pencil, PlayCircle, Trash2 } from "lucide-react";
import { Btn, StackedList, Toggle, Tooltip } from "@/ui";
type CronJob = CronJobSpecOutput;

interface JobListProps {
  jobs: CronJob[];
  loading: boolean;
  compact?: boolean;
  onToggleEnabled: (job: CronJob) => void;
  onExecuteNow: (job: CronJob) => void;
  onEdit: (job: CronJob) => void;
  onDelete: (jobId: string) => void;
  t: TFunction;
}

function describeCron(cron: string, t: TFunction): string {
  const parts = parseCron(cron || "0 9 * * *");
  const pad = (n: number | undefined) => String(n ?? 0).padStart(2, "0");

  switch (parts.type) {
    case "hourly":
      return t("cronJobs.cronTypeHourly");
    case "daily":
      return `${t("cronJobs.cronTypeDaily")} ${pad(parts.hour)}:${pad(
        parts.minute,
      )}`;
    case "weekly": {
      const dayMap: Record<string, string> = {
        mon: t("cronJobs.cronDayMon"),
        tue: t("cronJobs.cronDayTue"),
        wed: t("cronJobs.cronDayWed"),
        thu: t("cronJobs.cronDayThu"),
        fri: t("cronJobs.cronDayFri"),
        sat: t("cronJobs.cronDaySat"),
        sun: t("cronJobs.cronDaySun"),
      };
      const days = (parts.daysOfWeek || [])
        .map((d) => dayMap[d] || d)
        .join(",");
      return `${t("cronJobs.cronTypeWeekly")} ${days} ${pad(parts.hour)}:${pad(
        parts.minute,
      )}`;
    }
    case "custom":
      return cron;
    default:
      return cron;
  }
}

function describeSchedule(job: CronJob, t: TFunction): string {
  const schedule = job.schedule as any;
  if (schedule?.type === "once") {
    const when = schedule.run_at ? new Date(schedule.run_at) : null;
    const text =
      when && !Number.isNaN(when.valueOf()) ? when.toLocaleString() : "";
    return text
      ? `${t("cronJobs.scheduleTypeOnce")} ${text}`
      : t("cronJobs.scheduleTypeOnce");
  }
  return describeCron(schedule?.cron || "", t);
}

function formatScheduleNextRun(job: CronJob, t: TFunction): string {
  const schedule = job.schedule as any;
  if (schedule?.type === "once") {
    if (!job.enabled || !schedule.run_at) return t("cronJobs.nextRunNone");
    const when = new Date(schedule.run_at);
    return Number.isNaN(when.valueOf())
      ? t("cronJobs.nextRunNone")
      : when.toLocaleString();
  }
  return formatNextRun(schedule?.cron || "", !!job.enabled, t);
}

function summarizeJob(
  job: CronJob,
  t: TFunction,
): { tag: string | null; text: string } {
  const taskType = (job.task_type || "").toLowerCase();
  if (taskType === "text" || (!taskType && job.text)) {
    return { tag: null, text: job.text || "" };
  }
  if (job.request?.input) {
    let raw: string;
    try {
      raw = JSON.stringify(job.request.input);
    } catch {
      raw = String(job.request.input);
    }

    const text = raw.length > 50 ? raw.slice(0, 50) + "…" : raw;
    return { tag: null, text };
  }
  return { tag: null, text: t("cronJobs.summaryEmpty") };
}

export function JobList({
  jobs,
  loading,
  compact = false,
  onToggleEnabled,
  onExecuteNow,
  onEdit,
  onDelete,
  t,
}: JobListProps) {
  if (!loading && jobs.length === 0) {
    return (
      <div className={styles.listEmpty}>
        <EmptyState description={t("cronJobs.emptyState")} />
      </div>
    );
  }

  return (
    <StackedList
      className={`${styles.list} ${compact ? styles.listCompact : ""}`}
      role="list"
    >
      {jobs.map((job) => {
        const cronText = describeSchedule(job, t);
        const tz = job.schedule?.timezone;
        const nextRun = formatScheduleNextRun(job, t);
        const summary = summarizeJob(job, t);

        const hasSummary = Boolean(summary.tag || summary.text);

        return (
          <div key={job.id} className={styles.listItem} role="listitem">
            {}
            <div className={styles.listItemActions}>
              <Tooltip
                title={job.enabled ? t("cronJobs.disable") : t("common.enable")}
              >
                <Toggle
                  checked={job.enabled}
                  onChange={() => onToggleEnabled(job)}
                />
              </Tooltip>
              <Tooltip title={t("cronJobs.executeNow")}>
                <Btn
                  variant="outline"
                  size="sm"
                  icon={<PlayCircle />}
                  onClick={() => onExecuteNow(job)}
                />
              </Tooltip>
              <Tooltip title={t("cronJobs.edit")}>
                <Btn
                  variant="outline"
                  size="sm"
                  icon={<Pencil />}
                  onClick={() => onEdit(job)}
                />
              </Tooltip>
              <Tooltip title={t("cronJobs.delete")}>
                <Btn
                  variant="danger"
                  size="sm"
                  icon={<Trash2 />}
                  onClick={() => onDelete(job.id)}
                />
              </Tooltip>
            </div>

            {}
            <div className={styles.listItemName} title={job.name}>
              {job.name}
            </div>

            {}
            <div className={styles.listItemMeta}>
              <span className={styles.listItemMetaCron}>{cronText}</span>
              <span className={styles.listItemMetaSeparator}>·</span>
              <span className={styles.listItemMetaNext}>
                <span className={styles.listItemMetaNextLabel}>
                  {t("cronJobs.nextRun")}:
                </span>{" "}
                {nextRun}
              </span>
              {tz && (
                <>
                  <span className={styles.listItemMetaSeparator}>·</span>
                  <span className={styles.listItemMetaTz}>{tz}</span>
                </>
              )}
            </div>

            {}
            {hasSummary && (
              <div className={styles.listItemSummary}>
                <span className={styles.listItemSummaryText}>
                  {summary.text || "—"}
                </span>
              </div>
            )}
          </div>
        );
      })}
    </StackedList>
  );
}
