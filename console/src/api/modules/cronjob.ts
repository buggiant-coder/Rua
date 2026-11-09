import { request } from "../request";
import { getSelectedAgentId } from "../authHeaders";
import type {
  CronDispatchTargetsResponse,
  CronJobSpecInput,
  CronJobSpecOutput,
  CronJobView,
} from "../types";

export interface CronDispatchTargetsParams {
  channel?: string;
  keyword?: string;
  limit?: number;
}

function buildDispatchTargetsQuery(params?: CronDispatchTargetsParams): string {
  const query = new URLSearchParams();
  if (params?.channel) query.set("channel", params.channel);
  if (params?.keyword) query.set("keyword", params.keyword);
  if (params?.limit) query.set("limit", String(params.limit));
  const text = query.toString();
  return text ? `?${text}` : "";
}

export const cronJobApi = {
  listCronJobs: (agentId: string = getSelectedAgentId()) =>
    request<CronJobSpecOutput[]>(`/agents/${agentId}/cron/jobs`),

  createCronJob: (
    spec: CronJobSpecInput,
    agentId: string = getSelectedAgentId(),
  ) =>
    request<CronJobSpecOutput>(`/agents/${agentId}/cron/jobs`, {
      method: "POST",
      body: JSON.stringify(spec),
    }),

  getCronJob: (jobId: string, agentId: string = getSelectedAgentId()) =>
    request<CronJobView>(
      `/agents/${agentId}/cron/jobs/${encodeURIComponent(jobId)}`,
    ),

  replaceCronJob: (
    jobId: string,
    spec: CronJobSpecInput,
    agentId: string = getSelectedAgentId(),
  ) =>
    request<CronJobSpecOutput>(
      `/agents/${agentId}/cron/jobs/${encodeURIComponent(jobId)}`,
      {
        method: "PUT",
        body: JSON.stringify(spec),
      },
    ),

  deleteCronJob: (jobId: string, agentId: string = getSelectedAgentId()) =>
    request<void>(`/agents/${agentId}/cron/jobs/${encodeURIComponent(jobId)}`, {
      method: "DELETE",
    }),

  pauseCronJob: (jobId: string, agentId: string = getSelectedAgentId()) =>
    request<void>(
      `/agents/${agentId}/cron/jobs/${encodeURIComponent(jobId)}/pause`,
      {
        method: "POST",
      },
    ),

  resumeCronJob: (jobId: string, agentId: string = getSelectedAgentId()) =>
    request<void>(
      `/agents/${agentId}/cron/jobs/${encodeURIComponent(jobId)}/resume`,
      {
        method: "POST",
      },
    ),

  runCronJob: (jobId: string, agentId: string = getSelectedAgentId()) =>
    request<void>(
      `/agents/${agentId}/cron/jobs/${encodeURIComponent(jobId)}/run`,
      {
        method: "POST",
      },
    ),

  triggerCronJob: (jobId: string, agentId: string = getSelectedAgentId()) =>
    request<void>(
      `/agents/${agentId}/cron/jobs/${encodeURIComponent(jobId)}/run`,
      {
        method: "POST",
      },
    ),

  getCronJobState: (jobId: string, agentId: string = getSelectedAgentId()) =>
    request<unknown>(
      `/agents/${agentId}/cron/jobs/${encodeURIComponent(jobId)}/state`,
    ),

  listDispatchTargets: (
    params?: CronDispatchTargetsParams,
    agentId: string = getSelectedAgentId(),
  ) =>
    request<CronDispatchTargetsResponse>(
      `/agents/${agentId}/cron/dispatch-targets${buildDispatchTargetsQuery(
        params,
      )}`,
    ),
};
