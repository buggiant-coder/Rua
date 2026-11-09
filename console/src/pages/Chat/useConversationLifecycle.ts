import { useCallback, useRef, useState } from "react";
import type { ChatHistory, ModelSlot } from "@/api/types";
import {
  loadDraftConversationModel,
  materializeConversation,
  saveDraftConversationModel,
  loadDraftConversationDevice,
  saveDraftConversationDevice,
  draftCreationStarted,
  markDraftCreationStarted,
} from "./conversationLifecycle";
import { historyProvesConversationMaterialized } from "./conversationRouteController";

type ConversationLifecycleOptions = {
  scope: string;
  conversationId: string;
  unmaterialized: boolean;
  onMaterialized?: () => void;
};

export function useConversationLifecycle({
  scope,
  conversationId,
  unmaterialized,
  onMaterialized,
}: ConversationLifecycleOptions) {
  const identity = `${scope}\u0000${conversationId}`;
  const [trackedIdentity, setTrackedIdentity] = useState(identity);
  const [lazy, setLazy] = useState(unmaterialized);
  const [creationStarted, setCreationStarted] = useState(
    () => unmaterialized && draftCreationStarted(scope, conversationId),
  );
  const [draftDevice, setDraftDevice] = useState<string | null>(() =>
    unmaterialized ? loadDraftConversationDevice(scope, conversationId) : null,
  );
  const [draftModel, setDraftModel] = useState<ModelSlot | null>(() =>
    unmaterialized ? loadDraftConversationModel(scope, conversationId) : null,
  );
  const identityChanged = trackedIdentity !== identity;
  const currentLazy = identityChanged ? unmaterialized : lazy;
  const currentCreationStarted = identityChanged
    ? unmaterialized && draftCreationStarted(scope, conversationId)
    : creationStarted;
  const currentDraftDevice = identityChanged
    ? unmaterialized
      ? loadDraftConversationDevice(scope, conversationId)
      : null
    : draftDevice;
  const currentDraftModel = identityChanged
    ? unmaterialized
      ? loadDraftConversationModel(scope, conversationId)
      : null
    : draftModel;
  const lazyRef = useRef(currentLazy);
  lazyRef.current = currentLazy;

  if (identityChanged) {
    setTrackedIdentity(identity);
    setLazy(currentLazy);
    setCreationStarted(currentCreationStarted);
    setDraftDevice(currentDraftDevice);
    setDraftModel(currentDraftModel);
  }

  const saveDraftModel = useCallback(
    (model: ModelSlot) => {
      setDraftModel(model);
      saveDraftConversationModel(scope, conversationId, model);
    },
    [conversationId, scope],
  );

  const confirmMaterialized = useCallback(() => {
    if (!lazyRef.current) return;
    materializeConversation(scope, conversationId);
    lazyRef.current = false;
    setLazy(false);
    setDraftModel(null);
    setDraftDevice(null);
    setCreationStarted(false);
    markDraftCreationStarted(scope, conversationId, false);
    saveDraftConversationDevice(scope, conversationId, null);
    onMaterialized?.();
  }, [conversationId, onMaterialized, scope]);

  const historyObserved = useCallback(
    (history: ChatHistory) => {
      if (!historyProvesConversationMaterialized(history)) return false;
      const wasLazy = lazyRef.current;
      confirmMaterialized();
      return wasLazy;
    },
    [confirmMaterialized],
  );

  return {
    lazy: currentLazy,
    lazyRef,
    draftModel: currentDraftModel,
    draftDevice: currentDraftDevice,
    creationStarted: currentCreationStarted,
    beginCreation: (device: string) => {
      setDraftDevice(device);
      saveDraftConversationDevice(scope, conversationId, device);
      setCreationStarted(true);
      markDraftCreationStarted(scope, conversationId, true);
    },
    setDraftDevice: (device: string) => {
      setDraftDevice(device);
      saveDraftConversationDevice(scope, conversationId, device);
    },
    setDraftModel: saveDraftModel,
    statefulOperationAccepted: confirmMaterialized,
    authoritativeHistoryObserved: historyObserved,
  };
}
