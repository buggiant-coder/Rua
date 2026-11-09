import { useEffect, useState } from "react";
import { Bug, Copy, FolderOpen } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast, Toggle } from "@/ui";
import { chatApi, type LlmDebugCaptureStatus } from "../../api/modules/chat";
import { agentsApi } from "../../api/modules/agents";
import { projectChatsApi } from "../../api/modules/projectChats";
import {
  isDesktopShell,
  openLlmDebugCaptureDirectory,
} from "../../desktop/runtime";

interface LlmDebugCaptureControlProps {
  agentId: string;
  chatId: string;
  projectId?: string;
}

export default function LlmDebugCaptureControl({
  agentId,
  chatId,
  projectId,
}: LlmDebugCaptureControlProps) {
  const { t } = useTranslation();
  const [debugVisible, setDebugVisible] = useState(false);
  const [status, setStatus] = useState<LlmDebugCaptureStatus>({
    active: false,
  });
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setDebugVisible(false);
    setStatus({ active: false });
    agentsApi
      .getAgentRunningConfig(agentId)
      .then(async (config) => {
        if (cancelled || !config.show_debug_menu) return undefined;
        setDebugVisible(true);
        return projectId
          ? projectChatsApi.getLlmDebugCapture(projectId, chatId)
          : chatApi.getLlmDebugCapture(chatId, agentId);
      })
      .then((next) => {
        if (!cancelled && next) setStatus(next);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [agentId, chatId, projectId]);

  async function setCaptureEnabled(enabled: boolean) {
    if (enabled === status.active) return;
    setBusy(true);
    try {
      if (projectId) {
        setStatus(
          enabled
            ? await projectChatsApi.startLlmDebugCapture(projectId, chatId)
            : await projectChatsApi.stopLlmDebugCapture(projectId, chatId),
        );
      } else {
        setStatus(
          enabled
            ? await chatApi.startLlmDebugCapture(chatId, agentId)
            : await chatApi.stopLlmDebugCapture(chatId, agentId),
        );
      }
    } finally {
      setBusy(false);
    }
  }

  async function openOrCopy() {
    if (!status.path) return;
    if (desktop) {
      try {
        await openLlmDebugCaptureDirectory(status.path);
      } catch (error) {
        console.error("Failed to open LLM Debug Capture directory:", error);
        toast.error(t("chat.debugCapture.openDirectoryFailed"));
      }
      return;
    }
    try {
      await navigator.clipboard.writeText(status.path);
      toast.success(t("common.copied"));
    } catch {
      toast.error(t("common.copyFailed"));
    }
  }

  if (!debugVisible) return null;

  const desktop = isDesktopShell();

  return (
    <div className={`rua-llm-debug-menu${status.active ? " is-active" : ""}`}>
      <div className="rua-llm-debug-menu-heading">
        <span className="rua-llm-debug-menu-title">
          <Bug size={14} aria-hidden="true" />
          <span>{t("chat.debugCapture.title")}</span>
        </span>
        <Toggle
          checked={status.active}
          disabled={busy}
          aria-label={t("chat.debugCapture.toggle")}
          onChange={(event) => void setCaptureEnabled(event.target.checked)}
        />
      </div>
      {status.active && status.path && (
        <div className="rua-llm-debug-menu-details">
          {status.capture_id && (
            <div
              className="rua-llm-debug-menu-capture-id"
              title={status.capture_id}
            >
              {status.capture_id}
            </div>
          )}
          <div className="rua-llm-debug-menu-path" title={status.path}>
            {status.path}
          </div>
          <button
            type="button"
            className="rua-llm-debug-menu-open"
            aria-label={
              desktop
                ? t("chat.debugCapture.openDirectory")
                : t("chat.debugCapture.copyPath")
            }
            onClick={openOrCopy}
          >
            {desktop ? (
              <FolderOpen size={14} aria-hidden="true" />
            ) : (
              <Copy size={14} aria-hidden="true" />
            )}
            <span>
              {desktop
                ? t("chat.debugCapture.openDirectory")
                : t("chat.debugCapture.copyPath")}
            </span>
          </button>
        </div>
      )}
    </div>
  );
}
