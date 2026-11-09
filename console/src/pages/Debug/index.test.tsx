import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import DebugPage from "./index";
import type { BackendDebugLogsResponse } from "../../api/modules/debug";

const {
  getBackendLogsMock,
  copyTextMock,
  tMock,
  toastSuccessMock,
  toastErrorMock,
} = vi.hoisted(() => ({
  getBackendLogsMock: vi.fn(),
  copyTextMock: vi.fn(),
  tMock: vi.fn((_key: string, fallback?: string) => fallback ?? _key),
  toastSuccessMock: vi.fn(),
  toastErrorMock: vi.fn(),
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: tMock,
  }),
}));

vi.mock("../../api/modules/debug", () => ({
  debugApi: {
    getBackendLogs: getBackendLogsMock,
  },
}));

vi.mock("../../utils/clipboard", () => ({
  copyText: copyTextMock,
}));

vi.mock("@/ui", () => ({
  Btn: ({
    children,
    onClick,
  }: {
    children: React.ReactNode;
    onClick?: () => void;
  }) => <button onClick={onClick}>{children}</button>,
  Dropdown: ({
    children,
    menu,
  }: {
    children: React.ReactNode;
    menu?: {
      items?: Array<
        | { type: "divider" }
        | {
            disabled?: boolean;
            key: string;
            label: React.ReactNode;
            onClick?: () => void;
          }
      >;
    };
  }) => (
    <div>
      {children}
      {menu?.items?.map((item, index) =>
        item.type === "divider" ? null : (
          <button
            disabled={item.disabled}
            key={item.key ?? index}
            type="button"
            onClick={item.onClick}
          >
            {item.label}
          </button>
        ),
      )}
    </div>
  ),
  InfoBox: ({ title }: { title: string }) => <div role="alert">{title}</div>,
  Loading: ({ children }: { children: React.ReactNode }) => <>{children}</>,
  Select: ({
    value,
    onChange,
    options,
  }: {
    value: string;
    onChange: (value: string) => void;
    options: Array<{ value: string; label: React.ReactNode }>;
  }) => (
    <select value={value} onChange={(e) => onChange(e.target.value)}>
      {options.map((option) => (
        <option key={option.value} value={option.value}>
          {option.value}
        </option>
      ))}
    </select>
  ),
  Tag: ({ children }: { children: React.ReactNode }) => <span>{children}</span>,
  TextInput: (props: React.InputHTMLAttributes<HTMLInputElement>) => (
    <input {...props} />
  ),
  Toggle: (props: React.InputHTMLAttributes<HTMLInputElement>) => (
    <input type="checkbox" {...props} />
  ),
  toast: {
    success: toastSuccessMock,
    error: toastErrorMock,
  },
}));

function backendLogs(
  overrides: Partial<BackendDebugLogsResponse> = {},
): BackendDebugLogsResponse {
  return {
    path: "/tmp/rua-server.log",
    exists: true,
    lines: 200,
    updated_at: 1_718_691_200,
    size: 128,
    content: [
      "2026-06-18T10:00:00Z INFO server ready",
      "2026-06-18T10:00:01Z DEBUG route matched /health",
      "2026-06-18T10:00:02Z ERROR request failed",
    ].join("\n"),
    ...overrides,
  };
}

describe.sequential("DebugPage backend logs", () => {
  afterEach(() => {
    cleanup();
  });

  beforeEach(() => {
    cleanup();
    getBackendLogsMock.mockReset();
    copyTextMock.mockReset();
    tMock.mockClear();
    toastSuccessMock.mockReset();
    toastErrorMock.mockReset();
    getBackendLogsMock.mockResolvedValue(backendLogs());
    copyTextMock.mockResolvedValue(true);
  });

  it("loads the Rust backend-log route shape with the default line budget", async () => {
    render(<DebugPage />);

    await screen.findByText("2026-06-18T10:00:02Z ERROR request failed");

    expect(getBackendLogsMock).toHaveBeenCalledWith(200);
    expect(screen.getByText(/Updated at:/)).toBeInTheDocument();
  });

  it("shows the Rust missing-log empty state when the log file does not exist", async () => {
    getBackendLogsMock.mockReset();
    getBackendLogsMock.mockResolvedValueOnce(
      backendLogs({
        exists: false,
        updated_at: null,
        size: 0,
        content: "",
      }),
    );

    render(<DebugPage />);

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Backend log file was not found yet.",
    );
    expect(
      screen.getByText("Backend log output will appear here."),
    ).toBeInTheDocument();
  });

  it("filters visible log lines by Rust level text and search query", async () => {
    render(<DebugPage />);

    await screen.findByText("2026-06-18T10:00:02Z ERROR request failed");

    fireEvent.change(screen.getByRole("combobox"), {
      target: { value: "error" },
    });
    fireEvent.change(screen.getByPlaceholderText("Search backend logs..."), {
      target: { value: "failed" },
    });

    expect(
      screen.getAllByText((_content, node) => {
        return (
          node?.textContent === "2026-06-18T10:00:02Z ERROR request failed"
        );
      }).length,
    ).toBeGreaterThan(0);
    expect(
      screen.queryByText("2026-06-18T10:00:01Z DEBUG route matched /health"),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByText("2026-06-18T10:00:00Z INFO server ready"),
    ).not.toBeInTheDocument();
  });

  it("toggles newest-first ordering without mutating the backend content", async () => {
    const { container } = render(<DebugPage />);

    const newestLine = await screen.findByText(
      "2026-06-18T10:00:02Z ERROR request failed",
    );
    const oldestLine = screen.getByText(
      "2026-06-18T10:00:00Z INFO server ready",
    );
    expect(
      newestLine.compareDocumentPosition(oldestLine) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();

    fireEvent.click(screen.getAllByRole("checkbox")[0]);

    const renderedLines = Array.from(container.querySelectorAll("div"))
      .map((node) => node.textContent ?? "")
      .filter((text) => text.includes("2026-06-18T10:00:"));
    expect(renderedLines.join("\n")).toContain(
      [
        "2026-06-18T10:00:00Z INFO server ready",
        "2026-06-18T10:00:01Z DEBUG route matched /health",
        "2026-06-18T10:00:02Z ERROR request failed",
      ].join("\n"),
    );
  });

  it("refreshes logs manually and reports the Rust page toast", async () => {
    getBackendLogsMock.mockReset();
    getBackendLogsMock
      .mockResolvedValueOnce(backendLogs({ content: "first INFO line" }))
      .mockResolvedValueOnce(backendLogs({ content: "second INFO line" }));

    render(<DebugPage />);

    await screen.findByText("first INFO line");
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));

    await screen.findByText("second INFO line");
    await waitFor(() => {
      expect(toastSuccessMock).toHaveBeenCalledWith("Logs refreshed");
    });
    expect(getBackendLogsMock).toHaveBeenCalledTimes(2);
  });
});
