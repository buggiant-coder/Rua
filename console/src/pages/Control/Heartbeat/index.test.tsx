import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import HeartbeatPage from "./index";

const { apiMock, selectedAgentRef, toastMock } = vi.hoisted(() => ({
  apiMock: {
    getHeartbeatConfig: vi.fn(),
    updateHeartbeatConfig: vi.fn(),
  },
  selectedAgentRef: { current: "heartbeat-agent" },
  toastMock: {
    error: vi.fn(),
    success: vi.fn(),
  },
}));

vi.mock("../../../api", () => ({ default: apiMock }));

vi.mock("../../../stores/agentStore", () => ({
  useAgentStore: () => ({ selectedAgent: selectedAgentRef.current }),
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "heartbeat.activeEnd": "Active end",
        "heartbeat.activeHours": "Active hours",
        "heartbeat.activeStart": "Active start",
        "heartbeat.enabled": "Enabled",
        "heartbeat.every": "Every",
        "heartbeat.loadFailed": "Load failed",
        "heartbeat.saveFailed": "Save failed",
        "heartbeat.saveSuccess": "Saved",
        "heartbeat.target": "Target",
        "heartbeat.targetLast": "Last session",
        "heartbeat.targetMain": "Main session",
        "heartbeat.title": "Heartbeat",
        "heartbeat.unitHours": "Hours",
        "heartbeat.unitMinutes": "Minutes",
      })[key] ?? key,
  }),
}));

vi.mock("@/ui", () => ({
  Btn: ({
    "aria-label": ariaLabel,
    children,
    disabled,
    loading,
    onClick,
  }: {
    "aria-label"?: string;
    children?: React.ReactNode;
    disabled?: boolean;
    loading?: boolean;
    onClick?: () => void;
  }) => (
    <button
      aria-label={ariaLabel}
      disabled={disabled || loading}
      type="button"
      onClick={onClick}
    >
      {children ?? "action"}
    </button>
  ),
  Dropdown: ({ children }: { children: React.ReactNode }) => <>{children}</>,
  FormField: ({
    children,
    label,
  }: {
    children: React.ReactNode;
    label: React.ReactNode;
  }) => (
    <label>
      <span>{label}</span>
      {children}
    </label>
  ),
  NumberInput: ({
    className,
    min,
    onChange,
    value,
  }: {
    className?: string;
    min?: number;
    onChange: (value: number) => void;
    value: number;
  }) => (
    <input
      className={className}
      min={min}
      type="number"
      value={value}
      onChange={(event) => onChange(Number(event.target.value))}
    />
  ),
  Select: ({
    className,
    onChange,
    options,
    value,
  }: {
    className?: string;
    onChange: (value: string) => void;
    options: Array<{ label: string; value: string }>;
    value: string;
  }) => (
    <select
      className={className}
      value={value}
      onChange={(event) => onChange(event.target.value)}
    >
      {options.map((option) => (
        <option key={option.value} value={option.value}>
          {option.label}
        </option>
      ))}
    </select>
  ),
  TimePicker: ({
    onChange,
    value,
  }: {
    onChange: (_: unknown, value: string) => void;
    value: { format?: (format: string) => string } | string | null;
  }) => {
    const displayValue =
      value && typeof value === "object" && "format" in value
        ? value.format?.("HH:mm")
        : value ?? "";
    return (
      <input
        aria-label="time"
        value={displayValue ?? ""}
        onChange={(event) => onChange(null, event.target.value)}
      />
    );
  },
  Toggle: ({
    checked,
    disabled,
    onChange,
  }: {
    checked: boolean;
    disabled?: boolean;
    onChange: (event: React.ChangeEvent<HTMLInputElement>) => void;
  }) => (
    <input
      checked={checked}
      disabled={disabled}
      type="checkbox"
      onChange={onChange}
    />
  ),
  toast: toastMock,
}));

describe("HeartbeatPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    selectedAgentRef.current = "heartbeat-agent";
    apiMock.getHeartbeatConfig.mockResolvedValue({
      activeHours: { end: "18:00", start: "09:00" },
      enabled: true,
      every: "90m",
      target: "last",
    });
    apiMock.updateHeartbeatConfig.mockResolvedValue({});
  });

  it("loads the Rust heartbeat config and parses interval plus active hours", async () => {
    render(<HeartbeatPage />);

    await screen.findByText("Heartbeat");

    expect(apiMock.getHeartbeatConfig).toHaveBeenCalledTimes(1);
    expect(screen.getByLabelText("Enabled")).toBeChecked();
    expect(screen.getByLabelText("Every")).toHaveValue(90);
    expect(screen.getByLabelText("Every")).toHaveDisplayValue("90");
    expect(screen.getByDisplayValue("Minutes")).toBeInTheDocument();
    expect(screen.getByLabelText("Target")).toHaveValue("last");
    expect(screen.getByLabelText("Active hours")).toBeChecked();
    expect(screen.getByDisplayValue("09:00")).toBeInTheDocument();
    expect(screen.getByDisplayValue("18:00")).toBeInTheDocument();
  });

  it("persists enabled changes through the root/header-selected heartbeat API", async () => {
    apiMock.getHeartbeatConfig.mockResolvedValueOnce({
      activeHours: null,
      enabled: false,
      every: "30m",
      target: "main",
    });
    render(<HeartbeatPage />);

    const enabled = await screen.findByLabelText("Enabled");
    fireEvent.click(enabled);

    await waitFor(() => {
      expect(apiMock.updateHeartbeatConfig).toHaveBeenCalledWith({
        activeHours: undefined,
        enabled: true,
        every: "30m",
        target: "main",
      });
    });
    expect(toastMock.success).toHaveBeenCalledWith("Saved");
  });

  it("serializes edited interval and active-hours values on save", async () => {
    render(<HeartbeatPage />);
    await screen.findByText("Heartbeat");

    fireEvent.change(screen.getByLabelText("Every"), {
      target: { value: "3" },
    });
    fireEvent.change(screen.getAllByRole("combobox")[0], {
      target: { value: "h" },
    });
    fireEvent.change(screen.getByLabelText("Target"), {
      target: { value: "main" },
    });
    const [start, end] = screen.getAllByLabelText("time");
    fireEvent.change(start, { target: { value: "07:00" } });
    fireEvent.change(end, { target: { value: "21:00" } });
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));

    await waitFor(() => {
      expect(apiMock.updateHeartbeatConfig).toHaveBeenCalledWith({
        activeHours: { end: "21:00", start: "07:00" },
        enabled: true,
        every: "3h",
        target: "main",
      });
    });
  });

  it("reloads when the selected agent changes", async () => {
    const { rerender } = render(<HeartbeatPage />);
    await screen.findByText("Heartbeat");

    selectedAgentRef.current = "other-agent";
    rerender(<HeartbeatPage />);

    await waitFor(() => {
      expect(apiMock.getHeartbeatConfig).toHaveBeenCalledTimes(2);
    });
  });
});
