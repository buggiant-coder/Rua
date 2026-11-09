import { useEffect, useRef } from "react";
import { buildAuthHeaders } from "@/api/authHeaders";
import { getApiUrl } from "@/api/config";
import type { ChatHistory, Message } from "@/api/types";

export type LiveActivityStartOptions = {
  passive?: boolean;
};

export type LiveActivityPendingSubmission = {
  device_delivery_id: string;
  target_device_id: string;
};

export type ConversationLiveActivityViewerLifecycleOptions = {
  viewKey: string | undefined;
  history: ChatHistory | null;
  currentDeviceId?: string | null;
  remotePendingSubmissions: LiveActivityPendingSubmission[];
  isWatchingLiveActivity: () => boolean;
  startLiveActivityViewer: (options?: LiveActivityStartOptions) => boolean;
  loadStableHistory: () => Promise<ChatHistory | null>;
  applyStableHistory: (history: ChatHistory) => void;
  onRemotePendingStableHistory: (history: ChatHistory) => void;
  viewerVisible: boolean;
  wakeSignal?: number;
};

export async function triggerTargetedSyncCycle(targetDeviceId: string) {
  const response = await fetch(getApiUrl("/sync/cycle/target"), {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      ...buildAuthHeaders(),
    },
    body: JSON.stringify({ target_device_id: targetDeviceId }),
  });
  if (!response.ok) {
    const body = await response.text().catch(() => "");
    throw new Error(
      `Targeted sync failed: ${response.status}${body ? ` ${body}` : ""}`,
    );
  }
}

export function deviceDeliveryIdFromMessage(message: Message): string | null {
  return metadataStringFromMessage(message, "device_delivery_id");
}

export function submissionIdFromMessage(message: Message): string | null {
  return metadataStringFromMessage(message, "submission_id");
}

function metadataStringFromMessage(
  message: Message | null | undefined,
  key: string,
): string | null {
  if (!message || typeof message !== "object") return null;
  const metadata = message.metadata;
  if (!metadata || typeof metadata !== "object") return null;
  const direct = (metadata as Record<string, unknown>)[key];
  if (typeof direct === "string" && direct) return direct;
  const nested = (metadata as Record<string, unknown>).metadata;
  if (!nested || typeof nested !== "object") return null;
  const nestedId = (nested as Record<string, unknown>)[key];
  return typeof nestedId === "string" && nestedId ? nestedId : null;
}

function deviceDeliverySourceDeviceFromMessage(
  message: Message,
): string | null {
  return metadataStringFromMessage(message, "source_device_id");
}

export function deviceDeliveryIdsInHistory(history: ChatHistory | null) {
  return new Set(
    (history?.messages || [])
      .map(deviceDeliveryIdFromMessage)
      .filter((id): id is string => Boolean(id)),
  );
}

export function completedDeviceDeliveryIdsInHistory(
  history: ChatHistory | null,
) {
  const completed = new Set<string>();
  let openDeviceDeliveryId: string | null = null;
  for (const message of history?.messages || []) {
    if (!message || typeof message !== "object") continue;
    const role = String(message.role || "");
    if (role === "user") {
      openDeviceDeliveryId = deviceDeliveryIdFromMessage(message);
      continue;
    }
    if (openDeviceDeliveryId && messageTerminatesOpenDeviceDelivery(message)) {
      completed.add(openDeviceDeliveryId);
      openDeviceDeliveryId = null;
    }
  }
  return completed;
}

export function openDeviceDeliveryId(
  history: ChatHistory | null,
  currentDeviceId?: string | null,
): string | null {
  let openId: string | null = null;
  let openSourceDeviceId: string | null = null;
  for (const message of history?.messages || []) {
    if (!message || typeof message !== "object") continue;
    const role = String(message.role || "");
    if (role === "user") {
      openId = deviceDeliveryIdFromMessage(message);
      openSourceDeviceId = openId
        ? deviceDeliverySourceDeviceFromMessage(message)
        : null;
      continue;
    }
    if (openId && messageTerminatesOpenDeviceDelivery(message)) return null;
  }
  if (
    openId &&
    currentDeviceId &&
    openSourceDeviceId &&
    openSourceDeviceId !== currentDeviceId
  ) {
    return null;
  }
  return openId;
}

function messageTerminatesOpenDeviceDelivery(message: Message): boolean {
  if (String(message.role || "") === "assistant") return true;
  if (message.type === "turn_aborted" || message.type === "turn_interjected") {
    return true;
  }
  const runtimeNote = (message as { runtime_note?: unknown }).runtime_note;
  if (!runtimeNote || typeof runtimeNote !== "object") return false;
  const payload = (runtimeNote as Record<string, unknown>).payload;
  if (!payload || typeof payload !== "object") return false;
  const terminalStatus = (payload as Record<string, unknown>).terminal_status;
  return (
    terminalStatus === "completed" ||
    terminalStatus === "failed" ||
    terminalStatus === "interrupted" ||
    terminalStatus === "interjected"
  );
}

function uniqueTargetIds(submissions: LiveActivityPendingSubmission[]) {
  return Array.from(
    new Set(
      submissions
        .map((submission) => submission.target_device_id)
        .filter(Boolean),
    ),
  );
}

export function useConversationLiveActivityViewerLifecycle({
  viewKey,
  history,
  currentDeviceId,
  remotePendingSubmissions,
  isWatchingLiveActivity,
  startLiveActivityViewer,
  loadStableHistory,
  applyStableHistory,
  onRemotePendingStableHistory,
  viewerVisible,
  wakeSignal,
}: ConversationLiveActivityViewerLifecycleOptions) {
  const remotePendingSubmissionsRef = useRef(remotePendingSubmissions);
  const openDeviceDeliveryIdRef = useRef<string | null>(null);
  const lastWakeSignalRef = useRef(wakeSignal);
  const previousViewerVisibleRef = useRef(viewerVisible);

  useEffect(() => {
    remotePendingSubmissionsRef.current = remotePendingSubmissions;
  }, [remotePendingSubmissions]);

  useEffect(() => {
    openDeviceDeliveryIdRef.current = null;
  }, [viewKey]);

  useEffect(() => {
    const wasVisible = previousViewerVisibleRef.current;
    previousViewerVisibleRef.current = viewerVisible;
    if (!viewKey || !viewerVisible || wasVisible) return;

    let cancelled = false;
    void loadStableHistory()
      .then((nextHistory) => {
        if (cancelled || !nextHistory) return;
        applyStableHistory(nextHistory);
        onRemotePendingStableHistory(nextHistory);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [
    applyStableHistory,
    viewKey,
    loadStableHistory,
    onRemotePendingStableHistory,
    viewerVisible,
  ]);

  useEffect(() => {
    if (!viewKey || !viewerVisible) return;
    if (wakeSignal === undefined || lastWakeSignalRef.current === wakeSignal) {
      return;
    }
    lastWakeSignalRef.current = wakeSignal;
    if (isWatchingLiveActivity()) return;
    startLiveActivityViewer({ passive: true });
  }, [
    viewKey,
    isWatchingLiveActivity,
    startLiveActivityViewer,
    viewerVisible,
    wakeSignal,
  ]);

  useEffect(() => {
    if (!viewKey || !viewerVisible || remotePendingSubmissions.length === 0)
      return;

    let cancelled = false;
    let inFlight = false;

    const refreshPendingResults = async () => {
      if (cancelled || inFlight) return;
      const current = remotePendingSubmissionsRef.current;
      if (current.length === 0) return;
      inFlight = true;
      try {
        await Promise.allSettled(
          uniqueTargetIds(current).map(triggerTargetedSyncCycle),
        );
        const nextHistory = await loadStableHistory();
        if (cancelled || !nextHistory) return;
        applyStableHistory(nextHistory);
        onRemotePendingStableHistory(nextHistory);
      } catch {
      } finally {
        inFlight = false;
      }
    };

    void refreshPendingResults();

    return () => {
      cancelled = true;
    };
  }, [
    applyStableHistory,
    viewKey,
    loadStableHistory,
    onRemotePendingStableHistory,
    remotePendingSubmissions.length,
    viewerVisible,
    wakeSignal,
  ]);

  useEffect(() => {
    if (!viewKey || !viewerVisible) return;
    if (!isWatchingLiveActivity()) {
      startLiveActivityViewer({ passive: true });
    }
  }, [viewKey, isWatchingLiveActivity, startLiveActivityViewer, viewerVisible]);

  useEffect(() => {
    const submissionId = openDeviceDeliveryId(history, currentDeviceId);
    if (!submissionId) {
      openDeviceDeliveryIdRef.current = null;
      return;
    }
    if (
      !viewKey ||
      !viewerVisible ||
      openDeviceDeliveryIdRef.current === submissionId ||
      isWatchingLiveActivity()
    ) {
      return;
    }

    openDeviceDeliveryIdRef.current = submissionId;
    startLiveActivityViewer();
  }, [
    viewKey,
    currentDeviceId,
    history,
    isWatchingLiveActivity,
    startLiveActivityViewer,
    viewerVisible,
  ]);
}
