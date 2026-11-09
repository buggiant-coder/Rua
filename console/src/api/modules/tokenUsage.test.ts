import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../request", () => ({ request: vi.fn() }));

import { request } from "../request";
import { tokenUsageApi } from "./tokenUsage";

describe("tokenUsageApi Rust route canaries", () => {
  afterEach(() => vi.clearAllMocks());

  it("builds the Rust summary route with date query parameters", async () => {
    await tokenUsageApi.getTokenUsage({
      start_date: "2026-06-01",
      end_date: "2026-06-18",
    });

    expect(request).toHaveBeenCalledWith(
      "/token-usage?start_date=2026-06-01&end_date=2026-06-18",
    );
  });

  it("URL-encodes date parameters", async () => {
    await tokenUsageApi.getTokenUsage({
      start_date: "2026-06-01T00:00:00+08:00",
      end_date: "2026-06-18T23:59:59+08:00",
    });

    expect(request).toHaveBeenCalledOnce();
    expect(request).toHaveBeenCalledWith(
      "/token-usage?start_date=2026-06-01T00%3A00%3A00%2B08%3A00&end_date=2026-06-18T23%3A59%3A59%2B08%3A00",
    );
  });

  it("builds the QwenPaw-style details route with model and provider filters", async () => {
    await tokenUsageApi.getTokenUsageDetails({
      start_date: "2026-06-01",
      end_date: "2026-06-18",
      model: "gpt-4o",
      provider: "openai",
    });

    expect(request).toHaveBeenCalledWith(
      "/token-usage/details?start_date=2026-06-01&end_date=2026-06-18&model=gpt-4o&provider=openai",
    );
  });

  it("passes agent and chat attribution filters through the same usage route", async () => {
    await tokenUsageApi.getTokenUsage({
      start_date: "2026-06-01",
      end_date: "2026-06-18",
      agent_id: "research-agent",
      chat_id: "chat-123",
    });

    expect(request).toHaveBeenCalledWith(
      "/token-usage?start_date=2026-06-01&end_date=2026-06-18&agent_id=research-agent&chat_id=chat-123",
    );
  });
});
