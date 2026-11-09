import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { WebSearchConfigModal } from "./WebSearchConfigModal";

const { getConfigMock, tMock, updateConfigMock, toastMock } = vi.hoisted(
  () => ({
    getConfigMock: vi.fn(),
    tMock: vi.fn((key: string, options?: { defaultValue?: string }) => {
      if (key === "tools.webSearchConfig.optionalKeyPlaceholder") {
        return "Paste optional API key";
      }
      return options?.defaultValue ?? key;
    }),
    updateConfigMock: vi.fn(),
    toastMock: {
      error: vi.fn(),
      success: vi.fn(),
    },
  }),
);

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: tMock,
  }),
}));

vi.mock("../../../api/modules/agents", () => ({
  agentsApi: {
    getAgentToolConfig: getConfigMock,
    updateAgentToolConfig: updateConfigMock,
  },
}));

vi.mock("../../../stores/agentStore", () => ({
  useAgentStore: () => ({ selectedAgent: "agent-a" }),
}));

vi.mock("@/ui", () => ({
  Btn: ({
    children,
    onClick,
  }: {
    children: React.ReactNode;
    onClick?: () => void;
  }) => <button onClick={onClick}>{children}</button>,
  Modal: ({
    children,
    footer,
    open,
    title,
  }: {
    children: React.ReactNode;
    footer?: React.ReactNode;
    open: boolean;
    title?: React.ReactNode;
  }) =>
    open ? (
      <div role="dialog">
        <h1>{title}</h1>
        {children}
        {footer}
      </div>
    ) : null,
  TextInput: (props: React.InputHTMLAttributes<HTMLInputElement>) => (
    <input {...props} />
  ),
  Toggle: (props: React.InputHTMLAttributes<HTMLInputElement>) => (
    <input type="checkbox" {...props} />
  ),
  toast: toastMock,
}));

describe("WebSearchConfigModal", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    getConfigMock.mockResolvedValue({});
    updateConfigMock.mockImplementation(
      async (_agent: string, _tool: string, config: unknown) => config,
    );
  });

  it("uses Tavily then Exa by default without scraping providers", async () => {
    render(<WebSearchConfigModal open onClose={vi.fn()} />);

    await screen.findByText("Tavily");
    const labels = Array.from(
      document.querySelectorAll('[draggable="true"]'),
      (row) => row.textContent,
    );

    expect(labels.slice(0, 2)).toEqual([
      expect.stringContaining("Tavily"),
      expect.stringContaining("Exa"),
    ]);
    expect(screen.queryByText("Bing RSS")).not.toBeInTheDocument();
    expect(screen.queryByText("Sogou")).not.toBeInTheDocument();
    expect(screen.queryByText("DuckDuckGo HTML")).not.toBeInTheDocument();
    expect(
      screen.getAllByPlaceholderText("Paste optional API key"),
    ).toHaveLength(2);
  });

  it("preserves user ordering and saves a drag reorder with optional keys", async () => {
    getConfigMock.mockResolvedValue({
      mode: "ordered_fallback",
      providers: [
        { id: "brave", enabled: false },
        { id: "exa", enabled: true },
        { id: "tavily", enabled: true },
      ],
    });

    render(<WebSearchConfigModal open onClose={vi.fn()} />);

    const brave = await screen.findByText("Brave Search API");
    const exa = screen.getByText("Exa");
    const braveRow = brave.closest('[draggable="true"]');
    const exaRow = exa.closest('[draggable="true"]');
    expect(braveRow).not.toBeNull();
    expect(exaRow).not.toBeNull();

    fireEvent.dragStart(exaRow!);
    fireEvent.dragOver(braveRow!);
    fireEvent.drop(braveRow!);
    fireEvent.click(screen.getByText("Save"));

    await waitFor(() => expect(updateConfigMock).toHaveBeenCalled());
    const saved = updateConfigMock.mock.calls[0][2] as {
      providers: Array<{ id: string }>;
    };
    expect(saved.providers.slice(0, 3).map(({ id }) => id)).toEqual([
      "exa",
      "brave",
      "tavily",
    ]);
  });
});
