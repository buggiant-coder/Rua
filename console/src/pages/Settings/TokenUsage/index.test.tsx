import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "@/i18n";
import TokenUsagePage from "./index";

const getTokenUsageDetailsMock = vi.hoisted(() => vi.fn());

vi.mock("../../../api", () => ({
  default: {
    getTokenUsageDetails: getTokenUsageDetailsMock,
  },
}));

describe("TokenUsagePage", () => {
  beforeEach(async () => {
    vi.clearAllMocks();
    vi.useFakeTimers({ shouldAdvanceTime: true });
    vi.setSystemTime(new Date("2026-06-18T08:00:00+08:00"));
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("loads the Rust summary route window and renders the empty state", async () => {
    getTokenUsageDetailsMock.mockResolvedValueOnce([]);

    render(<TokenUsagePage />);

    await screen.findByText("No token usage data in the selected period");
    expect(screen.getByLabelText("Start date")).toHaveValue("2026-05-19");
    expect(screen.getByLabelText("End date")).toHaveValue("2026-06-18");
    expect(getTokenUsageDetailsMock).toHaveBeenCalledWith({
      start_date: "2026-05-19",
      end_date: "2026-06-18",
    });
  });

  it("renders by-model and by-date data aggregated from detail rows", async () => {
    getTokenUsageDetailsMock.mockResolvedValueOnce([
      {
        date: "2026-06-17",
        provider_id: "dashscope",
        model: "qwen-plus",
        prompt_tokens: 100,
        completion_tokens: 40,
        call_count: 1,
      },
      {
        date: "2026-06-18",
        provider_id: "dashscope",
        model: "qwen-plus",
        prompt_tokens: 1100,
        completion_tokens: 300,
        call_count: 2,
      },
    ]);

    render(<TokenUsagePage />);

    await screen.findByText("dashscope");
    expect(screen.getByText("qwen-plus")).toBeInTheDocument();
    expect(screen.getByText("Model Trend")).toBeInTheDocument();
    expect(screen.getByText("Token Type Trend")).toBeInTheDocument();
    expect(screen.getAllByText("Total Calls").length).toBeGreaterThan(0);
    expect(screen.getAllByText("Total Tokens").length).toBeGreaterThan(0);
    expect(screen.getByText("By Model")).toBeInTheDocument();
    expect(screen.getByText("By Date")).toBeInTheDocument();
    expect(screen.getByText("2026-06-18")).toBeInTheDocument();
    expect(screen.getByText("2026-06-17")).toBeInTheDocument();
  });

  it("refreshes the current date window through the details API", async () => {
    getTokenUsageDetailsMock.mockResolvedValueOnce([]).mockResolvedValueOnce([
      {
        date: "2026-06-18",
        provider_id: "openai",
        model: "gpt-4o",
        prompt_tokens: 8,
        completion_tokens: 5,
        call_count: 1,
      },
    ]);

    render(<TokenUsagePage />);

    await screen.findByText("No token usage data in the selected period");
    fireEvent.click(screen.getAllByRole("button")[0]);
    fireEvent.click(await screen.findByText("Refresh page"));

    await screen.findByText("openai");
    expect(getTokenUsageDetailsMock).toHaveBeenCalledTimes(2);
    expect(getTokenUsageDetailsMock).toHaveBeenLastCalledWith({
      start_date: "2026-05-19",
      end_date: "2026-06-18",
    });
  });

  it("reloads usage after selecting a new date range", async () => {
    getTokenUsageDetailsMock.mockResolvedValue([]);

    render(<TokenUsagePage />);

    await screen.findByText("No token usage data in the selected period");
    fireEvent.click(screen.getByLabelText("Start date"));
    fireEvent.click(screen.getByRole("button", { name: "May 18, 2026" }));

    await vi.waitFor(() => {
      expect(getTokenUsageDetailsMock).toHaveBeenLastCalledWith({
        start_date: "2026-05-18",
        end_date: "2026-06-18",
      });
    });
  });
});
