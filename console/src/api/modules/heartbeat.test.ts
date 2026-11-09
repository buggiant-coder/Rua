import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../request", () => ({ request: vi.fn() }));

import { request } from "../request";
import { heartbeatApi } from "./heartbeat";

describe("heartbeatApi Rust route canaries", () => {
  afterEach(() => vi.clearAllMocks());

  it("uses the root/header-selected heartbeat config route", async () => {
    const body = {
      enabled: true,
      interval_minutes: 30,
    } as Parameters<typeof heartbeatApi.updateHeartbeatConfig>[0];

    await heartbeatApi.getHeartbeatConfig();
    await heartbeatApi.updateHeartbeatConfig(body);
    await heartbeatApi.runHeartbeatNow();

    expect(request).toHaveBeenNthCalledWith(1, "/config/heartbeat");
    expect(request).toHaveBeenNthCalledWith(2, "/config/heartbeat", {
      method: "PUT",
      body: JSON.stringify(body),
    });
    expect(request).toHaveBeenNthCalledWith(3, "/config/heartbeat/run", {
      method: "POST",
    });
  });
});
