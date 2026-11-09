import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import CronJobsPage from "./index";

const {
  createJobMock,
  deleteJobMock,
  executeNowMock,
  fetchJobsMock,
  getUserTimezoneMock,
  modalConfirmMock,
  toggleEnabledMock,
  updateJobMock,
} = vi.hoisted(() => ({
  createJobMock: vi.fn(),
  deleteJobMock: vi.fn(),
  executeNowMock: vi.fn(),
  fetchJobsMock: vi.fn(),
  getUserTimezoneMock: vi.fn(),
  modalConfirmMock: vi.fn(),
  toggleEnabledMock: vi.fn(),
  updateJobMock: vi.fn(),
}));

vi.mock("../../../api", () => ({
  default: {
    getUserTimezone: getUserTimezoneMock,
  },
}));

vi.mock("@/utils/isMobile", () => ({
  isMobile: () => false,
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, params?: unknown) => {
      if (key === "cronJobs.executeNowContent") {
        const name =
          params && typeof params === "object" && "name" in params
            ? String((params as { name: unknown }).name)
            : "";
        return `Execute ${name}?`;
      }
      return (
        {
          "common.refresh": "Refresh",
          "common.save": "Save",
          "cronJobs.cancelText": "Cancel",
          "cronJobs.confirmDelete": "Delete cron job",
          "cronJobs.createJob": "Create job",
          "cronJobs.deleteConfirm": "Delete this job?",
          "cronJobs.deleteConfirmEnabled": "Delete enabled job?",
          "cronJobs.deleteText": "Delete",
          "cronJobs.executeNowConfirm": "Run",
          "cronJobs.executeNowTitle": "Run now",
          "cronJobs.rescheduleConfirmContent": "Reschedule enabled job?",
          "cronJobs.rescheduleConfirmTitle": "Reschedule",
          "cronJobs.searchPlaceholder": "Search jobs",
          "cronJobs.sortByCreated": "Created",
          "cronJobs.sortByName": "Name",
          "cronJobs.sortByNextRun": "Next run",
          "cronJobs.sortDefault": "Default",
          "cronJobs.title": "Cron Jobs",
        }[key] ?? key
      );
    },
  }),
}));

vi.mock("@/ui", () => ({
  Btn: ({
    children,
    disabled,
    loading,
    onClick,
  }: {
    children?: React.ReactNode;
    disabled?: boolean;
    loading?: boolean;
    onClick?: () => void;
  }) => (
    <button disabled={disabled || loading} type="button" onClick={onClick}>
      {children ?? "icon-action"}
    </button>
  ),
  Dropdown: ({ children }: { children: React.ReactNode }) => <>{children}</>,
  Modal: {
    confirm: modalConfirmMock,
  },
  TextInput: ({
    onChange,
    placeholder,
    value,
  }: {
    onChange: (event: React.ChangeEvent<HTMLInputElement>) => void;
    placeholder?: string;
    value: string;
  }) => <input placeholder={placeholder} value={value} onChange={onChange} />,
  Tooltip: ({ children }: { children: React.ReactNode }) => <>{children}</>,
}));

vi.mock("./components", () => ({
  DEFAULT_FORM_VALUES: {
    dispatch: {
      channel: "console",
      mode: "stream",
      target: { session_id: "default", user_id: "default" },
      type: "channel",
    },
    runtime: {
      max_concurrency: 1,
      misfire_grace_seconds: 60,
      timeout_seconds: 300,
    },
    schedule: { cron: "0 9 * * *", timezone: "UTC", type: "cron" },
  },
  JobDrawer: ({
    defaultTimezone,
    editingJob,
    onSubmit,
    open,
  }: {
    defaultTimezone?: string;
    editingJob: { name: string } | null;
    onSubmit: (values: Record<string, unknown>) => void;
    open: boolean;
  }) =>
    open ? (
      <div>
        <div>drawer timezone:{defaultTimezone}</div>
        <div>editing:{editingJob?.name ?? "new"}</div>
        <button
          type="button"
          onClick={() =>
            onSubmit({
              cronTime: { hour: () => 10, minute: () => 15 },
              cronType: "daily",
              enabled: true,
              name: "Morning report",
              request: { input: "stale request" },
              schedule: { timezone: "Asia/Shanghai", type: "cron" },
              task_type: "text",
              text: "Summarize today",
            })
          }
        >
          Submit drawer
        </button>
      </div>
    ) : null,
  JobList: ({
    jobs,
    onDelete,
    onEdit,
    onExecuteNow,
    onToggleEnabled,
  }: {
    jobs: Array<{ id: string; name: string }>;
    onDelete: (id: string) => void;
    onEdit: (job: { id: string; name: string }) => void;
    onExecuteNow: (job: { id: string; name: string }) => void;
    onToggleEnabled: (job: { id: string; name: string }) => void;
  }) => (
    <div>
      {jobs.map((job) => (
        <div key={job.id}>
          <span>{job.name}</span>
          <button type="button" onClick={() => onToggleEnabled(job)}>
            Toggle {job.name}
          </button>
          <button type="button" onClick={() => onExecuteNow(job)}>
            Run {job.name}
          </button>
          <button type="button" onClick={() => onEdit(job)}>
            Edit {job.name}
          </button>
          <button type="button" onClick={() => onDelete(job.id)}>
            Delete {job.name}
          </button>
        </div>
      ))}
    </div>
  ),
  useCronJobs: () => ({
    createJob: createJobMock,
    deleteJob: deleteJobMock,
    executeNow: executeNowMock,
    fetchJobs: fetchJobsMock,
    filteredJobs: [
      {
        enabled: true,
        id: "enabled-job",
        name: "Enabled job",
        schedule: { cron: "0 9 * * *", type: "cron" },
        task_type: "text",
        text: "hello",
      },
    ],
    jobs: [
      {
        enabled: true,
        id: "enabled-job",
        name: "Enabled job",
        schedule: { cron: "0 9 * * *", type: "cron" },
        task_type: "text",
        text: "hello",
      },
    ],
    loading: false,
    searchQuery: "",
    setSearchQuery: vi.fn(),
    setSortField: vi.fn(),
    setSortOrder: vi.fn(),
    sortField: "default",
    sortOrder: "asc",
    toggleEnabled: toggleEnabledMock,
    updateJob: updateJobMock,
  }),
}));

describe("CronJobsPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    createJobMock.mockResolvedValue(true);
    deleteJobMock.mockResolvedValue(true);
    executeNowMock.mockResolvedValue(true);
    getUserTimezoneMock.mockResolvedValue({ timezone: "Asia/Shanghai" });
    modalConfirmMock.mockImplementation(({ onOk }) => onOk?.());
    toggleEnabledMock.mockResolvedValue(true);
    updateJobMock.mockResolvedValue(true);
  });

  it("creates Rust cron jobs with serialized schedule and text-task request cleanup", async () => {
    render(<CronJobsPage />);

    await waitFor(() => {
      expect(getUserTimezoneMock).toHaveBeenCalled();
    });
    await act(async () => {
      await Promise.resolve();
    });
    fireEvent.click(screen.getAllByRole("button", { name: "icon-action" })[0]);
    expect(screen.getByText("drawer timezone:Asia/Shanghai")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Submit drawer" }));

    await waitFor(() => {
      expect(createJobMock).toHaveBeenCalledWith(
        expect.objectContaining({
          enabled: true,
          id: "",
          name: "Morning report",
          schedule: expect.objectContaining({
            cron: "15 10 * * *",
            timezone: "Asia/Shanghai",
          }),
          task_type: "text",
          text: "Summarize today",
        }),
      );
    });
    expect(createJobMock.mock.calls[0][0]).not.toHaveProperty("request");
  });

  it("confirms enabled edits before updating the existing Rust job id", async () => {
    render(<CronJobsPage />);

    fireEvent.click(screen.getByRole("button", { name: "Edit Enabled job" }));
    expect(screen.getByText("editing:Enabled job")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Submit drawer" }));

    expect(modalConfirmMock).toHaveBeenCalledWith(
      expect.objectContaining({
        content: "Reschedule enabled job?",
        title: "Reschedule",
      }),
    );
    await waitFor(() => {
      expect(updateJobMock).toHaveBeenCalledWith(
        "enabled-job",
        expect.objectContaining({ id: "enabled-job" }),
      );
    });
  });

  it("delegates toggle, execute, and delete through the cron hook with confirmations", async () => {
    render(<CronJobsPage />);

    fireEvent.click(screen.getByRole("button", { name: "Toggle Enabled job" }));
    fireEvent.click(screen.getByRole("button", { name: "Run Enabled job" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete Enabled job" }));

    expect(toggleEnabledMock).toHaveBeenCalledWith(
      expect.objectContaining({ id: "enabled-job" }),
    );
    expect(modalConfirmMock).toHaveBeenCalledWith(
      expect.objectContaining({
        content: "Execute Enabled job?",
        title: "Run now",
      }),
    );
    expect(modalConfirmMock).toHaveBeenCalledWith(
      expect.objectContaining({
        content: "Delete enabled job?",
        title: "Delete cron job",
      }),
    );
    await waitFor(() => {
      expect(executeNowMock).toHaveBeenCalledWith("enabled-job");
      expect(deleteJobMock).toHaveBeenCalledWith("enabled-job");
    });
  });
});
