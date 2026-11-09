import { request } from "../request";

export type SyncTransferEntryKind = "directory" | "file" | "symlink" | "other";

export interface SyncTransferEntry {
  name: string;
  path: string;
  kind: SyncTransferEntryKind;
  size?: number;
  modified_at_ms?: number;
}

export interface SyncTransferListResponse {
  device_id: string;
  path: string;
  entries: SyncTransferEntry[];
}

export interface SyncTransferJob {
  job_id: string;
  status: "running" | "succeeded" | "failed" | "cancelled";
  source_device_id: string;
  target_device_id: string;
  target_dir: string;
  files_written: number;
  bytes_written: number;
  total_bytes: number;
  error?: string;
}

export interface SyncTransferSendRequest {
  source_device_id: string;
  source_paths: string[];
  target_device_id: string;
  target_dir: string;
  conflict?: "rename" | "fail" | "overwrite";
}

export const syncTransferApi = {
  listDirectory: (deviceId: string, path: string) =>
    request<SyncTransferListResponse>("/sync/transfer/list", {
      method: "POST",
      body: JSON.stringify({ device_id: deviceId, path }),
    }),

  send: (body: SyncTransferSendRequest) =>
    request<SyncTransferJob>("/sync/transfer/send", {
      method: "POST",
      body: JSON.stringify(body),
    }),

  listJobs: () => request<SyncTransferJob[]>("/sync/transfer/jobs"),

  getJob: (jobId: string) =>
    request<SyncTransferJob>(
      `/sync/transfer/jobs/${encodeURIComponent(jobId)}`,
    ),

  cancelJob: (jobId: string) =>
    request<SyncTransferJob>(
      `/sync/transfer/jobs/${encodeURIComponent(jobId)}/cancel`,
      { method: "POST" },
    ),
};
