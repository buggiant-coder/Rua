import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../request", () => ({ request: vi.fn() }));

import { request } from "../request";
import { debugApi } from "./debug";

describe("debugApi Rust route canaries", () => {
  afterEach(() => vi.clearAllMocks());

  it("uses Rust console backend-log route with a default line count", async () => {
    await debugApi.getBackendLogs();

    expect(request).toHaveBeenCalledWith(
      "/console/debug/backend-logs?lines=200",
    );
  });

  it("passes the requested line count without using QwenPaw debug DTO wrappers", async () => {
    await debugApi.getBackendLogs(50);

    expect(request).toHaveBeenCalledWith(
      "/console/debug/backend-logs?lines=50",
    );
  });
});
