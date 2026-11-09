import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { buildAuthHeaders } from "@/api/authHeaders";
import { getApiUrl } from "@/api/config";
import type { ConversationApiTarget } from "@/api/modules/conversation";
import { taskApi } from "@/api/modules/task";
import type { TaskEvent, TaskRecord } from "@/api/types/task";
import { readSseChunks } from "../owned/sse";

export type TaskActivityConnection =
  | "idle"
  | "connecting"
  | "connected"
  | "reconnecting";

type UseTaskActivityOptions = {
  open: boolean;
  target: ConversationApiTarget;
  chatId?: string;
  authorizationAgentId: string;
};

function sortTasks(tasks: TaskRecord[]): TaskRecord[] {
  return [...tasks].sort((left, right) =>
    right.created_at.localeCompare(left.created_at),
  );
}

function upsertTask(tasks: TaskRecord[], next: TaskRecord): TaskRecord[] {
  const index = tasks.findIndex((task) => task.task_id === next.task_id);
  if (index < 0) return sortTasks([...tasks, next]);
  const updated = [...tasks];
  updated[index] = next;
  return tasks[index].created_at === next.created_at
    ? updated
    : sortTasks(updated);
}

export function useTaskActivity({
  open,
  target,
  chatId,
  authorizationAgentId,
}: UseTaskActivityOptions) {
  const targetScope = target.scope;
  const targetAgentId = target.scope === "agent" ? target.agentId : null;
  const targetProjectId = target.scope === "project" ? target.projectId : null;
  const taskTarget = useMemo<ConversationApiTarget>(
    () =>
      targetScope === "project"
        ? { scope: "project", projectId: targetProjectId! }
        : { scope: "agent", agentId: targetAgentId! },
    [targetAgentId, targetProjectId, targetScope],
  );
  const [tasks, setTasks] = useState<TaskRecord[]>([]);
  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [busyTaskId, setBusyTaskId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [connection, setConnection] = useState<TaskActivityConnection>("idle");
  const [viewerVisible, setViewerVisible] = useState(
    () => typeof document === "undefined" || !document.hidden,
  );
  const scopeRef = useRef("");
  const scope =
    taskTarget.scope === "project"
      ? `project:${taskTarget.projectId}:${chatId ?? ""}`
      : `agent:${taskTarget.agentId}:${chatId ?? ""}`;
  scopeRef.current = scope;

  const [taskReads] = useState(
    () => new Map<string, { dirty: boolean; promise: Promise<void> }>(),
  );
  useEffect(() => {
    taskReads.clear();
    return () => taskReads.clear();
  }, [scope, taskReads]);

  const replaceTasks = useCallback((records: TaskRecord[]) => {
    const sorted = sortTasks(records);
    setTasks(sorted);
    setSelectedTaskId((current) => {
      if (current && records.some((task) => task.task_id === current)) {
        return current;
      }
      return sorted[0]?.task_id ?? null;
    });
  }, []);

  const reload = useCallback(async () => {
    if (!chatId) {
      replaceTasks([]);
      return;
    }
    const requestScope = scope;
    setLoading(true);
    setError(null);
    try {
      const records = await taskApi.listTasks(taskTarget, chatId, {
        limit: 200,
      });
      if (scopeRef.current === requestScope) replaceTasks(records);
    } catch (reason) {
      if (scopeRef.current === requestScope) {
        setError(reason instanceof Error ? reason.message : String(reason));
      }
    } finally {
      if (scopeRef.current === requestScope) setLoading(false);
    }
  }, [chatId, replaceTasks, scope, taskTarget]);

  const refreshTask = useCallback(
    (taskId: string): Promise<void> => {
      if (!chatId || scopeRef.current !== scope) return Promise.resolve();
      const existing = taskReads.get(taskId);
      if (existing) {
        existing.dirty = true;
        return existing.promise;
      }
      const pending = { dirty: false, promise: Promise.resolve() };
      taskReads.set(taskId, pending);
      const isCurrent = () =>
        scopeRef.current === scope && taskReads.get(taskId) === pending;
      pending.promise = (async () => {
        try {
          do {
            pending.dirty = false;
            try {
              const record = await taskApi.getTask(taskTarget, chatId, taskId);
              if (isCurrent()) {
                setTasks((current) => upsertTask(current, record));
              }
            } catch {
              if (isCurrent()) void reload();
            }
          } while (pending.dirty && isCurrent());
        } finally {
          if (taskReads.get(taskId) === pending) taskReads.delete(taskId);
        }
      })();
      return pending.promise;
    },
    [chatId, reload, scope, taskReads, taskTarget],
  );

  useEffect(() => {
    setTasks([]);
    setSelectedTaskId(null);
    setError(null);
  }, [scope]);

  useEffect(() => {
    const handleVisibility = () => setViewerVisible(!document.hidden);
    document.addEventListener("visibilitychange", handleVisibility);
    return () => {
      document.removeEventListener("visibilitychange", handleVisibility);
    };
  }, []);

  useEffect(() => {
    if (!open || !chatId || !viewerVisible) {
      setConnection("idle");
      return;
    }
    let stopped = false;
    let attempt = 0;
    let controller: AbortController | null = null;
    let reconnectTimer: number | null = null;
    const requestScope = scope;

    const connect = async () => {
      if (stopped) return;
      controller = new AbortController();
      setConnection(attempt === 0 ? "connecting" : "reconnecting");
      try {
        const response = await fetch(
          getApiUrl(taskApi.activityPath(taskTarget, chatId)),
          {
            method: "GET",
            headers: buildAuthHeaders(),
            signal: controller.signal,
          },
        );
        if (!response.ok || !response.body) {
          throw new Error(`Task activity unavailable (${response.status})`);
        }
        attempt = 0;
        setConnection("connected");
        for await (const chunk of readSseChunks(response.body)) {
          if (stopped || scopeRef.current !== requestScope) return;
          const payload = JSON.parse(chunk.data) as
            | { type?: "snapshot"; tasks?: TaskRecord[] }
            | TaskEvent;
          if (
            chunk.event === "snapshot" ||
            ("type" in payload && payload.type === "snapshot")
          ) {
            replaceTasks(
              "tasks" in payload && Array.isArray(payload.tasks)
                ? payload.tasks
                : [],
            );
          } else if (chunk.event === "resync_required") {
            void reload();
          } else if ("task_id" in payload && payload.task_id) {
            void refreshTask(payload.task_id);
          }
        }
      } catch (reason) {
        if (
          stopped ||
          (reason instanceof DOMException && reason.name === "AbortError")
        ) {
          return;
        }
      }
      if (!stopped) {
        attempt += 1;
        setConnection("reconnecting");
        const delay = Math.min(5_000, 750 * 2 ** Math.min(attempt - 1, 3));
        reconnectTimer = window.setTimeout(() => void connect(), delay);
      }
    };

    void reload();
    void connect();
    return () => {
      stopped = true;
      taskReads.clear();
      controller?.abort();
      if (reconnectTimer != null) window.clearTimeout(reconnectTimer);
    };
  }, [
    chatId,
    open,
    refreshTask,
    reload,
    replaceTasks,
    scope,
    taskTarget,
    taskReads,
    viewerVisible,
  ]);

  const runControl = useCallback(
    async (taskId: string, action: "cancel" | "retry") => {
      if (!chatId) return;
      setBusyTaskId(taskId);
      setError(null);
      try {
        const record =
          action === "cancel"
            ? await taskApi.cancelTask(taskTarget, chatId, taskId)
            : await taskApi.retryTask(
                taskTarget,
                chatId,
                taskId,
                authorizationAgentId,
              );
        setTasks((current) => upsertTask(current, record));
        setSelectedTaskId(taskId);
      } catch (reason) {
        setError(reason instanceof Error ? reason.message : String(reason));
      } finally {
        setBusyTaskId(null);
      }
    },
    [authorizationAgentId, chatId, taskTarget],
  );

  const setWatch = useCallback(
    async (taskId: string, watch: boolean) => {
      if (!chatId) return;
      setBusyTaskId(taskId);
      setError(null);
      try {
        if (watch) {
          await taskApi.watchTask(taskTarget, chatId, taskId);
        } else {
          await taskApi.unwatchTask(taskTarget, chatId, taskId);
        }
        await refreshTask(taskId);
      } catch (reason) {
        setError(reason instanceof Error ? reason.message : String(reason));
      } finally {
        setBusyTaskId(null);
      }
    },
    [chatId, refreshTask, taskTarget],
  );

  const sendInput = useCallback(
    async (
      taskId: string,
      input: { type: "text"; text: string } | { type: "close_stdin" },
    ) => {
      if (!chatId) return;
      setBusyTaskId(taskId);
      setError(null);
      try {
        await taskApi.sendInput(taskTarget, chatId, taskId, input);
        await refreshTask(taskId);
      } catch (reason) {
        setError(reason instanceof Error ? reason.message : String(reason));
      } finally {
        setBusyTaskId(null);
      }
    },
    [chatId, refreshTask, taskTarget],
  );

  const selectedTask = useMemo(
    () => tasks.find((task) => task.task_id === selectedTaskId) ?? null,
    [selectedTaskId, tasks],
  );

  return {
    tasks,
    selectedTask,
    selectedTaskId,
    setSelectedTaskId,
    loading,
    busyTaskId,
    error,
    connection,
    reload,
    cancelTask: (taskId: string) => runControl(taskId, "cancel"),
    retryTask: (taskId: string) => runControl(taskId, "retry"),
    watchTask: (taskId: string) => setWatch(taskId, true),
    unwatchTask: (taskId: string) => setWatch(taskId, false),
    sendTaskInput: sendInput,
  };
}

export { sortTasks, upsertTask };
