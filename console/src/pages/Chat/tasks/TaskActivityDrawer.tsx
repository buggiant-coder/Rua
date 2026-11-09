import { useEffect, useMemo, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import {
  Ban,
  Bot,
  CheckCircle2,
  CircleDashed,
  Clock3,
  Copy,
  CornerDownLeft,
  Eye,
  EyeOff,
  FileText,
  Monitor,
  RefreshCw,
  RotateCcw,
  Send,
  ShieldAlert,
  TriangleAlert,
  Workflow,
  Wrench,
  XCircle,
} from "lucide-react";
import { Btn, Drawer, Spinner, TextArea, Tooltip, toast } from "@/ui";
import {
  agentConversationTarget,
  type ConversationApiTarget,
} from "@/api/modules/conversation";
import { taskApi } from "@/api/modules/task";
import type {
  TaskAttempt,
  TaskRecord,
  TaskResourceRef,
  TaskSpec,
  TaskState,
} from "@/api/types/task";
import { useTaskActivity } from "./useTaskActivity";
import type { ConversationBackgroundWorkView } from "@/api/types/chat";
import { useCollaborationActivity } from "./useCollaborationActivity";
import {
  CollaborationActivityRow,
  CollaborationActivityDetail,
} from "./CollaborationActivityView";
import "./task-activity-drawer.css";

type TaskActivityDrawerProps = {
  open: boolean;
  onClose: () => void;
  agentId: string;
  target?: ConversationApiTarget;
  chatId?: string;
  currentDeviceId?: string | null;
  backgroundWork?: ConversationBackgroundWorkView | null;
};

const TERMINAL_STATES = new Set<TaskState>([
  "completed",
  "failed",
  "cancelled",
  "interrupted",
]);

const RETRYABLE_STATES = new Set<TaskState>([
  "failed",
  "cancelled",
  "interrupted",
]);

function statusMeta(state: TaskState): {
  icon: ReactNode;
  tone: string;
} {
  switch (state) {
    case "accepted":
      return {
        icon: <CircleDashed />,
        tone: "queued",
      };
    case "waiting":
      return { icon: <Clock3 />, tone: "waiting" };
    case "blocked":
      return {
        icon: <TriangleAlert />,
        tone: "blocked",
      };
    case "running":
      return { icon: <RefreshCw />, tone: "running" };
    case "waiting_approval":
      return {
        icon: <ShieldAlert />,
        tone: "approval",
      };
    case "completed":
      return {
        icon: <CheckCircle2 />,
        tone: "completed",
      };
    case "failed":
      return { icon: <XCircle />, tone: "failed" };
    case "cancelled":
      return { icon: <Ban />, tone: "cancelled" };
    case "interrupted":
      return {
        icon: <TriangleAlert />,
        tone: "interrupted",
      };
  }
}

function runnerMeta(runner: TaskSpec["runner"]): {
  icon: ReactNode;
  kindKey: string;
  name: string;
  nameKey?: string;
} {
  switch (runner.type) {
    case "tool":
      return {
        icon: <Wrench />,
        kindKey: "chat.tasks.runner.tool",
        name: runner.tool_name,
      };
    case "rua_agent":
      return {
        icon: <Bot />,
        kindKey: "chat.tasks.runner.agent",
        name: runner.agent_id,
      };
    case "sub_agent":
      return {
        icon: <Workflow />,
        kindKey: "chat.tasks.runner.subAgent",
        name: "",
        nameKey: "chat.tasks.runner.readOnly",
      };
    case "acp":
      return {
        icon: <Monitor />,
        kindKey: "chat.tasks.runner.acp",
        name: runner.runner,
      };
  }
}

function shortId(id: string): string {
  return id.length > 12 ? `${id.slice(0, 8)}...${id.slice(-4)}` : id;
}

function formatTime(value?: string | null): string {
  if (!value) return "-";
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
}

function isResourceAvailable(
  resource: TaskResourceRef,
  currentDeviceId?: string | null,
): boolean {
  return (
    !resource.source_device_id ||
    (currentDeviceId != null && resource.source_device_id === currentDeviceId)
  );
}

function StatusBadge({ state }: { state: TaskState }) {
  const { t } = useTranslation();
  const status = statusMeta(state);
  return (
    <span
      className={`rua-task-status rua-task-status--${status.tone}`}
      data-task-state={state}
    >
      <span className="rua-task-status-icon" aria-hidden="true">
        {status.icon}
      </span>
      {t(`chat.tasks.status.${state}`)}
    </span>
  );
}

function CopyButton({ value, label }: { value: string; label: string }) {
  const { t } = useTranslation();
  return (
    <Tooltip title={label}>
      <Btn
        variant="ghost"
        size="sm"
        icon={<Copy />}
        aria-label={label}
        onClick={() => {
          void navigator.clipboard.writeText(value).then(() => {
            toast.success(t("common.copied"));
          });
        }}
      />
    </Tooltip>
  );
}

function TaskExecutionDetails({ runner }: { runner: TaskSpec["runner"] }) {
  const { t } = useTranslation();
  if (runner.type !== "tool") return null;
  const { command, cwd } = runner.arguments;
  const hasCommand = typeof command === "string";
  const argumentsText = JSON.stringify(runner.arguments, null, 2);
  const argumentsContent = (
    <>
      <div className="rua-task-section-heading">
        <span>{t("chat.tasks.toolArguments")}</span>
        <CopyButton
          value={argumentsText}
          label={t("chat.tasks.copyArguments")}
        />
      </div>
      <pre
        className="rua-task-execution-code"
        aria-label={t("chat.tasks.toolArguments")}
        tabIndex={0}
      >
        <code>{argumentsText}</code>
      </pre>
    </>
  );
  return (
    <section
      className="rua-task-detail-section rua-task-execution"
      aria-label={t("chat.tasks.executionDetails")}
    >
      <h4>{t("chat.tasks.executionDetails")}</h4>
      {hasCommand && (
        <>
          <div className="rua-task-section-heading">
            <span>{t("chat.tasks.command")}</span>
            <CopyButton value={command} label={t("chat.tasks.copyCommand")} />
          </div>
          <pre
            className="rua-task-execution-code"
            aria-label={t("chat.tasks.command")}
            tabIndex={0}
          >
            <code>{command}</code>
          </pre>
        </>
      )}
      {typeof cwd === "string" && (
        <div className="rua-task-execution-cwd">
          <span className="rua-task-field-label">
            {t("chat.tasks.workingDirectory")}
          </span>
          <code>{cwd}</code>
        </div>
      )}
      {hasCommand ? (
        <details>
          <summary>{t("chat.tasks.toolArguments")}</summary>
          {argumentsContent}
        </details>
      ) : (
        argumentsContent
      )}
    </section>
  );
}

function TaskListRow({
  task,
  active,
  onSelect,
}: {
  task: TaskRecord;
  active: boolean;
  onSelect: () => void;
}) {
  const runner = runnerMeta(task.spec.runner);
  return (
    <button
      type="button"
      className={`rua-task-list-row${active ? " is-active" : ""}`}
      onClick={onSelect}
      aria-current={active ? "true" : undefined}
    >
      <span className="rua-task-list-row-head">
        <StatusBadge state={task.state} />
        <time>{formatTime(task.updated_at)}</time>
      </span>
      <span className="rua-task-list-row-title">{task.spec.instruction}</span>
      {task.spec.runner.type === "tool" &&
        typeof task.spec.runner.arguments.command === "string" && (
          <code className="rua-task-list-command">
            {task.spec.runner.arguments.command}
          </code>
        )}
      <span className="rua-task-list-row-meta">
        <span aria-hidden="true">{runner.icon}</span>
        <span>{runner.name}</span>
        <code>{shortId(task.task_id)}</code>
      </span>
    </button>
  );
}

function DependencyList({
  title,
  taskIds,
  tasks,
  onSelect,
}: {
  title: string;
  taskIds: string[];
  tasks: TaskRecord[];
  onSelect: (taskId: string) => void;
}) {
  if (taskIds.length === 0) return null;
  return (
    <div className="rua-task-dependencies">
      <span className="rua-task-field-label">{title}</span>
      <div className="rua-task-dependency-list">
        {taskIds.map((taskId) => {
          const task = tasks.find((candidate) => candidate.task_id === taskId);
          return (
            <button
              type="button"
              key={taskId}
              className="rua-task-dependency"
              onClick={() => onSelect(taskId)}
              disabled={!task}
              title={taskId}
            >
              <code>{shortId(taskId)}</code>
              {task && <StatusBadge state={task.state} />}
            </button>
          );
        })}
      </div>
    </div>
  );
}

function AttemptDetail({
  task,
  attempt,
  target,
  currentDeviceId,
}: {
  task: TaskRecord;
  attempt: TaskAttempt;
  target: ConversationApiTarget;
  currentDeviceId?: string | null;
}) {
  const { t } = useTranslation();
  const [fullResult, setFullResult] = useState<unknown>(null);
  const [fullResultError, setFullResultError] = useState<string | null>(null);
  const [fullResultLoading, setFullResultLoading] = useState(false);
  const result = attempt.result;
  const reference = result?.full_result_ref;
  const isLatest = attempt.attempt_no === task.attempt_no;
  const referenceAvailable =
    reference != null && isResourceAvailable(reference, currentDeviceId);

  useEffect(() => {
    setFullResult(null);
    setFullResultError(null);
  }, [attempt.attempt_no, task.task_id]);

  const loadFullResult = async () => {
    setFullResultLoading(true);
    setFullResultError(null);
    try {
      const address = task.spec.owner.chat_address;
      setFullResult(
        await taskApi.getFullResult(target, address.chat_id, task.task_id),
      );
    } catch (reason) {
      setFullResultError(
        reason instanceof Error ? reason.message : String(reason),
      );
    } finally {
      setFullResultLoading(false);
    }
  };

  return (
    <div className="rua-task-attempt-detail">
      <div className="rua-task-attempt-summary">
        <StatusBadge state={attempt.state} />
        <span>
          {t("chat.tasks.attemptCreated", {
            time: formatTime(attempt.created_at),
          })}
        </span>
        {attempt.finished_at && (
          <span>
            {t("chat.tasks.attemptFinished", {
              time: formatTime(attempt.finished_at),
            })}
          </span>
        )}
      </div>

      {result && (
        <>
          <section className="rua-task-detail-section">
            <h4>{t("chat.tasks.result")}</h4>
            <pre className="rua-task-result-output">{result.output}</pre>
            {result.error?.message && (
              <div className="rua-task-result-error" role="alert">
                {result.error.message}
              </div>
            )}
          </section>

          <section className="rua-task-detail-section">
            <h4>{t("chat.tasks.device")}</h4>
            <dl className="rua-task-device-grid">
              <div>
                <dt>{t("chat.tasks.deviceName")}</dt>
                <dd>{result.device.device_name || "-"}</dd>
              </div>
              <div>
                <dt>{t("chat.tasks.deviceId")}</dt>
                <dd title={result.device.device_id || undefined}>
                  {result.device.device_id
                    ? shortId(result.device.device_id)
                    : "-"}
                </dd>
              </div>
              <div>
                <dt>{t("chat.tasks.host")}</dt>
                <dd>{result.device.host_profile || "-"}</dd>
              </div>
              <div>
                <dt>{t("chat.tasks.epoch")}</dt>
                <dd>{result.device.authority_epoch ?? "-"}</dd>
              </div>
            </dl>
          </section>

          {result.artifacts.length > 0 && (
            <section className="rua-task-detail-section">
              <h4>
                {t("chat.tasks.artifacts", {
                  count: result.artifacts.length,
                })}
              </h4>
              <div className="rua-task-artifact-list">
                {result.artifacts.map((artifact) => {
                  const available = isResourceAvailable(
                    artifact.resource,
                    currentDeviceId,
                  );
                  return (
                    <div className="rua-task-artifact" key={artifact.id}>
                      <span className="rua-task-artifact-icon">
                        <FileText />
                      </span>
                      <span className="rua-task-artifact-main">
                        <strong>{artifact.name}</strong>
                        <span>{artifact.media_type}</span>
                        <code title={artifact.resource.uri}>
                          {artifact.resource.uri}
                        </code>
                      </span>
                      <span
                        className={
                          available
                            ? "rua-task-resource-state is-available"
                            : "rua-task-resource-state is-unavailable"
                        }
                      >
                        {available
                          ? t("chat.tasks.local")
                          : t("chat.tasks.remoteUnavailable")}
                      </span>
                      <CopyButton
                        value={artifact.resource.uri}
                        label={t("chat.tasks.copyReference")}
                      />
                    </div>
                  );
                })}
              </div>
            </section>
          )}

          {reference && (
            <section className="rua-task-detail-section">
              <h4>{t("chat.tasks.fullResult")}</h4>
              <div className="rua-task-full-result-ref">
                <code title={reference.uri}>{reference.uri}</code>
                <CopyButton
                  value={reference.uri}
                  label={t("chat.tasks.copyReference")}
                />
              </div>
              {!referenceAvailable && (
                <div className="rua-task-resource-unavailable" role="status">
                  <TriangleAlert />
                  <span>
                    {t("chat.tasks.remoteResultUnavailable", {
                      device: reference.source_device_id || "-",
                    })}
                  </span>
                </div>
              )}
              {referenceAvailable && isLatest && (
                <Btn
                  variant="outline"
                  size="sm"
                  loading={fullResultLoading}
                  onClick={() => void loadFullResult()}
                >
                  {t("chat.tasks.loadFullResult")}
                </Btn>
              )}
              {referenceAvailable && !isLatest && (
                <div className="rua-task-resource-note">
                  {t("chat.tasks.priorResultReference")}
                </div>
              )}
              {fullResultError && (
                <div className="rua-task-result-error" role="alert">
                  {fullResultError}
                </div>
              )}
              {fullResult != null && (
                <pre className="rua-task-full-result">
                  {typeof fullResult === "string"
                    ? fullResult
                    : JSON.stringify(fullResult, null, 2)}
                </pre>
              )}
            </section>
          )}
        </>
      )}

      {!result && (
        <div className="rua-task-no-result">{t("chat.tasks.noResult")}</div>
      )}
    </div>
  );
}

export default function TaskActivityDrawer({
  open,
  onClose,
  agentId,
  target,
  chatId,
  currentDeviceId,
  backgroundWork,
}: TaskActivityDrawerProps) {
  const { t } = useTranslation();
  const taskTarget = useMemo(
    () => target ?? agentConversationTarget(agentId),
    [agentId, target],
  );
  const {
    tasks,
    selectedTask,
    selectedTaskId,
    setSelectedTaskId,
    loading,
    busyTaskId,
    error,
    connection,
    reload,
    cancelTask,
    retryTask,
    watchTask,
    unwatchTask,
    sendTaskInput,
  } = useTaskActivity({
    open,
    target: taskTarget,
    chatId,
    authorizationAgentId: agentId,
  });
  const [attemptSelection, setAttemptSelection] = useState<{
    taskId: string;
    attemptNo: number;
  } | null>(null);
  const [taskInput, setTaskInput] = useState("");
  const collaborationActivity = useCollaborationActivity({
    open,
    target: taskTarget,
    chatId,
    backgroundWork,
  });
  const [selectedCollaborationKey, setSelectedCollaborationKey] = useState<
    string | null | undefined
  >(undefined);
  useEffect(() => setSelectedCollaborationKey(undefined), [taskTarget, chatId]);
  const selectedCollaboration =
    selectedCollaborationKey === undefined
      ? !tasks.some((task) => !TERMINAL_STATES.has(task.state))
        ? collaborationActivity.collaborations.find((item) => item.pending) ??
          (tasks.length === 0
            ? collaborationActivity.collaborations[0]
            : undefined)
        : undefined
      : collaborationActivity.collaborations.find(
          (item) => item.key === selectedCollaborationKey,
        );
  const activeCollaborations = collaborationActivity.collaborations.filter(
    (item) => item.pending,
  );
  const endedCollaborations = collaborationActivity.collaborations.filter(
    (item) => !item.pending,
  );
  const activityCount =
    tasks.length + collaborationActivity.collaborations.length;
  const [cancelConfirmTaskId, setCancelConfirmTaskId] = useState<string | null>(
    null,
  );
  useEffect(() => setTaskInput(""), [selectedTask?.task_id]);
  useEffect(() => setCancelConfirmTaskId(null), [open, selectedTask?.task_id]);
  const selectedAttemptNo =
    attemptSelection && attemptSelection.taskId === selectedTask?.task_id
      ? attemptSelection.attemptNo
      : selectedTask?.attempt_no ?? null;

  const selectedAttempt = useMemo(() => {
    const attempts = selectedTask?.attempts ?? [];
    return (
      attempts.find((attempt) => attempt.attempt_no === selectedAttemptNo) ??
      attempts[attempts.length - 1] ??
      null
    );
  }, [selectedAttemptNo, selectedTask]);
  const runner = selectedTask ? runnerMeta(selectedTask.spec.runner) : null;
  const activeTasks = tasks.filter((task) => !TERMINAL_STATES.has(task.state));
  const endedTasks = tasks.filter((task) => TERMINAL_STATES.has(task.state));
  const activeCount = activeTasks.length + activeCollaborations.length;
  const requestCancel = (taskId: string) => {
    if (cancelConfirmTaskId !== taskId) {
      setCancelConfirmTaskId(taskId);
      return;
    }
    setCancelConfirmTaskId(null);
    void cancelTask(taskId);
  };

  return (
    <Drawer
      open={open}
      onClose={onClose}
      width={760}
      className="rua-task-activity-drawer"
      title={
        <span className="rua-task-drawer-title">
          <span>{t("chat.tasks.title")}</span>
          {activeCount > 0 && (
            <span className="rua-task-active-count">{activeCount}</span>
          )}
        </span>
      }
    >
      <div className="rua-task-activity">
        <aside
          className="rua-task-list"
          aria-label={t("chat.tasks.listLabel")}
        >
          <div className="rua-task-list-toolbar">
            <span
              className={`rua-task-connection rua-task-connection--${connection}`}
            >
              <span aria-hidden="true" />
              {t(`chat.tasks.connection.${connection}`)}
            </span>
            <Tooltip title={t("common.refresh")}>
              <Btn
                variant="ghost"
                size="sm"
                icon={<RefreshCw />}
                aria-label={t("common.refresh")}
                onClick={() => {
                  void reload();
                  collaborationActivity.reload();
                }}
              />
            </Tooltip>
          </div>
          <div className="rua-task-list-scroll">
            {(loading || collaborationActivity.loading) &&
            activityCount === 0 ? (
              <div className="rua-task-list-state">
                <Spinner size="sm" />
              </div>
            ) : activityCount === 0 ? (
              <div className="rua-task-list-state">
                {t("chat.tasks.empty")}
              </div>
            ) : (
              [
                {
                  key: "active",
                  tasks: activeTasks,
                  collaborations: activeCollaborations,
                },
                {
                  key: "ended",
                  tasks: endedTasks,
                  collaborations: endedCollaborations,
                },
              ].map((group) => (
                <section
                  key={group.key}
                  aria-label={t(`chat.tasks.groups.${group.key}`)}
                >
                  <h3 className="rua-task-list-group-heading">
                    <span>{t(`chat.tasks.groups.${group.key}`)}</span>
                    <span className="rua-task-list-group-count">
                      {group.tasks.length + group.collaborations.length}
                    </span>
                  </h3>
                  {group.tasks.length + group.collaborations.length === 0 ? (
                    <div className="rua-task-list-group-empty">
                      {t(`chat.tasks.groups.${group.key}Empty`)}
                    </div>
                  ) : (
                    group.tasks.map((task) => (
                      <TaskListRow
                        key={task.task_id}
                        task={task}
                        active={
                          !selectedCollaboration &&
                          task.task_id === selectedTaskId
                        }
                        onSelect={() => {
                          setSelectedCollaborationKey(null);
                          setSelectedTaskId(task.task_id);
                        }}
                      />
                    ))
                  )}
                  {group.collaborations.map((item) => (
                    <CollaborationActivityRow
                      key={item.key}
                      item={item}
                      active={selectedCollaboration?.key === item.key}
                      onSelect={() => setSelectedCollaborationKey(item.key)}
                    />
                  ))}
                </section>
              ))
            )}
          </div>
        </aside>

        <main className="rua-task-detail">
          {collaborationActivity.error && (
            <div className="rua-task-drawer-error" role="alert">
              {t("chat.tasks.collaboration.loadError")}:{" "}
              {collaborationActivity.error}
            </div>
          )}
          {error && (
            <div className="rua-task-drawer-error" role="alert">
              {error}
            </div>
          )}
          {selectedCollaboration ? (
            <CollaborationActivityDetail
              item={selectedCollaboration}
              onOpen={onClose}
            />
          ) : !selectedTask || !runner ? (
            <div className="rua-task-detail-empty">
              <Workflow />
              <span>{t("chat.tasks.select")}</span>
            </div>
          ) : (
            <>
              <header className="rua-task-detail-header">
                <div className="rua-task-detail-identity">
                  <span className="rua-task-runner-icon" aria-hidden="true">
                    {runner.icon}
                  </span>
                  <div>
                    <span className="rua-task-runner-kind">
                      {t(runner.kindKey)} ·{" "}
                      {runner.nameKey ? t(runner.nameKey) : runner.name}
                    </span>
                    <span className="rua-task-detail-id">
                      <code title={selectedTask.task_id}>
                        {shortId(selectedTask.task_id)}
                      </code>
                      <CopyButton
                        value={selectedTask.task_id}
                        label={t("chat.tasks.copyId")}
                      />
                    </span>
                  </div>
                </div>
                <StatusBadge state={selectedTask.state} />
              </header>

              <div className="rua-task-detail-scroll">
                <section className="rua-task-instruction">
                  {selectedTask.spec.instruction}
                </section>

                <TaskExecutionDetails
                  key={selectedTask.task_id}
                  runner={selectedTask.spec.runner}
                />

                <DependencyList
                  title={t("chat.tasks.waitingOn")}
                  taskIds={selectedTask.waiting_on}
                  tasks={tasks}
                  onSelect={setSelectedTaskId}
                />
                <DependencyList
                  title={t("chat.tasks.blockedBy")}
                  taskIds={selectedTask.blocked_by}
                  tasks={tasks}
                  onSelect={setSelectedTaskId}
                />

                {selectedTask.input_capability !== "none" &&
                  !TERMINAL_STATES.has(selectedTask.state) && (
                    <section className="rua-task-detail-section rua-task-input">
                      <h4>
                        {selectedTask.input_capability === "stdin"
                          ? t("chat.tasks.stdin")
                          : t("chat.tasks.steering")}
                      </h4>
                      <TextArea
                        value={taskInput}
                        rows={3}
                        onChange={(event) => setTaskInput(event.target.value)}
                      />
                      <div className="rua-task-input-actions">
                        {selectedTask.input_capability === "stdin" && (
                          <Btn
                            variant="outline"
                            size="sm"
                            icon={<CornerDownLeft />}
                            onClick={() =>
                              setTaskInput((value) => `${value}\n`)
                            }
                          >
                            {t("chat.tasks.appendNewline")}
                          </Btn>
                        )}
                        <Btn
                          variant="filled"
                          size="sm"
                          icon={<Send />}
                          disabled={taskInput.length === 0}
                          loading={busyTaskId === selectedTask.task_id}
                          onClick={() => {
                            void sendTaskInput(selectedTask.task_id, {
                              type: "text",
                              text: taskInput,
                            }).then(() => setTaskInput(""));
                          }}
                        >
                          {t("common.send")}
                        </Btn>
                        {selectedTask.input_capability === "stdin" && (
                          <Btn
                            variant="outline"
                            size="sm"
                            loading={busyTaskId === selectedTask.task_id}
                            onClick={() =>
                              void sendTaskInput(selectedTask.task_id, {
                                type: "close_stdin",
                              })
                            }
                          >
                            {t("chat.tasks.closeStdin")}
                          </Btn>
                        )}
                      </div>
                    </section>
                  )}

                <section className="rua-task-attempts">
                  <div className="rua-task-attempts-heading">
                    <h4>{t("chat.tasks.attempts")}</h4>
                    <span>{selectedTask.attempts.length}</span>
                  </div>
                  <div
                    className="rua-task-attempt-switcher"
                    role="tablist"
                    aria-label={t("chat.tasks.attempts")}
                  >
                    {selectedTask.attempts.map((attempt) => (
                      <button
                        type="button"
                        role="tab"
                        aria-selected={
                          attempt.attempt_no === selectedAttempt?.attempt_no
                        }
                        className={
                          attempt.attempt_no === selectedAttempt?.attempt_no
                            ? "is-selected"
                            : ""
                        }
                        key={attempt.attempt_no}
                        onClick={() =>
                          setAttemptSelection({
                            taskId: selectedTask.task_id,
                            attemptNo: attempt.attempt_no,
                          })
                        }
                      >
                        <span>#{attempt.attempt_no}</span>
                        <StatusBadge state={attempt.state} />
                      </button>
                    ))}
                  </div>
                  {selectedAttempt && (
                    <AttemptDetail
                      task={selectedTask}
                      attempt={selectedAttempt}
                      target={taskTarget}
                      currentDeviceId={currentDeviceId}
                    />
                  )}
                </section>
              </div>

              <footer className="rua-task-detail-actions">
                {!TERMINAL_STATES.has(selectedTask.state) &&
                  (selectedTask.watch_active ? (
                    <Btn
                      variant="outline"
                      size="sm"
                      icon={<EyeOff />}
                      loading={busyTaskId === selectedTask.task_id}
                      onClick={() => void unwatchTask(selectedTask.task_id)}
                    >
                      {t("chat.tasks.unwatch")}
                    </Btn>
                  ) : (
                    <Btn
                      variant="outline"
                      size="sm"
                      icon={<Eye />}
                      loading={busyTaskId === selectedTask.task_id}
                      onClick={() => void watchTask(selectedTask.task_id)}
                    >
                      {t("chat.tasks.watch")}
                    </Btn>
                  ))}
                {!TERMINAL_STATES.has(selectedTask.state) && (
                  <div className="rua-task-cancel-controls">
                    {cancelConfirmTaskId === selectedTask.task_id && (
                      <Btn
                        variant="outline"
                        size="sm"
                        onClick={() => setCancelConfirmTaskId(null)}
                      >
                        {t("chat.tasks.keepTask")}
                      </Btn>
                    )}
                    <Btn
                      variant="danger"
                      size="sm"
                      icon={<Ban />}
                      loading={busyTaskId === selectedTask.task_id}
                      onClick={() => requestCancel(selectedTask.task_id)}
                    >
                      {cancelConfirmTaskId === selectedTask.task_id
                        ? t("chat.tasks.confirmCancel")
                        : t("chat.tasks.cancelTask")}
                    </Btn>
                  </div>
                )}
                {RETRYABLE_STATES.has(selectedTask.state) && (
                  <Btn
                    variant="outline"
                    size="sm"
                    icon={<RotateCcw />}
                    loading={busyTaskId === selectedTask.task_id}
                    onClick={() => void retryTask(selectedTask.task_id)}
                  >
                    {t("common.retry")}
                  </Btn>
                )}
              </footer>
            </>
          )}
        </main>
      </div>
    </Drawer>
  );
}
