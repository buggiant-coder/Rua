import dayjs from "dayjs";
import { parseCron } from "./parseCron";

const DAY_INDEX: Record<string, number> = {
  sun: 0,
  mon: 1,
  tue: 2,
  wed: 3,
  thu: 4,
  fri: 5,
  sat: 6,
};

export function computeNextRun(cron: string): dayjs.Dayjs | null {
  if (!cron) return null;
  const parts = parseCron(cron);
  const now = dayjs();

  switch (parts.type) {
    case "hourly": {
      const next = now.minute(0).second(0).millisecond(0);
      if (next.isAfter(now)) return next;
      return next.add(1, "hour");
    }

    case "daily": {
      const h = parts.hour ?? 9;
      const m = parts.minute ?? 0;
      const candidate = now.hour(h).minute(m).second(0).millisecond(0);
      if (candidate.isAfter(now)) return candidate;
      return candidate.add(1, "day");
    }

    case "weekly": {
      const h = parts.hour ?? 9;
      const m = parts.minute ?? 0;
      const days = (parts.daysOfWeek || [])
        .map((d) => DAY_INDEX[d])
        .filter((n): n is number => typeof n === "number");
      if (days.length === 0) return null;

      let best: dayjs.Dayjs | null = null;
      for (let offset = 0; offset < 7; offset++) {
        const cand = now
          .add(offset, "day")
          .hour(h)
          .minute(m)
          .second(0)
          .millisecond(0);
        if (!days.includes(cand.day())) continue;
        if (cand.isAfter(now) && (!best || cand.isBefore(best))) {
          best = cand;
        }
      }

      if (!best) {
        for (let offset = 0; offset < 14; offset++) {
          const cand = now
            .add(offset, "day")
            .hour(h)
            .minute(m)
            .second(0)
            .millisecond(0);
          if (!days.includes(cand.day())) continue;
          if (cand.isAfter(now)) {
            best = cand;
            break;
          }
        }
      }
      return best;
    }

    case "custom":
    default:
      return null;
  }
}

export function formatNextRun(
  cron: string,
  enabled: boolean,
  t: (key: string, opts?: Record<string, unknown>) => string,
): string {
  if (!enabled) return t("cronJobs.nextRunPaused");
  const next = computeNextRun(cron);
  if (!next) return "—";

  const now = dayjs();
  const diffMin = next.diff(now, "minute");
  const diffHour = next.diff(now, "hour");
  const diffDay = next.diff(now, "day");

  if (diffMin < 1) return t("cronJobs.nextRunImminent");
  if (diffMin < 60) return t("cronJobs.nextRunInMinutes", { count: diffMin });
  if (diffHour < 24) return t("cronJobs.nextRunInHours", { count: diffHour });
  if (diffDay < 7) {
    return `${next.format("ddd HH:mm")} · ${t("cronJobs.nextRunInDays", {
      count: diffDay,
    })}`;
  }
  return next.format("YYYY-MM-DD HH:mm");
}
