import { useEffect } from "react";
import { toast } from "@/ui";
import { conflictsApi, syncApi } from "../api/modules/sync";
import { MULTI_DEVICE_FEATURES_CHANGED_EVENT } from "./useMultiDeviceFeatureAvailability";

const STORAGE_KEY = "sync_conflict_last_seen_id";
const POLL_INTERVAL_MS = 30_000;

export function useSyncConflictToast() {
  useEffect(() => {
    let cancelled = false;
    let enabled: boolean | undefined;
    let running = false;
    let settingsVersion = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;

    const poll = async () => {
      if (cancelled || document.hidden || running) return;
      running = true;
      const version = settingsVersion;
      try {
        if (enabled === undefined) {
          const status = await syncApi.getStatus();
          if (cancelled || version !== settingsVersion) return;
          enabled = status.enabled;
        }
        if (cancelled || document.hidden || !enabled) return;
        const { conflicts } = await conflictsApi.getRecent();
        if (
          cancelled ||
          document.hidden ||
          version !== settingsVersion ||
          conflicts.length === 0
        )
          return;

        const lastSeenId = parseInt(
          localStorage.getItem(STORAGE_KEY) || "-1",
          10,
        );
        const newConflicts = conflicts.filter((c) => c.id > lastSeenId);
        if (newConflicts.length === 0) return;

        const maxId = Math.max(...newConflicts.map((c) => c.id));
        localStorage.setItem(STORAGE_KEY, String(maxId));
        toast.warning(
          `${newConflicts.length} 条改动因冲突被合并 — 点击查看详情`,
        );
      } catch {
        // A failed refresh must not block future visible checks.
      } finally {
        running = false;
        if (!cancelled && !document.hidden && enabled !== false) {
          timer = setTimeout(
            () => void poll(),
            version === settingsVersion ? POLL_INTERVAL_MS : 0,
          );
        }
      }
    };
    const handleVisibility = () => {
      clearTimeout(timer);
      timer = undefined;
      if (!document.hidden) void poll();
    };

    const handleSettingsChange = () => {
      enabled = undefined;
      settingsVersion += 1;
      handleVisibility();
    };
    window.addEventListener(
      MULTI_DEVICE_FEATURES_CHANGED_EVENT,
      handleSettingsChange,
    );
    document.addEventListener("visibilitychange", handleVisibility);
    void poll();
    return () => {
      cancelled = true;
      clearTimeout(timer);
      document.removeEventListener("visibilitychange", handleVisibility);
      window.removeEventListener(
        MULTI_DEVICE_FEATURES_CHANGED_EVENT,
        handleSettingsChange,
      );
    };
  }, []);
}
