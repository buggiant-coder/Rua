import { useEffect, useRef, useState } from "react";

import { subscribeJsonLines } from "@/api/jsonLineSubscription";
import { useRouteViewerVisible } from "@/routes/RouteActivity";

export type ConversationActivityScope =
  | { scope: "agent"; agentId: string }
  | { scope: "project"; projectId: string };

export type ConversationActivityWake = {
  sequence: number;
  /** Retained across coalesced wakes until the Runtime has reconciled recovery. */
  recoverySequence?: number;
  type: "chat_live_wake" | "activity_reconnected";
  version?: number;
  address: {
    scope:
      | { kind: "agent"; agent_id: string }
      | { kind: "project"; project_id: string };
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
  agent_id?: string;
  project_id?: string;
  chat_id?: string;
  stream_id?: string;
  phase?: string;
  authority_device_id?: string | null;
  authority_epoch?: string;
  base_manifest_hash?: string;
  address?: {
    scope?: {
      kind?: string;
      agent_id?: string;
      project_id?: string;
    };
    chat_id?: string;
  };
};

export function useConversationActivityWake(
  target: ConversationActivityScope,
  chatId: string | undefined,
): ConversationActivityWake | null {
  const visible = useRouteViewerVisible();
  const [latestWake, setLatestWake] = useState<ConversationActivityWake | null>(
    null,
  );
  const scope = target.scope;
  const ownerId = target.scope === "agent" ? target.agentId : target.projectId;

  const lastSubscriptionPath = useRef<string | null>(null);
  useEffect(() => {
    if (!visible || !chatId) return;
    const path = activityPath(scope, ownerId, chatId);
    const resuming = lastSubscriptionPath.current === path;
    lastSubscriptionPath.current = path;
    const publish = (payload: ChatActivityWakePayload, recovered = false) => {
      setLatestWake((current) => {
        const sequence = (current?.sequence ?? 0) + 1;
        const recoverySequence = recovered
          ? sequence
          : current && activityMatches(current, scope, ownerId, chatId)
          ? current.recoverySequence
          : undefined;
        return {
          ...normalizeActivityWake(payload, scope, ownerId, chatId, sequence),
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
          activityMatches(payload, scope, ownerId, chatId)
        ) {
          publish(payload);
        }
      },
    });
    return subscription.stop;
  }, [chatId, ownerId, scope, visible]);

  return visible &&
    chatId &&
    latestWake &&
    activityMatches(latestWake, scope, ownerId, chatId)
    ? latestWake
    : null;
}

function normalizeActivityWake(
  payload: ChatActivityWakePayload,
  scope: ConversationActivityScope["scope"],
  ownerId: string,
  chatId: string,
  sequence: number,
): ConversationActivityWake {
  const address =
    scope === "agent"
      ? {
          scope: { kind: "agent" as const, agent_id: ownerId },
          chat_id: chatId,
        }
      : {
          scope: { kind: "project" as const, project_id: ownerId },
          chat_id: chatId,
        };
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

function activityPath(
  scope: ConversationActivityScope["scope"],
  ownerId: string,
  chatId: string,
): string {
  if (scope === "project") {
    return `/projects/${encodeURIComponent(ownerId)}/chats/${encodeURIComponent(
      chatId,
    )}/console/activity`;
  }
  return `/agents/${encodeURIComponent(ownerId)}/chats/${encodeURIComponent(
    chatId,
  )}/console/activity`;
}

function activityMatches(
  payload: ChatActivityWakePayload,
  scope: ConversationActivityScope["scope"],
  ownerId: string,
  chatId: string,
): boolean {
  const address = payload.address;
  if (address) {
    if (address.chat_id !== chatId || address.scope?.kind !== scope) {
      return false;
    }
    return scope === "agent"
      ? address.scope.agent_id === ownerId
      : address.scope.project_id === ownerId;
  }
  return (
    payload.chat_id === chatId &&
    (scope === "agent"
      ? payload.agent_id === ownerId
      : payload.project_id === ownerId)
  );
}
