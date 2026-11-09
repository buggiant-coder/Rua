import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../request", () => ({ request: vi.fn() }));
vi.mock("../authHeaders", () => ({
  getSelectedAgentId: vi.fn(() => "selected-agent"),
}));

import { getSelectedAgentId } from "../authHeaders";
import { request } from "../request";
import { cronJobApi } from "./cronjob";

describe("cronJobApi", () => {
  afterEach(() => vi.clearAllMocks());

  it("uses the selected Rust agent scope by default", async () => {
    vi.mocked(request).mockResolvedValueOnce([]);

    await cronJobApi.listCronJobs();

    expect(getSelectedAgentId).toHaveBeenCalled();
    expect(request).toHaveBeenCalledWith("/agents/selected-agent/cron/jobs");
  });

  it("creates jobs under the explicit agent-scoped route", async () => {
    const spec = {
      name: "cleanup",
      cron_expression: "0 * * * *",
      enabled: true,
      prompt: "clean workspace",
    } as Parameters<typeof cronJobApi.createCronJob>[0];

    await cronJobApi.createCronJob(spec, "agent-1");

    expect(request).toHaveBeenCalledWith("/agents/agent-1/cron/jobs", {
      method: "POST",
      body: JSON.stringify(spec),
    });
  });

  it("encodes job ids for get, replace, and delete", async () => {
    vi.mocked(request).mockResolvedValue({});
    const jobId = "daily/job 1";
    const spec = {
      name: "daily",
      cron_expression: "*/5 * * * *",
    } as Parameters<typeof cronJobApi.replaceCronJob>[1];

    await cronJobApi.getCronJob(jobId, "agent-1");
    await cronJobApi.replaceCronJob(jobId, spec, "agent-1");
    await cronJobApi.deleteCronJob(jobId, "agent-1");

    const encoded = encodeURIComponent(jobId);
    expect(request).toHaveBeenNthCalledWith(
      1,
      `/agents/agent-1/cron/jobs/${encoded}`,
    );
    expect(request).toHaveBeenNthCalledWith(
      2,
      `/agents/agent-1/cron/jobs/${encoded}`,
      {
        method: "PUT",
        body: JSON.stringify(spec),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      3,
      `/agents/agent-1/cron/jobs/${encoded}`,
      { method: "DELETE" },
    );
  });

  it("routes pause, resume, run, trigger, and state through Rust scoped actions", async () => {
    vi.mocked(request).mockResolvedValue({});
    const jobId = "job/slash";
    const encoded = encodeURIComponent(jobId);

    await cronJobApi.pauseCronJob(jobId, "agent-1");
    await cronJobApi.resumeCronJob(jobId, "agent-1");
    await cronJobApi.runCronJob(jobId, "agent-1");
    await cronJobApi.triggerCronJob(jobId, "agent-1");
    await cronJobApi.getCronJobState(jobId, "agent-1");

    expect(request).toHaveBeenNthCalledWith(
      1,
      `/agents/agent-1/cron/jobs/${encoded}/pause`,
      { method: "POST" },
    );
    expect(request).toHaveBeenNthCalledWith(
      2,
      `/agents/agent-1/cron/jobs/${encoded}/resume`,
      { method: "POST" },
    );
    expect(request).toHaveBeenNthCalledWith(
      3,
      `/agents/agent-1/cron/jobs/${encoded}/run`,
      { method: "POST" },
    );
    expect(request).toHaveBeenNthCalledWith(
      4,
      `/agents/agent-1/cron/jobs/${encoded}/run`,
      { method: "POST" },
    );
    expect(request).toHaveBeenNthCalledWith(
      5,
      `/agents/agent-1/cron/jobs/${encoded}/state`,
    );
  });

  it("lists dispatch targets through the Rust scoped route with filters", async () => {
    vi.mocked(request).mockResolvedValue({});

    await cronJobApi.listDispatchTargets(
      { channel: "ding talk", keyword: "alice/bob", limit: 25 },
      "agent-1",
    );

    expect(request).toHaveBeenCalledWith(
      "/agents/agent-1/cron/dispatch-targets?channel=ding+talk&keyword=alice%2Fbob&limit=25",
    );
  });
});
