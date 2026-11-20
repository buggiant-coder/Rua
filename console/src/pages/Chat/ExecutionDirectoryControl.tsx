import { useState } from "react";
import { Folder } from "lucide-react";
import { useTranslation } from "react-i18next";
import {
  executionDirectoryApi,
  validateExecutionPosition,
} from "@/api/modules/executionDirectory";
import type { DeviceDto } from "@/api/modules/sync";
import type {
  ExecutionPosition,
  ChatHistory,
  ResolvedExecutionDirectory,
} from "@/api/types";
import { Btn, Modal } from "@/ui";
import { extractFriendlyMessage } from "@/utils/error";
import ExecutionDirectoryFields from "./ExecutionDirectoryFields";
import "./chat-device-chip.css";

const blockerKeys = {
  archived_chat: "archived",
  archived_project: "archived",
  active_turn: "activeTurn",
  queued_input: "queuedInput",
  unresolved_approval: "approval",
} as const;

export default function ExecutionDirectoryControl({
  chatId,
  devices,
  draft,
  position,
  history,
  disabled,
  archived,
  onDraftChange,
  onSaved,
}: {
  chatId: string;
  devices: DeviceDto[];
  draft: boolean;
  position: ExecutionPosition | null;
  history: ChatHistory | null;
  disabled?: boolean;
  archived?: boolean;
  onDraftChange: (position: ExecutionPosition) => void;
  onSaved: (directory: ResolvedExecutionDirectory) => void;
}) {
  const { t } = useTranslation();
  const [editing, setEditing] = useState<ExecutionPosition | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const stored = history?.meta?.execution_directory;
  const saved =
    !position || stored?.device_id === position.device_id ? stored : undefined;
  const current = draft ? position : history ? saved ?? position : null;
  const resolvedPath = draft ? position?.path : saved?.path;
  const control = history?.control;
  const blockers =
    control?.chat_agent_state?.blockers ??
    history?.meta?.chat_agent_state?.blockers ??
    [];
  const stateBlocker = blockers.find(
    (reason): reason is keyof typeof blockerKeys => reason in blockerKeys,
  );
  const blocker = archived
    ? "archived_project"
    : !draft &&
      (control?.active_turn ||
        history?.active_turn ||
        (!control && history?.status === "running"))
    ? "active_turn"
    : !draft && control?.queue?.entries?.length
    ? "queued_input"
    : stateBlocker;
  const blockedReason = blocker
    ? t(`executionLocation.blockers.${blockerKeys[blocker]}`)
    : undefined;
  const device = devices.find((d) => d.device_id === current?.device_id);
  const name = device?.is_self
    ? t("chat.deviceChip.localShort")
    : device?.friendly_name || device?.label || current?.device_id;
  const label = [name, resolvedPath || t("executionLocation.managed")]
    .filter(Boolean)
    .join(" · ");
  function open() {
    if (!current || disabled || blockedReason) return;
    setEditing({
      device_id: current.device_id,
      ...(current.path && (draft || !saved?.managed)
        ? { path: current.path }
        : {}),
    });
    setError("");
  }
  async function save() {
    if (!editing || saving || blockedReason || editing.path?.trim() === "")
      return;
    setSaving(true);
    setError("");
    try {
      const position = await validateExecutionPosition(editing);
      if (draft) onDraftChange(position);
      else onSaved(await executionDirectoryApi.update(chatId, position));
      setEditing(null);
    } catch (cause) {
      const detail = extractFriendlyMessage(cause);
      const reasons = detail.includes("Execution directory change blocked by")
        ? Object.entries(blockerKeys)
            .filter(([name]) => detail.includes(name))
            .map(([, key]) => t(`executionLocation.blockers.${key}`))
        : [];
      setError(reasons.length ? [...new Set(reasons)].join(" ") : detail);
    } finally {
      setSaving(false);
    }
  }
  return (
    <>
      <Btn
        className="rua-chat-execution-directory"
        variant="ghost"
        size="sm"
        aria-label={t("executionLocation.title")}
        title={blockedReason ?? label}
        disabled={!current || Boolean(blockedReason) || disabled}
        onClick={open}
      >
        <Folder size={13} />
        <span>{label}</span>
      </Btn>
      {editing && (
        <Modal
          open
          title={t("executionLocation.title")}
          onClose={() => {
            if (!saving) setEditing(null);
          }}
          onOk={save}
          okText={t("common.save")}
          cancelText={t("common.cancel")}
          confirmLoading={saving}
          okButtonProps={{
            disabled:
              saving || Boolean(blockedReason) || editing.path?.trim() === "",
          }}
          width={520}
        >
          <ExecutionDirectoryFields
            devices={devices}
            position={editing}
            lockDevice={!draft}
            disabled={saving}
            onChange={(next) => {
              if (next) setEditing(next);
              setError("");
            }}
            onError={(cause) => setError(extractFriendlyMessage(cause))}
          />
          {blockedReason && <p role="status">{blockedReason}</p>}
          {error && <p role="alert">{error}</p>}
        </Modal>
      )}
    </>
  );
}
