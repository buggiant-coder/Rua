import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../request", () => ({ request: vi.fn() }));

import { request } from "../request";
import { agentStatsApi } from "./agentStats";

describe("agentStatsApi Rust route canaries", () => {
  afterEach(() => vi.clearAllMocks());

  it("builds the Rust agent-stats route with date query parameters", async () => {
    await agentStatsApi.getAgentStats({
      start_date: "2026-06-01",
      end_date: "2026-06-18",
    });

    expect(request).toHaveBeenCalledWith(
      "/agent-stats?start_date=2026-06-01&end_date=2026-06-18",
    );
  });

  it("URL-encodes date parameters without calling token-usage or history routes", async () => {
    await agentStatsApi.getAgentStats({
      start_date: "2026-06-01T00:00:00+08:00",
      end_date: "2026-06-18T23:59:59+08:00",
    });

    expect(request).toHaveBeenCalledOnce();
    expect(request).toHaveBeenCalledWith(
      "/agent-stats?start_date=2026-06-01T00%3A00%3A00%2B08%3A00&end_date=2026-06-18T23%3A59%3A59%2B08%3A00",
    );
  });
});
