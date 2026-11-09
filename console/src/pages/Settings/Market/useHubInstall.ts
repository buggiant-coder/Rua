import { useCallback, useEffect, useRef, useState } from "react";
import { marketApi } from "../../../api/modules/market";
import type { MarketResult } from "../../../api/modules/market";

export type InstallTarget = "pool" | "workspace";

export type InstallStatus =
  | "queued"
  | "installing"
  | "completed"
  | "failed"
  | "cancelled";

export interface InstallQueueItem {
  id: string;
  result: MarketResult;
  target: InstallTarget;
  status: InstallStatus;
  message: string;
  installedName?: string;
}

export interface UseHubInstallOptions {
  selectedAgent: string;
  onSuccess?: (item: InstallQueueItem) => void;
  onError?: (item: InstallQueueItem, err: unknown) => void;
}

const POLL_MS = 1000;
const TIMEOUT_MS = 90_000;

export function marketMeta(result: MarketResult): Record<string, unknown> {
  return {
    source: result.source,
    slug: result.slug,
    name: result.name,
    author: result.author,
    icon_url: result.icon_url,
    stats: result.stats,
  };
}

export function useHubInstall(opts: UseHubInstallOptions) {
  const [queue, setQueueState] = useState<InstallQueueItem[]>([]);
  const queueRef = useRef<InstallQueueItem[]>([]);
  const runningRef = useRef(false);
  const cancelledRef = useRef<Set<string>>(new Set());
  const currentTaskIdRef = useRef<{ taskId: string; agentId: string } | null>(
    null,
  );
  const currentInstallingItemIdRef = useRef<string | null>(null);
  const selectedAgentRef = useRef(opts.selectedAgent);
  useEffect(() => {
    selectedAgentRef.current = opts.selectedAgent;
  }, [opts.selectedAgent]);

  const setQueue = useCallback((next: InstallQueueItem[]) => {
    queueRef.current = next;
    setQueueState(next);
  }, []);

  const updateItem = useCallback(
    (id: string, patch: Partial<InstallQueueItem>) => {
      const next = queueRef.current.map((it) =>
        it.id === id ? { ...it, ...patch } : it,
      );
      setQueue(next);
    },
    [setQueue],
  );

  const installWorkspace = useCallback(
    async (item: InstallQueueItem, overrideName: string | undefined) => {
      const agentId = selectedAgentRef.current;
      if (!agentId)
        throw new Error("Select an Agent before installing a workspace skill");
      const task = await marketApi.startHubInstall(
        {
          bundle_url: item.result.source_url,
          version: item.result.version || undefined,
          enable: true,
          target_name: overrideName,
          installed_from: item.result.source,
          market_meta: marketMeta(item.result),
        },
        agentId,
      );
      currentTaskIdRef.current = { taskId: task.task_id, agentId };
      currentInstallingItemIdRef.current = item.id;
      const startedAt = Date.now();
      try {
        while (currentTaskIdRef.current?.taskId === task.task_id) {
          if (cancelledRef.current.has(item.id)) {
            await marketApi.cancelHubInstall(task.task_id, agentId);
            updateItem(item.id, { status: "cancelled", message: "" });
            return;
          }
          const status = await marketApi.getHubInstallStatus(
            task.task_id,
            agentId,
          );
          if (status.status === "completed" && status.result?.installed) {
            const installedName = String(status.result.name || "");
            updateItem(item.id, {
              status: "completed",
              installedName,
              message: installedName,
            });
            opts.onSuccess?.({ ...item, status: "completed" });
            return;
          }
          if (status.status === "failed") {
            throw new Error(status.error || "");
          }
          if (status.status === "cancelled") {
            updateItem(item.id, { status: "cancelled", message: "" });
            return;
          }
          if (Date.now() - startedAt > TIMEOUT_MS) {
            await marketApi.cancelHubInstall(task.task_id, agentId);
            updateItem(item.id, {
              status: "failed",
              message: "__TIMED_OUT__",
            });
            return;
          }
          await new Promise((res) => window.setTimeout(res, POLL_MS));
        }
      } finally {
        if (currentTaskIdRef.current?.taskId === task.task_id) {
          currentTaskIdRef.current = null;
        }
        if (currentInstallingItemIdRef.current === item.id) {
          currentInstallingItemIdRef.current = null;
        }
      }
    },
    [opts, updateItem],
  );

  const installOne = useCallback(
    async (item: InstallQueueItem, overrideName: string | undefined) => {
      updateItem(item.id, { status: "installing", message: "" });
      try {
        if (item.target === "pool") {
          currentInstallingItemIdRef.current = item.id;
          try {
            const result = await marketApi.importPoolFromHub({
              bundle_url: item.result.source_url,
              version: item.result.version || undefined,
              target_name: overrideName,
              installed_from: item.result.source,
              market_meta: marketMeta(item.result),
            });
            if (cancelledRef.current.has(item.id)) {
              updateItem(item.id, { status: "cancelled", message: "" });
              return;
            }
            updateItem(item.id, {
              status: "completed",
              installedName: result.name,
              message: result.name,
            });
            opts.onSuccess?.({ ...item, status: "completed" });
          } finally {
            if (currentInstallingItemIdRef.current === item.id) {
              currentInstallingItemIdRef.current = null;
            }
          }
        } else {
          await installWorkspace(item, overrideName);
        }
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err);
        updateItem(item.id, { status: "failed", message: msg });
        opts.onError?.({ ...item, status: "failed" }, err);
      }
    },
    [installWorkspace, opts, updateItem],
  );

  const runQueue = useCallback(async () => {
    if (runningRef.current) return;
    runningRef.current = true;
    try {
      while (true) {
        const next = queueRef.current.find((it) => it.status === "queued");
        if (!next) break;
        if (cancelledRef.current.has(next.id)) {
          updateItem(next.id, { status: "cancelled", message: "" });
          continue;
        }
        await installOne(next, undefined);
      }
    } finally {
      runningRef.current = false;
    }
  }, [installOne, updateItem]);

  const enqueue = useCallback(
    (results: MarketResult[], target: InstallTarget) => {
      const items: InstallQueueItem[] = results.map((r) => ({
        id: `${r.source}:${r.slug}:${Date.now()}:${Math.random()
          .toString(36)
          .slice(2, 7)}`,
        result: r,
        target,
        status: "queued",
        message: "",
      }));
      setQueue([...queueRef.current, ...items]);
      void runQueue();
      return items;
    },
    [runQueue, setQueue],
  );

  const cancel = useCallback(
    (id: string) => {
      cancelledRef.current.add(id);
      if (id !== currentInstallingItemIdRef.current) {
        updateItem(id, { status: "cancelled", message: "" });
        return;
      }
      const taskId = currentTaskIdRef.current;
      if (taskId) {
        void marketApi.cancelHubInstall(taskId.taskId, taskId.agentId);
      }
    },
    [updateItem],
  );

  const retry = useCallback(
    (id: string) => {
      if (!queueRef.current.some((it) => it.id === id)) return;
      cancelledRef.current.delete(id);
      updateItem(id, { status: "queued", message: "" });
      void runQueue();
    },
    [runQueue, updateItem],
  );

  const clearCompleted = useCallback(() => {
    setQueue(queueRef.current.filter((it) => it.status !== "completed"));
  }, [setQueue]);

  return { queue, enqueue, cancel, retry, clearCompleted };
}
