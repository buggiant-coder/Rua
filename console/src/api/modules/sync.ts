import { subscribeConsoleResponse } from "../consoleSubscription";
import { getApiUrl, getApiToken } from "../config";
import { readSseChunks } from "@/pages/Chat/owned/sse";

export interface InitiateResponse {
  pin: string;
  expires_in_secs: number;
}

export interface PairingStatusResponse {
  status: "active" | "expired" | "consumed" | "paired" | "failed" | "none";
  pin?: string;
  remaining_secs?: number;
}

export interface CancelResponse {
  cancelled: boolean;
}

export interface JoinResponse {
  device_public_key: string;
}

export interface JoinError {
  detail: string;
  reason: "wrong-pin" | "expired" | "network";
}

function authHeaders(): Record<string, string> {
  const token = getApiToken();
  const headers: Record<string, string> = {
    "Content-Type": "application/json",
  };
  if (token) {
    headers["Authorization"] = `Bearer ${token}`;
  }
  return headers;
}

export type DeviceConnectivityState =
  | "connected"
  | "connecting"
  | "not_connected";

export type DeviceSessionPath = "direct" | "relay" | "other";

export interface DeviceDto {
  device_id: string;
  public_key: string;
  friendly_name: string;
  label: string;
  added_at_ms: number;
  last_seen_ms: number;
  is_self: boolean;
  connectivity_state: DeviceConnectivityState;
  session_path: DeviceSessionPath | null;
  session_rtt_ms: number | null;
}

export interface DeviceStatusResponse {
  revocation_state: "active" | "suspected" | "revoked";
  is_revoked: boolean;
}

export const devicesApi = {
  listDevices: async (): Promise<DeviceDto[]> => {
    const res = await fetch(getApiUrl("/sync/devices"), {
      headers: authHeaders(),
    });
    if (!res.ok) throw new Error("Failed to list devices");
    const data = await res.json();
    return data.devices;
  },

  watchChanges: (onChange: () => void): (() => void) => {
    const subscription = subscribeConsoleResponse(
      "/sync/devices/activity",
      async (response, _reconnected, signal) => {
        onChange();
        for await (const chunk of readSseChunks(
          response.body?.pipeThrough(new TransformStream(), { signal }) ?? null,
        )) {
          if (signal.aborted) break;
          if (chunk.event === "device_connectivity") onChange();
        }
      },
    );
    return subscription.stop;
  },

  removeDevice: async (
    deviceId: string,
  ): Promise<{ removed: boolean; device_id: string }> => {
    const res = await fetch(getApiUrl("/sync/devices/remove"), {
      method: "POST",
      headers: authHeaders(),
      body: JSON.stringify({ device_id: deviceId }),
    });
    if (!res.ok) throw new Error("Failed to remove device");
    return res.json();
  },

  renameDevice: async (
    deviceId: string,
    friendlyName: string,
  ): Promise<{
    renamed: boolean;
    device_id: string;
    friendly_name: string;
  }> => {
    const res = await fetch(getApiUrl("/sync/devices/rename"), {
      method: "POST",
      headers: authHeaders(),
      body: JSON.stringify({
        device_id: deviceId,
        friendly_name: friendlyName,
      }),
    });
    if (!res.ok) throw new Error("Failed to rename device");
    return res.json();
  },

  getDeviceStatus: async (): Promise<DeviceStatusResponse> => {
    const res = await fetch(getApiUrl("/sync/devices/status"), {
      headers: authHeaders(),
    });
    if (!res.ok) throw new Error("Failed to get device status");
    return res.json();
  },
};

export type SyncErrorReason =
  | "peer_unreachable"
  | "no_eligible_peer"
  | "signature_failed"
  | "decrypt_failed"
  | "timeout"
  | "repair_scan_failed"
  | "inbox_drain_failed"
  | "not_paired";

export interface SyncRepairScanResponse {
  scanned: number;
  recorded: number;
  already_applied: number;
  errors: string[];
  duration_ms: number;
}

export interface SyncCycleResponse {
  events_received: number;
  events_sent: number;
  peer_node_id: string;
  duration_ms: number;
  transport_duration_ms: number;
  local_apply_duration_ms: number;
  conflicts_detected: number;
  repair?: SyncRepairScanResponse;
}

export interface SyncCycleError {
  reason: SyncErrorReason;
  detail: string;
}

export interface SyncStatusResponse {
  enabled: boolean;
  paired: boolean;
  membership?: SyncNetworkMembership;
  peer_count: number;
  host_profile?: SyncRuntimeHostProfile;
  effective_power_policy?: EffectiveSyncPowerPolicy;
}

export interface SyncConfigResponse {
  enabled: boolean;
}

export interface SyncEnableResponse {
  enabled: boolean;
  identity_created: boolean;
  membership?: SyncNetworkMembership;
}

export type SyncNetworkMembershipState =
  | "disabled"
  | "enabled_no_network"
  | "network_created"
  | "bound"
  | "identity_mismatch"
  | "corrupt_identity";

export interface SyncNetworkMembership {
  state: SyncNetworkMembershipState;
  namespace_id?: string;
  detail?: string;
}

export interface SyncCreateNetworkResponse {
  enabled: boolean;
  created: boolean;
  namespace_id: string;
}

export interface SyncDisableResponse {
  enabled: boolean;
}

export type SyncPowerPolicyMode =
  | "power_saver"
  | "balanced"
  | "realtime"
  | "custom";

export type SyncRuntimeHostProfile =
  | "mobile-apk"
  | "desktop"
  | "headless"
  | "test";

export interface SyncPowerPolicyConfig {
  mode: SyncPowerPolicyMode;
  periodic_interval_secs?: number;
  background_flush_grace_secs?: number;
  allow_background_runtime?: boolean;
  allow_background_scheduled_sync?: boolean;
}

export interface EffectiveSyncPowerPolicy {
  mode: SyncPowerPolicyMode;
  periodic_interval_secs: number;
  background_flush_grace_secs: number;
  allow_background_runtime: boolean;
  allow_background_scheduled_sync: boolean;
}

export interface SyncPowerPolicyResponse {
  host_profile: SyncRuntimeHostProfile;
  stored_policy: SyncPowerPolicyConfig | null;
  effective_policy: EffectiveSyncPowerPolicy;
}

export type SyncRouteKind = "unknown" | "direct" | "relay" | "mixed";
export type SyncProgressState =
  | "disabled"
  | "not_paired"
  | "identity_mismatch"
  | "syncing"
  | "up_to_date"
  | "applying_changes"
  | "waiting_for_content"
  | "waiting_for_device"
  | "needs_attention";
export type SyncTransportCycleStatus = "success" | "failure";
export type SyncTransportFailureKind = "peer_unavailable" | "internal";

export interface SyncTransportCycleStats {
  completed_at_ms: number;
  peer_node_id: string;
  status: SyncTransportCycleStatus;
  error?: string;
  failure_kind?: SyncTransportFailureKind;
  events_received: number;
  events_sent: number;
  connect_ms: number;
  process_ms: number;
  duration_ms: number;
  route: SyncRouteKind;
  direct_tx_bytes: number;
  direct_rx_bytes: number;
  relay_tx_bytes: number;
  relay_rx_bytes: number;
  direct_rtt_ms?: number;
  relay_rtt_ms?: number;
  warnings?: string[];
}

export interface SyncTransportRollingStats {
  cycles: number;
  failures: number;
  events_received?: number;
  events_sent?: number;
  direct_tx_bytes: number;
  direct_rx_bytes: number;
  relay_tx_bytes: number;
  relay_rx_bytes: number;
}

export interface SyncTransportStatsState {
  active_cycle?: { started_at_ms: number; peer_node_id: string };
  last_cycle?: SyncTransportCycleStats;
  rolling: SyncTransportRollingStats;
  peers?: Record<
    string,
    {
      last_cycle?: SyncTransportCycleStats;
      direct_bytes: number;
      relay_bytes: number;
      cycles: number;
      failures: number;
    }
  >;
}

export interface SyncProgressDiagnostics {
  state: SyncProgressState;
  is_caught_up: boolean;
  pending_inbound: number;
  retryable_inbound: number;
  dormant_inbound: number;
  failed_inbound: number;
  pending_conflicts: number;
  last_cycle_status?: SyncTransportCycleStatus;
}

export interface PeerHealthDiagnosticEntry {
  node_id: string;
  device_id_hex: string;
  friendly_name?: string;
  last_seen_or_added_ms: number;
  selectable: boolean;
  consecutive_failures: number;
  last_failure_ms?: number;
  last_success_ms?: number;
  cooldown_until_ms?: number;
  sync_timeout_ms: number;
  last_route?: SyncRouteKind;
  last_direct_bytes?: number;
  last_relay_bytes?: number;
  last_rtt_ms?: number;
}

export interface PeerHealthDiagnostics {
  total_peers: number;
  selectable_peers: number;
  peers_in_cooldown: number;
  next_peer_node_id?: string;
  peers: PeerHealthDiagnosticEntry[];
}

export interface BlobDownloadAttemptDiagnostics {
  hash: string;
  label: string;
  provider_order: string[];
  tried_providers: string[];
  failed_providers: string[];
  selected_provider?: string;
  bytes: number;
  provider_selection_ms: number;
  connection_ms: number;
  download_ms: number;
  duration_ms: number;
  route: SyncRouteKind;
  status: "success" | "failure";
  failure?: string;
}

export interface BlobDownloadDiagnostics {
  requests: number;
  successes: number;
  failures: number;
  last?: BlobDownloadAttemptDiagnostics;
}

export type SyncStageStatus = "success" | "failure";

export interface SyncPublicationStageSample {
  correlation_id: string;
  commit_observed_at_ms: number;
  publication_started_at_ms: number;
  publication_completed_at_ms: number;
  commit_to_publication_ms: number;
  publication_ms: number;
  status: SyncStageStatus;
  failure_cause?: string;
}

export interface SyncInboundStageSample {
  correlation_id: string;
  source_commit_observed_at_ms?: number;
  transport_arrival_at_ms: number;
  inbox_recorded_at_ms: number;
  content_ready_at_ms: number;
  decrypt_read_completed_at_ms: number;
  domain_apply_completed_at_ms: number;
  cache_refresh_completed_at_ms: number;
  canonical_visible_at_ms: number;
  transport_to_inbox_ms: number;
  inbox_wait_ms: number;
  content_wait_ms: number;
  content_read_ms: number;
  decrypt_ms: number;
  domain_apply_ms: number;
  cache_refresh_ms: number;
  canonical_visibility_ms: number;
  receiver_pipeline_ms: number;
  end_to_end_wall_ms?: number;
  used_peer_download: boolean;
  status: SyncStageStatus;
}

export interface SyncLatencySummary {
  samples: number;
  p50_ms: number;
  p95_ms: number;
  max_ms: number;
}

export interface SyncStageDiagnostics {
  recent_publications: SyncPublicationStageSample[];
  recent_inbound: SyncInboundStageSample[];
  receiver_pipeline?: SyncLatencySummary;
  end_to_end_wall?: SyncLatencySummary;
  last_failure?: {
    correlation_id?: string;
    entry_fingerprint?: string;
    stage: string;
    cause: string;
    observed_at_ms: number;
  };
  last_sync_now?: {
    completed_at_ms: number;
    selected_peer_node_id?: string;
    transport_status: SyncStageStatus;
    local_completion_status: "not_started" | "success" | "failure";
    transport_duration_ms: number;
    local_apply_duration_ms: number;
    duration_ms: number;
    failure_stage?: string;
  };
}

export interface SyncDiagnosticsResponse {
  host_profile: SyncRuntimeHostProfile;
  effective_policy: EffectiveSyncPowerPolicy;
  runtime_state: {
    state: string;
    pending_grace: boolean;
    background_grace: { pending: boolean; remaining_ms?: number };
    warm_suspended: boolean;
  };
  identity?: {
    status: "disabled" | "not_paired" | "bound" | "identity_mismatch";
    namespace_id?: string;
    detail?: string;
  };
  inbox: {
    counts: {
      pending: number;
      retryable: number;
      dormant: number;
      discarded?: number;
      applied: number;
      failed: number;
    };
    repair_scan?: {
      last_successful_at_ms: number | null;
      next_due_at_ms: number | null;
      last_attempt_at_ms: number | null;
      last_duration_ms: number | null;
      last_scanned: number;
      last_recorded: number;
      last_already_applied: number;
      last_errors: number;
      last_result: "success" | "failure" | null;
    };
  };
  progress: SyncProgressDiagnostics;
  transport: SyncTransportStatsState;
  blob?: BlobDownloadDiagnostics;
  stages: SyncStageDiagnostics;
  peers: PeerHealthDiagnostics;
  device_network?: DeviceNetworkDiagnostics | null;
}

export interface DeviceConnectivityProjection {
  peer_device_id: string;
  peer_endpoint_id: string;
  state: DeviceConnectivityState;
}

export interface DeviceSessionDiagnostics extends DeviceConnectivityProjection {
  generation: number | null;
  origin: "incoming" | "outgoing" | null;
  path_class: "direct" | "relay" | "other" | "unknown";
  active_logical_streams: number;
  established_at_ms: number | null;
  last_used_at_ms: number | null;
  state_changed_at_ms: number | null;
  last_reconnect_at_ms: number | null;
  rtt_ms: number | null;
  tx_bytes: number;
  rx_bytes: number;
  reconnects: number;
  last_error?: string;
}

export interface DeviceNetworkDiagnostics {
  state: "active" | "stopped";
  local_device_id: string;
  endpoint_id: string;
  network_id: string;
  mdns_enabled: boolean;
  bound_sockets: string[];
  home_relays: string[];
  gossip_running: boolean;
  gossip_neighbors: number;
  connectivity: DeviceConnectivityProjection[];
  device_sessions: {
    physical_dial_attempts: number;
    accepted_connections: number;
    session_replacements: number;
    sessions: DeviceSessionDiagnostics[];
  };
}

async function readSyncError(res: Response, fallback: string): Promise<Error> {
  const body = await res.json().catch(() => null);
  const detail =
    typeof body?.detail === "string"
      ? body.detail
      : typeof body?.message === "string"
      ? body.message
      : fallback;
  return new Error(detail);
}

export const syncApi = {
  getConfig: async (): Promise<SyncConfigResponse> => {
    const res = await fetch(getApiUrl("/sync/config"), {
      headers: authHeaders(),
    });
    if (!res.ok) throw new Error("Failed to get sync config");
    return res.json();
  },

  getStatus: async (): Promise<SyncStatusResponse> => {
    const res = await fetch(getApiUrl("/sync/status"), {
      headers: authHeaders(),
    });
    if (!res.ok) throw new Error("Failed to get sync status");
    return res.json();
  },

  enable: async (): Promise<SyncEnableResponse> => {
    const res = await fetch(getApiUrl("/sync/enable"), {
      method: "POST",
      headers: authHeaders(),
    });
    if (!res.ok) throw new Error("Failed to enable sync");
    return res.json();
  },

  createNetwork: async (): Promise<SyncCreateNetworkResponse> => {
    const res = await fetch(getApiUrl("/sync/network/create"), {
      method: "POST",
      headers: authHeaders(),
    });
    if (!res.ok)
      throw await readSyncError(res, "Failed to create sync network");
    return res.json();
  },

  disable: async (): Promise<SyncDisableResponse> => {
    const res = await fetch(getApiUrl("/sync/disable"), {
      method: "POST",
      headers: authHeaders(),
    });
    if (!res.ok) throw new Error("Failed to disable sync");
    return res.json();
  },

  triggerSyncCycle: async (): Promise<SyncCycleResponse> => {
    const res = await fetch(getApiUrl("/sync/cycle"), {
      method: "POST",
      headers: authHeaders(),
    });
    if (!res.ok) {
      const err: SyncCycleError = await res.json().catch(() => ({
        reason: "peer_unreachable" as const,
        detail: "Network error",
      }));
      const error = new Error(err.detail) as Error & {
        reason: SyncErrorReason;
      };
      error.reason = err.reason;
      throw error;
    }
    return res.json();
  },

  getDiagnostics: async (): Promise<SyncDiagnosticsResponse> => {
    const res = await fetch(getApiUrl("/sync/diagnostics"), {
      headers: authHeaders(),
    });
    if (!res.ok)
      throw await readSyncError(res, "Failed to get sync diagnostics");
    return res.json();
  },

  retryFailedInbox: async (): Promise<{ retried: number }> => {
    const res = await fetch(getApiUrl("/sync/inbox/retry-failed"), {
      method: "POST",
      headers: authHeaders(),
    });
    if (!res.ok)
      throw await readSyncError(res, "Failed to retry failed sync items");
    return res.json();
  },

  getPowerPolicy: async (): Promise<SyncPowerPolicyResponse> => {
    const res = await fetch(getApiUrl("/sync/power-policy"), {
      headers: authHeaders(),
    });
    if (!res.ok)
      throw await readSyncError(res, "Failed to get sync power policy");
    return res.json();
  },

  updatePowerPolicy: async (
    policy: SyncPowerPolicyConfig,
  ): Promise<SyncPowerPolicyResponse> => {
    const res = await fetch(getApiUrl("/sync/power-policy"), {
      method: "PUT",
      headers: authHeaders(),
      body: JSON.stringify(policy),
    });
    if (!res.ok)
      throw await readSyncError(res, "Failed to save sync power policy");
    return res.json();
  },
};

export type ConflictPolicy = "silent" | "review";

export interface ConflictPolicyResponse {
  policy: ConflictPolicy;
}

export const conflictPolicyApi = {
  get: async (): Promise<ConflictPolicyResponse> => {
    const res = await fetch(getApiUrl("/sync/conflict-policy"), {
      headers: authHeaders(),
    });
    if (!res.ok) throw new Error("Failed to get conflict policy");
    return res.json();
  },

  set: async (policy: ConflictPolicy): Promise<ConflictPolicyResponse> => {
    const res = await fetch(getApiUrl("/sync/conflict-policy"), {
      method: "PUT",
      headers: authHeaders(),
      body: JSON.stringify({ policy }),
    });
    if (!res.ok) throw new Error("Failed to set conflict policy");
    return res.json();
  },
};

export interface ConflictEntry {
  id: number;
  op_kind: string;
  target_id: string;
  before_value: string;
  after_value: string;
  originating_device: string;
  hlc: string;
  observed_at: number;
}

export interface RecentConflictsResponse {
  conflicts: ConflictEntry[];
  count: number;
}

export interface PendingConflictEntry {
  id: string;
  live_path: string;
  record_id: string;
  winner_text: string;
  loser_text: string;
  created_at_ms: number;
}

export interface PendingConflictsResponse {
  conflicts: PendingConflictEntry[];
}

export const conflictsApi = {
  getRecent: async (): Promise<RecentConflictsResponse> => {
    const res = await fetch(getApiUrl("/sync/conflicts/recent"), {
      headers: authHeaders(),
    });
    if (!res.ok) throw new Error("Failed to get recent conflicts");
    return res.json();
  },

  getPending: async (): Promise<PendingConflictsResponse> => {
    const res = await fetch(getApiUrl("/sync/conflicts/pending"), {
      headers: authHeaders(),
    });
    if (!res.ok) throw new Error("Failed to get pending conflicts");
    return res.json();
  },

  resolve: async (id: string, chosen: string): Promise<void> => {
    const res = await fetch(getApiUrl("/sync/conflicts/resolve"), {
      method: "POST",
      headers: authHeaders(),
      body: JSON.stringify({ id, chosen }),
    });
    if (!res.ok) throw new Error("Failed to resolve conflict");
  },
};

export const pairingApi = {
  initiate: async (): Promise<InitiateResponse> => {
    const res = await fetch(getApiUrl("/sync/pairing/initiate"), {
      method: "POST",
      headers: authHeaders(),
    });
    if (!res.ok) {
      const err = await res.json().catch(() => ({}));
      throw new Error(err.detail || "Failed to initiate pairing");
    }
    return res.json();
  },

  status: async (): Promise<PairingStatusResponse> => {
    const res = await fetch(getApiUrl("/sync/pairing/status"), {
      headers: authHeaders(),
    });
    if (!res.ok) {
      const err = await res.json().catch(() => ({}));
      throw new Error(err.detail || "Failed to get pairing status");
    }
    return res.json();
  },

  cancel: async (): Promise<CancelResponse> => {
    const res = await fetch(getApiUrl("/sync/pairing/cancel"), {
      method: "POST",
      headers: authHeaders(),
    });
    if (!res.ok) {
      const err = await res.json().catch(() => ({}));
      throw new Error(err.detail || "Failed to cancel pairing");
    }
    return res.json();
  },

  join: async (pin: string): Promise<JoinResponse> => {
    const res = await fetch(getApiUrl("/sync/pairing/join"), {
      method: "POST",
      headers: authHeaders(),
      body: JSON.stringify({ pin }),
    });
    if (!res.ok) {
      const err: JoinError = await res.json().catch(() => ({
        detail: "Connection failed",
        reason: "network" as const,
      }));
      const error = new Error(err.detail) as Error & { reason: string };
      error.reason = err.reason;
      throw error;
    }
    return res.json();
  },
};
