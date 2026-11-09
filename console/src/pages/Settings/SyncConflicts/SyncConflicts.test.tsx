import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { MemoryRouter } from "react-router-dom";
import SyncConflictsPage from "./index";

const mockMessage = { success: vi.fn(), error: vi.fn() };
vi.mock("../../../hooks/useAppMessage", () => ({
  useAppMessage: () => ({ message: mockMessage }),
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, fallback?: string | Record<string, unknown>) => {
      if (typeof fallback === "string") return fallback;
      return key;
    },
  }),
}));

const mockGetPending = vi.fn();
const mockResolve = vi.fn();

vi.mock("../../../api/modules/sync", () => ({
  conflictsApi: {
    getPending: () => mockGetPending(),
    resolve: (id: string, chosen: string) => mockResolve(id, chosen),
  },
}));

function renderPage() {
  return render(
    <MemoryRouter initialEntries={["/settings/sync/conflicts"]}>
      <SyncConflictsPage />
    </MemoryRouter>,
  );
}

describe("SyncConflictsPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("shows empty state when no pending conflicts", async () => {
    mockGetPending.mockResolvedValue({ conflicts: [] });

    renderPage();

    await waitFor(() => {
      expect(
        screen.getByText("No pending conflicts to review"),
      ).toBeInTheDocument();
    });
  });

  it("renders pending conflicts list", async () => {
    mockGetPending.mockResolvedValue({
      conflicts: [
        {
          id: "c1",
          live_path: "prompts/test.json",
          record_id: "rec-1",
          winner_text: "winner content",
          loser_text: "loser content",
          created_at_ms: Date.now(),
        },
        {
          id: "c2",
          live_path: "agents/bot.json",
          record_id: "rec-2",
          winner_text: "winner 2",
          loser_text: "loser 2",
          created_at_ms: Date.now(),
        },
      ],
    });

    renderPage();

    await waitFor(() => {
      expect(screen.getByText("prompts/test.json")).toBeInTheDocument();
      expect(screen.getByText("agents/bot.json")).toBeInTheDocument();
    });

    const reviewButtons = screen.getAllByText("Review");
    expect(reviewButtons).toHaveLength(2);
  });

  it("opens resolve modal and resolves with winner", async () => {
    mockGetPending.mockResolvedValue({
      conflicts: [
        {
          id: "c1",
          live_path: "prompts/test.json",
          record_id: "rec-1",
          winner_text: "winner content",
          loser_text: "loser content",
          created_at_ms: Date.now(),
        },
      ],
    });
    mockResolve.mockResolvedValue(undefined);

    renderPage();

    await waitFor(() => {
      expect(screen.getByText("prompts/test.json")).toBeInTheDocument();
    });

    fireEvent.click(screen.getByText("Review"));

    await waitFor(() => {
      expect(screen.getByText("Resolve Conflict")).toBeInTheDocument();
      expect(screen.getByText("Version A (Winner)")).toBeInTheDocument();
      expect(screen.getByText("Version B (Loser)")).toBeInTheDocument();
    });

    fireEvent.click(screen.getByText("Keep Version A"));

    await waitFor(() => {
      expect(mockResolve).toHaveBeenCalledWith("c1", "winner content");
    });
  });

  it("resolves with loser version", async () => {
    mockGetPending.mockResolvedValue({
      conflicts: [
        {
          id: "c1",
          live_path: "prompts/test.json",
          record_id: "rec-1",
          winner_text: "winner content",
          loser_text: "loser content",
          created_at_ms: Date.now(),
        },
      ],
    });
    mockResolve.mockResolvedValue(undefined);

    renderPage();

    await waitFor(() => {
      expect(screen.getByText("prompts/test.json")).toBeInTheDocument();
    });

    fireEvent.click(screen.getByText("Review"));

    await waitFor(() => {
      expect(screen.getByText("Keep Version B")).toBeInTheDocument();
    });

    fireEvent.click(screen.getByText("Keep Version B"));

    await waitFor(() => {
      expect(mockResolve).toHaveBeenCalledWith("c1", "loser content");
    });
  });
});
