import { fireEvent, render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import AgentConfigPage from "./index";

const {
  fetchConfigMock,
  handleLanguageChangeMock,
  handleResetMock,
  handleSaveMock,
  handleTimezoneChangeMock,
  useAgentConfigMock,
} = vi.hoisted(() => ({
  fetchConfigMock: vi.fn(),
  handleLanguageChangeMock: vi.fn(),
  handleResetMock: vi.fn(),
  handleSaveMock: vi.fn(),
  handleTimezoneChangeMock: vi.fn(),
  useAgentConfigMock: vi.fn(),
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "agentConfig.title": "Agent config",
        "chat.options.agentRuntime": "Agent chat runtime",
        "chat.options.contextCompaction": "Context compaction",
        "chat.options.aggressiveContext": "Aggressive context management",
        "chat.options.tokenEstimation": "Token estimation",
        "agentConfig.otherRunningConfig": "Other running configuration",
        "agentConfig.showDebugMenu": "Show debug menu",
        "agentConfig.showDebugMenuTooltip":
          "Show Debug Capture controls in Chat menus.",
        "agentConfig.resetToApplicationDefaults":
          "Reset to application defaults",
        "common.reset": "Reset",
        "common.save": "Save",
        "environments.retry": "Retry",
      })[key] ?? key,
  }),
}));

vi.mock("lucide-react", () => ({
  ChevronDown: () => <span aria-hidden>chevron</span>,
  RefreshCw: () => <span data-testid="reset-icon">reset</span>,
  RotateCcw: () => <span data-testid="reset-icon">reset</span>,
  Save: () => <span data-testid="save-icon">save</span>,
}));

vi.mock("@/components/Page", () => ({
  Page: ({ children }: { children?: ReactNode }) => <main>{children}</main>,
  PageBody: ({ children }: { children?: ReactNode }) => (
    <section>{children}</section>
  ),
}));

vi.mock("@/components/PageHeader", () => ({
  PageHeader: ({
    current,
    extra,
    onRefresh,
    refreshing,
  }: {
    current?: ReactNode;
    extra?: ReactNode;
    onRefresh?: () => void;
    refreshing?: boolean;
  }) => (
    <header>
      <h1>{current}</h1>
      {extra}
      <button
        aria-label="Refresh"
        disabled={refreshing}
        type="button"
        onClick={onRefresh}
      >
        Refresh
      </button>
    </header>
  ),
}));

vi.mock("@/components/Skeleton", () => ({
  default: ({ count, variant }: { count?: number; variant?: string }) => (
    <div data-testid="skeleton">
      {variant}:{count}
    </div>
  ),
}));

vi.mock("@/components/RunningConfigOptions", () => ({
  ChatOptionsLayout: ({ children }: { children?: ReactNode }) => (
    <div>{children}</div>
  ),
  ChatOptionRow: ({
    children,
    label,
  }: {
    children?: ReactNode;
    label: ReactNode;
  }) => (
    <div>
      <span>{label}</span>
      {children}
    </div>
  ),
  RunningConfigOptions: ({
    onToggleSection,
    openSections,
    runtimeExtras,
  }: {
    onToggleSection: (section: string) => void;
    openSections: { runtime: boolean };
    runtimeExtras?: ReactNode;
  }) => (
    <section>
      <button type="button" onClick={() => onToggleSection("runtime")}>
        Agent chat runtime
      </button>
      {openSections.runtime ? runtimeExtras : null}
    </section>
  ),
  ChatOptionSection: ({
    children,
    onToggle,
    open,
    title,
  }: {
    children?: ReactNode;
    onToggle: () => void;
    open: boolean;
    title: ReactNode;
  }) => (
    <section>
      <button type="button" aria-expanded={open} onClick={onToggle}>
        {title}
      </button>
      {open ? children : null}
    </section>
  ),
}));

vi.mock("@/ui", () => ({
  Btn: ({
    children,
    disabled,
    icon,
    loading,
    onClick,
  }: {
    children?: ReactNode;
    disabled?: boolean;
    icon?: ReactNode;
    loading?: boolean;
    onClick?: () => void;
  }) => (
    <button disabled={disabled || loading} type="button" onClick={onClick}>
      {children ?? icon}
    </button>
  ),
  EnableDisableSwitch: () => <div>EnableDisableSwitch</div>,
  Slider: () => <div>Slider</div>,
  Tooltip: ({ children }: { children?: ReactNode }) => <>{children}</>,
}));

vi.mock("./components", () => ({
  ContextCompactCard: ({ section }: { section?: string }) => (
    <div>ContextCompactCard:{section}</div>
  ),
  EmbeddingConfigCard: () => <div>EmbeddingConfigCard</div>,
  LlmRateLimiterCard: () => <div>LlmRateLimiterCard</div>,
  LlmRetryCard: ({ llmRetryEnabled }: { llmRetryEnabled: boolean }) => (
    <div>LlmRetryCard:{String(llmRetryEnabled)}</div>
  ),
  MemorySummaryCard: () => <div>MemorySummaryCard</div>,
  ReactAgentCard: ({
    language,
    onLanguageChange,
    onTimezoneChange,
    savingLang,
    savingTimezone,
    timezone,
  }: {
    language: string;
    onLanguageChange: (language: string) => void;
    onTimezoneChange: (timezone: string) => void;
    savingLang: boolean;
    savingTimezone: boolean;
    timezone: string;
  }) => (
    <div>
      <span>
        ReactAgentCard:{language}:{timezone}:{String(savingLang)}:
        {String(savingTimezone)}
      </span>
      <button type="button" onClick={() => onLanguageChange("zh-CN")}>
        Change language
      </button>
      <button type="button" onClick={() => onTimezoneChange("UTC")}>
        Change timezone
      </button>
    </div>
  ),
  ToolResultCompactCard: ({ section }: { section?: string }) => (
    <div>ToolResultCompactCard:{section}</div>
  ),
}));

vi.mock("./useAgentConfig.tsx", async () => {
  const actual = await vi.importActual<typeof import("./useAgentConfig.tsx")>(
    "./useAgentConfig.tsx",
  );
  return {
    ...actual,
    useAgentConfig: useAgentConfigMock,
  };
});

function configState(overrides: Record<string, unknown> = {}) {
  return {
    error: null,
    fetchConfig: fetchConfigMock,
    handleLanguageChange: handleLanguageChangeMock,
    handleReset: handleResetMock,
    handleSave: handleSaveMock,
    handleTimezoneChange: handleTimezoneChangeMock,
    language: "en",
    loading: false,
    saving: false,
    resetting: false,
    savingLang: false,
    savingTimezone: false,
    timezone: "Asia/Shanghai",
    ...overrides,
  };
}

describe("AgentConfigPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    useAgentConfigMock.mockReturnValue(configState());
  });

  it("renders the loading skeleton while selected-agent config is loading", () => {
    useAgentConfigMock.mockReturnValue(configState({ loading: true }));

    render(<AgentConfigPage />);

    expect(screen.getByTestId("skeleton")).toHaveTextContent("detail:3");
  });

  it("renders load errors with a retry action", () => {
    useAgentConfigMock.mockReturnValue(
      configState({ error: "failed to load config" }),
    );

    render(<AgentConfigPage />);

    expect(screen.getByText("failed to load config")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(fetchConfigMock).toHaveBeenCalledTimes(1);
  });

  it("wires the page chrome and Rust runtime cards to hook callbacks", () => {
    render(<AgentConfigPage />);

    expect(screen.getByRole("heading", { name: "Agent config" })).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Agent chat runtime" }));
    expect(
      screen.getByText("ReactAgentCard:en:Asia/Shanghai:false:false"),
    ).toBeVisible();
    expect(screen.getByText("LlmRetryCard:true")).toBeVisible();
    expect(screen.getByText("Show debug menu")).toBeVisible();

    fireEvent.click(
      screen.getByRole("button", { name: "Other running configuration" }),
    );
    expect(screen.getByText("MemorySummaryCard")).toBeVisible();
    expect(screen.getByText("EmbeddingConfigCard")).toBeVisible();

    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    fireEvent.click(screen.getByTestId("reset-icon").closest("button")!);
    fireEvent.click(screen.getByTestId("save-icon").closest("button")!);
    fireEvent.click(screen.getByRole("button", { name: "Change language" }));
    fireEvent.click(screen.getByRole("button", { name: "Change timezone" }));

    expect(fetchConfigMock).toHaveBeenCalledTimes(1);
    expect(handleResetMock).toHaveBeenCalledTimes(1);
    expect(handleSaveMock).toHaveBeenCalledTimes(1);
    expect(handleLanguageChangeMock).toHaveBeenCalledWith("zh-CN");
    expect(handleTimezoneChangeMock).toHaveBeenCalledWith("UTC");
  });

  it("disables reset and save actions while saving", () => {
    useAgentConfigMock.mockReturnValue(configState({ saving: true }));

    render(<AgentConfigPage />);

    expect(screen.getByRole("button", { name: "Refresh" })).toBeDisabled();
    expect(screen.getByTestId("reset-icon").closest("button")).toBeDisabled();
    expect(screen.getByTestId("save-icon").closest("button")).toBeDisabled();
  });
});
