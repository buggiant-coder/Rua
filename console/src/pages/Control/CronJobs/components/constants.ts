import dayjs from "dayjs";

export { TIMEZONE_OPTIONS } from "../../../../constants/timezone";

function currentUserId(): string {
  return "default";
}

function activeChatIdFromLocation(): string {
  if (typeof window === "undefined") return "default";
  const match = window.location.pathname.match(/^\/chat\/([^/?#]+)/);
  const raw = match?.[1];
  if (!raw || raw === "undefined" || raw === "null") return "default";
  return decodeURIComponent(raw);
}

export const DEFAULT_FORM_VALUES = {
  enabled: false,
  schedule: {
    type: "cron" as const,
    cron: "0 9 * * *",
    timezone: "UTC",
    run_at: undefined,
    repeat_every_days: undefined,
    repeat_end_type: undefined,
    repeat_until: undefined,
    repeat_count: undefined,
  },
  scheduleMode: "cron" as const,
  cronType: "daily",
  cronTime: dayjs().hour(9).minute(0),
  onceDate: dayjs().add(1, "day").format("YYYY-MM-DD"),
  onceTime: "09:00",
  repeatEnabled: false,
  repeatEveryDays: 1,
  repeatEndType: "never" as const,
  repeatUntilDate: dayjs().add(30, "day").format("YYYY-MM-DD"),
  repeatCount: 3,
  task_type: "agent" as const,
  save_result_to_inbox: undefined,
  dispatch: {
    type: "channel" as const,
    channel: "console",
    target: {
      user_id: currentUserId(),
      session_id: activeChatIdFromLocation(),
    },
    mode: "final" as const,
  },
  runtime: {
    max_concurrency: 1,
    timeout_seconds: 120,
    misfire_grace_seconds: 600,
    share_session: true,
    tool_safety: false,
  },
};
