import { useCallback, useEffect, useRef, useState } from "react";
import type { ChatHistory, ModelSlot, ExecutionPosition } from "@/api/types";
import {
  loadDraftConversationModel,
  materializeConversation,
  saveDraftConversationModel,
  loadDraftExecutionPosition,
  saveDraftExecutionPosition,
  draftCreationStarted,
  markDraftCreationStarted,
} from "./conversationLifecycle";
import { historyProvesConversationMaterialized } from "./conversationRouteController";

type ConversationLifecycleOptions = {
  scope: string;
  conversationId: string;
  unmaterialized: boolean;
  onMaterialized?: () => void;
  initialPosition?: ExecutionPosition | null;
};

export function useConversationLifecycle({
  scope,
  conversationId,
  unmaterialized,
  initialPosition,
  onMaterialized,
}: ConversationLifecycleOptions) {
  const identity = `${scope}\u0000${conversationId}`;
  const [trackedIdentity, setTrackedIdentity] = useState(identity);
  const [lazy, setLazy] = useState(unmaterialized);
  const [creationStarted, setCreationStarted] = useState(
    () => unmaterialized && draftCreationStarted(scope, conversationId),
  );
  const [draftPosition, setDraftPosition] = useState<ExecutionPosition | null>(
    () =>
      unmaterialized ? loadDraftExecutionPosition(scope, conversationId) : null,
  );
  const [draftModel, setDraftModel] = useState<ModelSlot | null>(() =>
    unmaterialized ? loadDraftConversationModel(scope, conversationId) : null,
  );
  const identityChanged = trackedIdentity !== identity;
  const currentLazy = identityChanged ? unmaterialized : lazy;
  const currentCreationStarted = identityChanged
    ? unmaterialized && draftCreationStarted(scope, conversationId)
    : creationStarted;
  const currentDraftPosition = identityChanged
    ? unmaterialized
      ? loadDraftExecutionPosition(scope, conversationId)
      : null
    : draftPosition;
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
    setDraftPosition(currentDraftPosition);
    setDraftModel(currentDraftModel);
  }

  const initialDeviceId = initialPosition?.device_id;
  const initialPath = initialPosition?.path;
  useEffect(() => {
    if (
      !currentLazy ||
      !conversationId ||
      currentDraftPosition ||
      !initialDeviceId
    )
      return;
    const position = {
      device_id: initialDeviceId,
      ...(initialPath ? { path: initialPath } : {}),
    };
    saveDraftExecutionPosition(scope, conversationId, position);
    setDraftPosition(position);
  }, [
    conversationId,
    currentDraftPosition,
    currentLazy,
    initialDeviceId,
    initialPath,
    scope,
  ]);

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
    setDraftPosition(null);
    setCreationStarted(false);
    markDraftCreationStarted(scope, conversationId, false);
    saveDraftExecutionPosition(scope, conversationId, null);
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
    draftPosition: currentLazy
      ? currentDraftPosition ?? initialPosition ?? null
      : null,
    creationStarted: currentCreationStarted,
    setDraftPosition: (position: ExecutionPosition) => {
      if (!lazyRef.current) return;
      setDraftPosition(position);
      saveDraftExecutionPosition(scope, conversationId, position);
    },
    setDraftModel: saveDraftModel,
    statefulOperationAccepted: confirmMaterialized,
    authoritativeHistoryObserved: historyObserved,
  };
}
