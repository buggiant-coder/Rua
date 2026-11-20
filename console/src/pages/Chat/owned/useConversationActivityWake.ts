import { useEffect, useRef, useState } from "react";

import { subscribeJsonLines } from "@/api/jsonLineSubscription";
import { useRouteViewerVisible } from "@/routes/RouteActivity";

export type ConversationActivityWake = {
  sequence: number;
  /** Retained across coalesced wakes until the Runtime has reconciled recovery. */
  recoverySequence?: number;
  type: "chat_live_wake" | "activity_reconnected";
  version?: number;
  address: {
    chat_id: string;
  };
  chat_id: string;
  stream_id?: string;
  phase?: string;
  authority_device_id?: string | null;
  authority_epoch?: string;
  base_manifest_hash?: string;
};

type ChatActivityWakePayload = {
  type?: string;
  version?: number;
  chat_id?: string;
  stream_id?: string;
  phase?: string;
  authority_device_id?: string | null;
  authority_epoch?: string;
  base_manifest_hash?: string;
  address?: {
    chat_id?: string;
  };
};

export function useConversationActivityWake(
  chatId: string | undefined,
): ConversationActivityWake | null {
  const visible = useRouteViewerVisible();
  const [latestWake, setLatestWake] = useState<ConversationActivityWake | null>(
    null,
  );

  const lastSubscriptionPath = useRef<string | null>(null);
  useEffect(() => {
    if (!visible || !chatId) return;
    const path = activityPath(chatId);
    const resuming = lastSubscriptionPath.current === path;
    lastSubscriptionPath.current = path;
    const publish = (payload: ChatActivityWakePayload, recovered = false) => {
      setLatestWake((current) => {
        const sequence = (current?.sequence ?? 0) + 1;
        const recoverySequence = recovered
          ? sequence
          : current && activityMatches(current, chatId)
          ? current.recoverySequence
          : undefined;
        return {
          ...normalizeActivityWake(payload, chatId, sequence),
          ...(recoverySequence === undefined ? {} : { recoverySequence }),
          type: recovered ? "activity_reconnected" : "chat_live_wake",
        };
      });
    };
    const subscription = subscribeJsonLines({
      path,
      onConnected: (reconnected) => {
        if (reconnected || resuming) publish({}, true);
      },
      onMessage: (message) => {
        if (!message || typeof message !== "object") return;
        const payload = message as ChatActivityWakePayload;
        if (
          payload.type === "chat_live_wake" &&
          activityMatches(payload, chatId)
        ) {
          publish(payload);
        }
      },
    });
    return subscription.stop;
  }, [chatId, visible]);

  return visible && chatId && latestWake && activityMatches(latestWake, chatId)
    ? latestWake
    : null;
}

function normalizeActivityWake(
  payload: ChatActivityWakePayload,
  chatId: string,
  sequence: number,
): ConversationActivityWake {
  const address = { chat_id: chatId };
  return {
    sequence,
    type: "chat_live_wake",
    address,
    chat_id: chatId,
    ...(payload.version !== undefined ? { version: payload.version } : {}),
    ...(payload.stream_id !== undefined
      ? { stream_id: payload.stream_id }
      : {}),
    ...(payload.phase !== undefined ? { phase: payload.phase } : {}),
    ...(payload.authority_device_id !== undefined
      ? { authority_device_id: payload.authority_device_id }
      : {}),
    ...(payload.authority_epoch !== undefined
      ? { authority_epoch: payload.authority_epoch }
      : {}),
    ...(payload.base_manifest_hash !== undefined
      ? { base_manifest_hash: payload.base_manifest_hash }
      : {}),
  };
}

function activityPath(chatId: string): string {
  return `/chats/${encodeURIComponent(chatId)}/console/activity`;
}

function activityMatches(
  payload: ChatActivityWakePayload,
  chatId: string,
): boolean {
  return (payload.address?.chat_id ?? payload.chat_id) === chatId;
}
