import type {
  ACPChatStatus,
  ChatHistory,
  ConversationControlView,
} from "@/api/types";

export type ChatAcpBinding = {
  deviceId: string | null;
  runner: string | null;
  runnerSessionId: string | null;
  cwd: string | null;
};

export type ChatPlacementLockReason = "acp_active";

export type ChatPlacementState = {
  authorityPlacementDeviceId: string | null;
  acpStatus: ACPChatStatus | null;
  acpActive: boolean;
  acpStatusUnknown: boolean;
  authorityPlacementLocked: boolean;
  lockReason: ChatPlacementLockReason | null;
};

export type ResolveChatPlacementInput = {
  chatId: string | undefined;
  meta: Record<string, unknown> | undefined;
  polledAcpStatus?: ACPChatStatus | null;
  currentDeviceId?: string | null;
  workspaceDirFallback?: string;
  requireAcpStatus?: boolean;
};

export function chatAuthorityPlacementFromMeta(
  meta: Record<string, unknown> | undefined,
): string | null {
  const raw = meta?.authority_placement;
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return null;
  const placement = raw as Record<string, unknown>;
  const deviceId = placement.device_id;
  return typeof deviceId === "string" && deviceId ? deviceId : null;
}

export function chatAcpBindingFromMeta(
  meta: Record<string, unknown> | undefined,
): ChatAcpBinding | null {
  const raw = meta?.acp_binding;
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return null;
  const binding = raw as Record<string, unknown>;
  const deviceId = binding.device_id;
  const runner = binding.runner;
  const runnerSessionId = binding.runner_session_id;
  const cwd = binding.cwd;
  return {
    deviceId: typeof deviceId === "string" && deviceId ? deviceId : null,
    runner: typeof runner === "string" && runner ? runner : null,
    runnerSessionId:
      typeof runnerSessionId === "string" && runnerSessionId
        ? runnerSessionId
        : null,
    cwd: typeof cwd === "string" && cwd ? cwd : null,
  };
}

export function acpStatusFromConversationControl(
  control: ConversationControlView | null | undefined,
  chatId: string | undefined,
): ACPChatStatus | null {
  if (!control?.acp.active || !chatId) return null;
  const authorityDeviceId = control.placement.authority_device_id;
  const binding = control.acp.binding;
  if (
    typeof authorityDeviceId !== "string" ||
    !authorityDeviceId ||
    !binding ||
    typeof binding !== "object" ||
    Array.isArray(binding)
  ) {
    return null;
  }
  const values = binding as Record<string, unknown>;
  const bindingDeviceId = values.device_id;
  if (bindingDeviceId !== authorityDeviceId) return null;
  const runner = values.runner;
  const runnerSessionId = values.runner_session_id;
  const cwd = values.cwd;
  return {
    active: true,
    runner: typeof runner === "string" && runner ? runner : null,
    session_id:
      typeof runnerSessionId === "string" && runnerSessionId
        ? runnerSessionId
        : null,
    cwd: typeof cwd === "string" && cwd ? cwd : null,
    chat_id: chatId,
    workspace_dir: typeof cwd === "string" ? cwd : "",
    target_device_id: authorityDeviceId,
  };
}

export function acpStatusMatchesAuthorityPlacement(
  status: ACPChatStatus,
  authorityPlacementDeviceId: string | null,
  currentDeviceId: string | null | undefined,
): boolean {
  const statusTargetDeviceId = status.target_device_id ?? null;
  if (statusTargetDeviceId === authorityPlacementDeviceId) return true;
  return (
    statusTargetDeviceId === null &&
    authorityPlacementDeviceId !== null &&
    authorityPlacementDeviceId === (currentDeviceId ?? null)
  );
}

function acpStatusFromBinding(
  binding: ChatAcpBinding | null,
  chatId: string | undefined,
  workspaceDir: string,
): ACPChatStatus | null {
  if (!binding || !chatId) return null;
  return {
    active: true,
    runner: binding.runner,
    session_id: binding.runnerSessionId,
    cwd: binding.cwd,
    chat_id: chatId,
    workspace_dir: workspaceDir,
    target_device_id: binding.deviceId,
  };
}

export function removeAcpBindingFromHistory(
  history: ChatHistory | null,
): ChatHistory | null {
  if (!history?.meta) return history;
  const meta = { ...history.meta };
  delete meta.acp_binding;
  return { ...history, meta };
}

export function resolveChatPlacement({
  chatId,
  meta,
  polledAcpStatus,
  currentDeviceId,
  workspaceDirFallback = "",
  requireAcpStatus = true,
}: ResolveChatPlacementInput): ChatPlacementState {
  const backendAuthorityPlacementDeviceId =
    chatAuthorityPlacementFromMeta(meta);
  const acpBinding = chatAcpBindingFromMeta(meta);
  const authorityPlacementDeviceId = backendAuthorityPlacementDeviceId;
  const matchingAcpBinding =
    acpBinding?.deviceId === authorityPlacementDeviceId ? acpBinding : null;
  const bindingAcpStatus = acpStatusFromBinding(
    matchingAcpBinding,
    chatId,
    polledAcpStatus?.workspace_dir ?? workspaceDirFallback,
  );
  const matchingPolledAcpStatus =
    chatId &&
    authorityPlacementDeviceId &&
    polledAcpStatus?.chat_id === chatId &&
    acpStatusMatchesAuthorityPlacement(
      polledAcpStatus,
      authorityPlacementDeviceId,
      currentDeviceId,
    )
      ? polledAcpStatus
      : null;

  const acpStatus =
    matchingPolledAcpStatus?.active === true
      ? matchingPolledAcpStatus
      : bindingAcpStatus ?? matchingPolledAcpStatus;
  const acpStatusUnknown = Boolean(
    requireAcpStatus &&
      chatId &&
      authorityPlacementDeviceId &&
      acpStatus === null &&
      !matchingAcpBinding,
  );
  const acpActive = acpStatus?.active === true;
  const lockReason: ChatPlacementLockReason | null = acpActive
    ? "acp_active"
    : null;

  return {
    authorityPlacementDeviceId,
    acpStatus,
    acpActive,
    acpStatusUnknown,
    authorityPlacementLocked: lockReason !== null,
    lockReason,
  };
}
