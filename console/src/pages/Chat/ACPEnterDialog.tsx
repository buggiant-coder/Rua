import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Play, Plus, Zap } from "lucide-react";
import { Btn, Modal, Select, Spinner } from "@/ui";
import { projectsApi } from "@/api/modules/projects";
import DirectoryPicker from "@/components/DirectoryPicker";
import {
  agentConversationTarget,
  conversationApi,
  type ConversationApiTarget,
} from "@/api/modules/conversation";
import type {
  ACPChatSessionEntry,
  ACPChatStatus,
  ACPRunnerInfo,
} from "@/api/types";
import "./acp-enter-dialog.css";
import type { DraftAcpSelection } from "./conversationLifecycle";

function formatSessionTime(raw: string): string {
  if (!raw || !raw.trim()) return "";
  let d = new Date(raw);
  if (isNaN(d.getTime())) d = new Date(raw.replace(" ", "T"));
  if (isNaN(d.getTime())) return raw;
  return d.toLocaleString(undefined, { hour12: false });
}

const CWD_HISTORY_KEY = "rua.acp.cwd_history";
const CWD_HISTORY_MAX = 20;

function readCwdHistory(): string[] {
  try {
    const raw = window.localStorage.getItem(CWD_HISTORY_KEY);
    if (!raw) return [];
    const arr = JSON.parse(raw);
    if (!Array.isArray(arr)) return [];
    return arr.filter(
      (x): x is string => typeof x === "string" && x.trim() !== "",
    );
  } catch {
    return [];
  }
}

function pushCwdHistory(cwd: string) {
  const trimmed = cwd.trim();
  if (!trimmed) return;
  try {
    const existing = readCwdHistory().filter((x) => x !== trimmed);
    const next = [trimmed, ...existing].slice(0, CWD_HISTORY_MAX);
    window.localStorage.setItem(CWD_HISTORY_KEY, JSON.stringify(next));
  } catch {}
}

interface ACPEnterDialogProps {
  open: boolean;
  onClose: () => void;
  agentId: string;
  target?: ConversationApiTarget;

  discoveryTarget?: ConversationApiTarget;
  chatId: string | undefined;
  targetDeviceId?: string | null;

  defaultCwd?: string;
  unmaterialized?: boolean;
  draftSelection?: DraftAcpSelection | null;
  onDraftSelected?: (selection: DraftAcpSelection) => void;

  onEntered: (status: ACPChatStatus, historyPreview?: string | null) => void;
}

export default function ACPEnterDialog({
  open,
  onClose,
  agentId,
  target,
  discoveryTarget,
  chatId,
  targetDeviceId,
  defaultCwd,
  unmaterialized = false,
  draftSelection,
  onDraftSelected,
  onEntered,
}: ACPEnterDialogProps) {
  const { t } = useTranslation();
  const [runners, setRunners] = useState<ACPRunnerInfo[]>([]);
  const [runnersLoading, setRunnersLoading] = useState(false);
  const [runnersError, setRunnersError] = useState<string | null>(null);
  const [runnerKey, setRunnerKey] = useState<string>("");
  const [cwd, setCwd] = useState<string>("");
  const [sessions, setSessions] = useState<ACPChatSessionEntry[] | null>(null);
  const [sessionsLoading, setSessionsLoading] = useState(false);
  const [sessionsError, setSessionsError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);
  const conversationTarget = useMemo(
    () => target ?? agentConversationTarget(agentId),
    [agentId, target],
  );
  const acpDiscoveryTarget = discoveryTarget ?? conversationTarget;

  useEffect(() => {
    if (!open) return;
    setRunners([]);
    setRunnersError(null);
    setRunnerKey("");
    setCwd(draftSelection?.cwd || defaultCwd || "");
    setSessions(null);
    setSessionsError(null);
    setSubmitError(null);
    setSubmitting(false);
    if (!chatId) {
      setRunnersLoading(false);
      setRunnersError(t("chat.acpEnter.missingChat", "缺少对话上下文"));
      return;
    }
    setRunnersLoading(true);
    conversationApi
      .listAcpRunners(acpDiscoveryTarget, chatId, targetDeviceId)
      .then((rs) => {
        const enabled = rs
          .filter((r) => r.enabled)
          .sort((a, b) => a.key.localeCompare(b.key));
        setRunners(enabled);
        if (draftSelection) {
          setRunnerKey(draftSelection.runner);
          if (!enabled.some((runner) => runner.key === draftSelection.runner)) {
            setSubmitError(
              `Selected ACP runner '${draftSelection.runner}' is unavailable`,
            );
          }
        } else if (enabled.length > 0) setRunnerKey(enabled[0].key);
      })
      .catch((e) => setRunnersError(String(e?.message ?? e)))
      .finally(() => setRunnersLoading(false));
  }, [
    open,
    acpDiscoveryTarget,
    agentId,
    chatId,
    targetDeviceId,
    t,
    draftSelection?.runner,
    draftSelection?.cwd,
    defaultCwd,
  ]);

  const trimmedCwd = cwd.trim();
  const runnerOptions = useMemo(
    () =>
      runners.map((runner) => ({
        value: runner.key,
        label: `${runner.key} — ${runner.command}`,
      })),
    [runners],
  );
  const browseDirectory = useCallback(
    async (path: string) => {
      if (!targetDeviceId) {
        throw new Error(t("chat.acpEnter.deviceRequired", "无法确定工作设备"));
      }
      const result = await projectsApi.browseDirectory(
        targetDeviceId,
        path.trim() || defaultCwd || "~",
      );
      return {
        path: result.path,
        parent: result.parent_path,
        directories: result.entries
          .filter((entry) => entry.kind === "directory")
          .map((entry) => ({ name: entry.name, path: entry.path })),
      };
    },
    [defaultCwd, t, targetDeviceId],
  );
  useEffect(() => {
    if (!open || !chatId || !runnerKey) {
      setSessions(null);
      return;
    }
    const handle = window.setTimeout(() => {
      setSessionsLoading(true);
      setSessionsError(null);
      conversationApi
        .listAcpSessions(
          acpDiscoveryTarget,
          chatId,
          runnerKey,
          trimmedCwd || undefined,
          targetDeviceId,
        )
        .then((entries) => setSessions(entries))
        .catch((e) => setSessionsError(String(e?.message ?? e)))
        .finally(() => setSessionsLoading(false));
    }, 350);
    return () => window.clearTimeout(handle);
  }, [
    open,
    agentId,
    chatId,
    acpDiscoveryTarget,
    runnerKey,
    trimmedCwd,
    targetDeviceId,
  ]);

  const enterFresh = useCallback(async () => {
    if (!chatId || !runners.some((runner) => runner.key === runnerKey)) return;
    setSubmitting(true);
    setSubmitError(null);
    try {
      if (unmaterialized) {
        onDraftSelected?.({ runner: runnerKey, cwd: trimmedCwd || undefined });
        onClose();
        return;
      }
      const resp = await conversationApi.enterAcp(conversationTarget, chatId, {
        agent_id: agentId,
        runner: runnerKey,
        cwd: trimmedCwd || undefined,
        target_device_id: targetDeviceId || undefined,
      });
      if (!resp.ok) {
        setSubmitError(resp.message || t("chat.acpEnter.errorGeneric"));
        return;
      }
      if (trimmedCwd) pushCwdHistory(trimmedCwd);
      onEntered(resp.status, resp.history_preview);
      onClose();
    } catch (e) {
      setSubmitError(String((e as { message?: string })?.message ?? e));
    } finally {
      setSubmitting(false);
    }
  }, [
    chatId,
    unmaterialized,
    onDraftSelected,
    conversationTarget,
    agentId,
    runnerKey,
    runners,
    trimmedCwd,
    targetDeviceId,
    onEntered,
    onClose,
    t,
  ]);

  const resumeSession = useCallback(
    async (sessionId: string) => {
      if (!chatId || !runners.some((runner) => runner.key === runnerKey))
        return;
      setSubmitting(true);
      setSubmitError(null);
      try {
        if (unmaterialized) {
          onDraftSelected?.({
            runner: runnerKey,
            cwd: trimmedCwd || undefined,
            resume_session_id: sessionId,
          });
          onClose();
          return;
        }
        const resp = await conversationApi.enterAcp(
          conversationTarget,
          chatId,
          {
            agent_id: agentId,
            runner: runnerKey,
            cwd: trimmedCwd || undefined,
            resume_session_id: sessionId,
            target_device_id: targetDeviceId || undefined,
          },
        );
        if (!resp.ok) {
          setSubmitError(resp.message || t("chat.acpEnter.errorGeneric"));
          return;
        }
        if (trimmedCwd) pushCwdHistory(trimmedCwd);
        onEntered(resp.status, resp.history_preview);
        onClose();
      } catch (e) {
        setSubmitError(String((e as { message?: string })?.message ?? e));
      } finally {
        setSubmitting(false);
      }
    },
    [
      chatId,
      unmaterialized,
      onDraftSelected,
      conversationTarget,
      agentId,
      runnerKey,
      runners,
      trimmedCwd,
      targetDeviceId,
      onEntered,
      onClose,
      t,
    ],
  );

  const sessionsSection = useMemo(() => {
    if (!runnerKey) return null;
    if (sessionsLoading) {
      return (
        <div className="rua-acp-enter-sessions-loading">
          <Spinner />
          <span>{t("chat.acpEnter.sessionsLoading", "加载会话列表 …")}</span>
        </div>
      );
    }
    if (sessionsError) {
      return (
        <div className="rua-acp-enter-sessions-error">
          {t("chat.acpEnter.sessionsError", "无法列出会话")}: {sessionsError}
        </div>
      );
    }
    if (!sessions || sessions.length === 0) {
      return (
        <div className="rua-acp-enter-sessions-empty">
          {t(
            "chat.acpEnter.sessionsEmpty",
            "此目录下没有历史会话,可点上方 “新建会话” 开始新的对话。",
          )}
        </div>
      );
    }
    return (
      <div className="rua-acp-enter-sessions-list">
        {sessions.map((s) => (
          <div key={s.session_id} className="rua-acp-enter-session-item">
            <div className="rua-acp-enter-session-info">
              <div className="rua-acp-enter-session-title">
                {s.title?.trim() ||
                  t("chat.acpEnter.untitledSession", "(未命名会话)")}
              </div>
              <div className="rua-acp-enter-session-meta">
                <span
                  className="rua-acp-enter-session-sid"
                  title={s.session_id}
                >
                  {s.session_id.length > 12
                    ? `${s.session_id.slice(0, 12)}…`
                    : s.session_id}
                </span>
                {s.updated_at && (
                  <span className="rua-acp-enter-session-updated">
                    {formatSessionTime(s.updated_at)}
                  </span>
                )}
              </div>
            </div>
            <button
              type="button"
              className="rua-acp-enter-resume-btn"
              title={t("chat.acpEnter.resume", "Resume")}
              aria-label={t("chat.acpEnter.resume", "Resume")}
              disabled={submitting}
              onClick={() => void resumeSession(s.session_id)}
            >
              <Play size={14} />
            </button>
          </div>
        ))}
      </div>
    );
  }, [
    runnerKey,
    sessions,
    sessionsLoading,
    sessionsError,
    submitting,
    resumeSession,
    t,
  ]);

  return (
    <Modal
      open={open}
      onClose={onClose}
      className="rua-acp-enter-dialog"
      title={
        <span className="rua-acp-enter-title">
          <Zap size={16} className="rua-acp-enter-title-icon" />
          {t("chat.acpEnter.title", "进入 ACP 透传")}
        </span>
      }
      width={520}
      footer={
        <>
          <Btn variant="outline" onClick={onClose} disabled={submitting}>
            {t("common.cancel")}
          </Btn>
          <Btn
            onClick={() => void enterFresh()}
            disabled={
              !runners.some((runner) => runner.key === runnerKey) ||
              !chatId ||
              submitting
            }
          >
            {t("chat.acpEnter.enterFresh", "新建会话进入")}
          </Btn>
        </>
      }
    >
      <div className="rua-acp-enter-body">
        {runnersLoading ? (
          <div className="rua-acp-enter-loading">
            <Spinner />
            <span>
              {t("chat.acpEnter.runnersLoading", "加载 runner 列表 …")}
            </span>
          </div>
        ) : runnersError ? (
          <div className="rua-acp-enter-runners-error">
            {t("chat.acpEnter.runnersError", "无法加载 runner")}: {runnersError}
          </div>
        ) : runners.length === 0 ? (
          <div className="rua-acp-enter-runners-empty">
            {t(
              "chat.acpEnter.runnersEmpty",
              "没有启用的 ACP runner。请到 Agent → ACP 页面启用一个后再试。",
            )}
          </div>
        ) : (
          <>
            <div className="rua-acp-enter-field">
              <label className="rua-acp-enter-label">
                {t("chat.acpEnter.runnerLabel", "Runner")}
              </label>
              <Select
                className="rua-acp-enter-select"
                value={runnerKey}
                onChange={(value) => setRunnerKey(String(value || ""))}
                disabled={submitting}
                options={runnerOptions}
                popupClassName="rua-acp-enter-select-dropdown"
              />
            </div>
            <div className="rua-acp-enter-field">
              <label className="rua-acp-enter-label">
                {t("chat.acpEnter.cwdLabel", "工作目录 (cwd)")}
              </label>
              <DirectoryPicker
                value={cwd}
                onChange={setCwd}
                browse={browseDirectory}
                placeholder={
                  defaultCwd ??
                  t("chat.acpEnter.cwdPlaceholder", "留空 = 使用默认")
                }
                disabled={submitting}
                className="rua-acp-enter-cwd"
                openLabel={t("chat.acpEnter.browseCwd", "浏览工作目录")}
                closeLabel={t("chat.acpEnter.closeCwdBrowser", "关闭目录浏览")}
                emptyLabel={t("chat.acpEnter.noDirectories", "没有子目录")}
                onError={(error) =>
                  setSubmitError(
                    error instanceof Error ? error.message : String(error),
                  )
                }
              />
            </div>

            <div className="rua-acp-enter-sessions-header">
              <span>
                {t("chat.acpEnter.sessionsHeader", "该目录下的历史会话")}
              </span>
              {sessions && sessions.length > 0 && (
                <span className="rua-acp-enter-sessions-count">
                  {sessions.length}
                </span>
              )}
            </div>
            {sessionsSection}

            {submitError && (
              <div className="rua-acp-enter-submit-error">
                <Plus size={14} />
                {submitError}
              </div>
            )}
          </>
        )}
      </div>
    </Modal>
  );
}
