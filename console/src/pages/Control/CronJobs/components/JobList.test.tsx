import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { JobList } from "./JobList";
import type { CronJobSpecOutput } from "../../../../api/types";

vi.mock("@/components/EmptyState", () => ({
  EmptyState: ({ description }: { description: string }) => (
    <div>{description}</div>
  ),
}));

vi.mock("@/ui", () => ({
  Btn: ({ onClick }: { onClick?: () => void }) => (
    <button type="button" onClick={onClick}>
      action
    </button>
  ),
  StackedList: ({
    children,
    role,
  }: {
    children: React.ReactNode;
    role?: string;
  }) => <div role={role}>{children}</div>,
  Toggle: ({
    checked,
    onChange,
  }: {
    checked: boolean;
    onChange: () => void;
  }) => <input checked={checked} type="checkbox" onChange={onChange} />,
  Tooltip: ({ children }: { children: React.ReactNode }) => <>{children}</>,
}));

const labels: Record<string, string> = {
  "common.enable": "Enable",
  "cronJobs.cronDayFri": "Fri",
  "cronJobs.cronDayMon": "Mon",
  "cronJobs.cronDaySat": "Sat",
  "cronJobs.cronDaySun": "Sun",
  "cronJobs.cronDayThu": "Thu",
  "cronJobs.cronDayTue": "Tue",
  "cronJobs.cronDayWed": "Wed",
  "cronJobs.cronTypeDaily": "Daily",
  "cronJobs.cronTypeHourly": "Hourly",
  "cronJobs.cronTypeWeekly": "Weekly",
  "cronJobs.delete": "Delete",
  "cronJobs.disable": "Disable",
  "cronJobs.edit": "Edit",
  "cronJobs.emptyState": "No cron jobs",
  "cronJobs.executeNow": "Execute now",
  "cronJobs.nextRun": "Next",
  "cronJobs.nextRunDisabled": "Disabled",
  "cronJobs.summaryEmpty": "No task",
};

const t = ((key: string) => labels[key] ?? key) as any;

function cronJob(overrides: Partial<CronJobSpecOutput>): CronJobSpecOutput {
  return {
    dispatch: {
      channel: "console",
      mode: "stream",
      target: { session_id: "session-a", user_id: "user-a" },
      type: "channel",
    },
    enabled: true,
    id: "daily",
    name: "Daily standup",
    schedule: { cron: "30 9 * * *", timezone: "Asia/Shanghai", type: "cron" },
    task_type: "text",
    text: "Summarize the workspace",
    ...overrides,
  };
}

describe("JobList", () => {
  it("renders the empty Rust cron list state", () => {
    render(
      <JobList
        jobs={[]}
        loading={false}
        onDelete={vi.fn()}
        onEdit={vi.fn()}
        onExecuteNow={vi.fn()}
        onToggleEnabled={vi.fn()}
        t={t}
      />,
    );

    expect(screen.getByText("No cron jobs")).toBeVisible();
  });

  it("describes Rust cron jobs with schedule, timezone, and task summary", () => {
    render(
      <JobList
        jobs={[
          cronJob({ id: "daily", name: "Daily standup" }),
          cronJob({
            id: "weekly",
            name: "Weekly report",
            request: {
              input: {
                messages: [
                  {
                    content:
                      "Prepare a very long weekly summary for the current project",
                    role: "user",
                  },
                ],
              },
            },
            schedule: { cron: "45 18 * * 5", type: "cron" },
            task_type: "agent",
            text: undefined,
          }),
        ]}
        loading={false}
        onDelete={vi.fn()}
        onEdit={vi.fn()}
        onExecuteNow={vi.fn()}
        onToggleEnabled={vi.fn()}
        t={t}
      />,
    );

    expect(screen.getByText("Daily standup")).toBeVisible();
    expect(screen.getByText("Daily 09:30")).toBeVisible();
    expect(screen.getByText("Asia/Shanghai")).toBeVisible();
    expect(screen.getByText("Summarize the workspace")).toBeVisible();

    expect(screen.getByText("Weekly report")).toBeVisible();
    expect(screen.getByText("Weekly Fri 18:45")).toBeVisible();
    expect(screen.getByText(/\{"messages":/)).toBeVisible();
    expect(screen.getByText(/…$/)).toBeVisible();
  });

  it("delegates toggle, execute, edit, and delete actions for a job", () => {
    const onDelete = vi.fn();
    const onEdit = vi.fn();
    const onExecuteNow = vi.fn();
    const onToggleEnabled = vi.fn();
    const job = cronJob({ id: "daily" });

    render(
      <JobList
        jobs={[job]}
        loading={false}
        onDelete={onDelete}
        onEdit={onEdit}
        onExecuteNow={onExecuteNow}
        onToggleEnabled={onToggleEnabled}
        t={t}
      />,
    );

    fireEvent.click(screen.getByRole("checkbox"));
    const [executeButton, editButton, deleteButton] =
      screen.getAllByRole("button");
    fireEvent.click(executeButton);
    fireEvent.click(editButton);
    fireEvent.click(deleteButton);

    expect(onToggleEnabled).toHaveBeenCalledWith(job);
    expect(onExecuteNow).toHaveBeenCalledWith(job);
    expect(onEdit).toHaveBeenCalledWith(job);
    expect(onDelete).toHaveBeenCalledWith("daily");
  });
});
