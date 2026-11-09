import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Lock, Pencil, Plus, Square, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { DeviceDto } from "../../api/modules/sync";
import {
  terminalApi,
  type TerminalPalette,
  type TerminalExecutionDevice,
  type TerminalProfileListResponse,
  type TerminalSession,
} from "../../api/modules/terminal";
import { getBrowserViewerId } from "../../deviceNetwork/viewerIdentity";
import { Page, PageBody } from "@/components/Page";
import { PageHeader } from "@/components/PageHeader";
import {
  DeviceConnectivityTag,
  deviceConnectivityLabel,
} from "@/components/DeviceConnectivity";
import { Btn, Check, Modal, NumberInput, Select } from "@/ui";
import TerminalViewport from "./TerminalViewport";
import type {
  TerminalStreamCursor,
  TerminalStreamFrame,
} from "../../terminal/stream";
import { recordTerminalDiagnostic } from "../../terminal/diagnostics";
import { terminalRuntimeRegistry } from "../../terminal/runtime/registry";
import { useMultiDeviceFeatureAvailability } from "../../hooks/useMultiDeviceFeatureAvailability";
import "./terminal.css";

function deviceLabel(device: DeviceDto): string {
  return (
    device.friendly_name?.trim() ||
    device.label?.trim() ||
    device.device_id.slice(0, 8)
  );
}

function executionDeviceKey(device: TerminalExecutionDevice): string {
  return device.kind === "local" ? "local" : `remote:${device.device_id}`;
}

function executionDeviceFromKey(key: string): TerminalExecutionDevice {
  if (key === "local") return { kind: "local" };
  return { kind: "remote", device_id: key.replace(/^remote:/, "") };
}

function activeExecutionDeviceStorageKey(storageScope: string) {
  return `${ACTIVE_EXECUTION_DEVICE_KEY_PREFIX}${storageScope}`;
}

function loadActiveExecutionDevice(
  storageScope: string | null,
): TerminalExecutionDevice {
  if (!storageScope) return { kind: "local" };
  const stored = window.localStorage.getItem(
    activeExecutionDeviceStorageKey(storageScope),
  );
  if (!stored || stored === "local") return { kind: "local" };
  const deviceId = stored.replace(/^remote:/, "").trim();
  return deviceId ? { kind: "remote", device_id: deviceId } : { kind: "local" };
}

function saveActiveExecutionDevice(
  storageScope: string | null,
  device: TerminalExecutionDevice,
) {
  if (!storageScope) return;
  window.localStorage.setItem(
    activeExecutionDeviceStorageKey(storageScope),
    executionDeviceKey(device),
  );
}

function executionDeviceQuery(device: TerminalExecutionDevice): string {
  return device.kind === "local" ? "local" : device.device_id;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function terminalSessionIsGone(error: unknown) {
  const message = errorMessage(error).toLowerCase();
  return message.includes("not found") || message.includes("404");
}

function cwdName(cwd: string): string {
  const normalized = cwd.replace(/\/+$/, "");
  return normalized.split("/").filter(Boolean).pop() || normalized || "/";
}

function sessionShortId(sessionId: string): string {
  return sessionId.replace(/^terminal-session-/, "").slice(0, 8);
}

const SKIP_KILL_CONFIRM_KEY = "rua-terminal-skip-kill-confirm";
const SESSION_NAMES_KEY = "rua-terminal-session-names";
const DEFAULT_PROFILE_KEY_PREFIX = "rua-terminal-default-profile:";
const ACTIVE_SESSION_KEY_PREFIX = "rua-terminal-active-session:";
const ACTIVE_EXECUTION_DEVICE_KEY_PREFIX =
  "rua-terminal-active-execution-device:";
const SCROLLBACK_LINES_KEY = "rua-terminal-scrollback-lines";
const ASK_PROFILE_ID = "ask";
const DEFAULT_SCROLLBACK_LINES = 5000;
const MIN_SCROLLBACK_LINES = 100;
const MAX_SCROLLBACK_LINES = 100_000;

function loadSessionNames(): Record<string, string> {
  try {
    const raw = window.localStorage.getItem(SESSION_NAMES_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw);
    return parsed && typeof parsed === "object" ? parsed : {};
  } catch {
    return {};
  }
}

function saveSessionNames(names: Record<string, string>) {
  window.localStorage.setItem(SESSION_NAMES_KEY, JSON.stringify(names));
}

function clampNumber(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
}

function loadStoredNumber(
  key: string,
  fallback: number,
  min: number,
  max: number,
) {
  const raw = window.localStorage.getItem(key);
  if (!raw) return fallback;
  const parsed = Number(raw);
  return Number.isFinite(parsed)
    ? clampNumber(Math.round(parsed), min, max)
    : fallback;
}

function defaultProfileStorageKey(executionDevice: TerminalExecutionDevice) {
  return `${DEFAULT_PROFILE_KEY_PREFIX}${executionDeviceKey(executionDevice)}`;
}

function activeSessionStorageKey(
  storageScope: string,
  executionDevice: TerminalExecutionDevice,
) {
  return `${ACTIVE_SESSION_KEY_PREFIX}${storageScope}:${executionDeviceKey(
    executionDevice,
  )}`;
}

function loadActiveSessionId(
  storageScope: string | null,
  executionDevice: TerminalExecutionDevice,
  sessions: TerminalSession[],
) {
  const stored = storageScope
    ? window.localStorage.getItem(
        activeSessionStorageKey(storageScope, executionDevice),
      )
    : null;
  return sessions.some((session) => session.session_id === stored)
    ? stored
    : sessions[0]?.session_id ?? null;
}

function saveActiveSessionId(
  storageScope: string | null,
  executionDevice: TerminalExecutionDevice,
  sessionId: string | null,
) {
  if (!storageScope) return;
  const key = activeSessionStorageKey(storageScope, executionDevice);
  if (sessionId) {
    window.localStorage.setItem(key, sessionId);
  } else {
    window.localStorage.removeItem(key);
  }
}

function terminalOutputCacheKey(
  executionDevice: TerminalExecutionDevice,
  sessionId: string,
  browserViewerId: string,
) {
  return `${executionDeviceKey(
    executionDevice,
  )}:${sessionId}:${browserViewerId}`;
}

function nextSessionIdAfterRemoval(
  sessions: TerminalSession[],
  removedSessionId: string,
) {
  const removedIndex = sessions.findIndex(
    (session) => session.session_id === removedSessionId,
  );
  const remaining = sessions.filter(
    (session) => session.session_id !== removedSessionId,
  );
  if (remaining.length === 0) return null;
  return remaining[Math.min(Math.max(removedIndex, 0), remaining.length - 1)]
    .session_id;
}

interface TerminalPageProps {
  routeActive?: boolean;
}

interface PendingTerminalGeometryRequest {
  executionDevice: TerminalExecutionDevice;
  geometry: { cols: number; rows: number };
  key: string;
  sessionId: string;
}

export default function TerminalPage({
  routeActive = true,
}: TerminalPageProps) {
  const { t } = useTranslation();
  const browserViewerId = useMemo(getBrowserViewerId, []);
  const {
    devices,
    loaded: devicesLoaded,
    available: multiDeviceFeaturesAvailable,
  } = useMultiDeviceFeatureAvailability(routeActive);
  const [executionDevice, setExecutionDevice] =
    useState<TerminalExecutionDevice>({ kind: "local" });
  const [sessions, setSessions] = useState<TerminalSession[]>([]);
  const [terminalProfiles, setTerminalProfiles] =
    useState<TerminalProfileListResponse | null>(null);
  const [defaultProfileId, setDefaultProfileId] = useState<string | null>(null);
  const [terminalOptionsOpen, setTerminalOptionsOpen] = useState(false);
  const [createProfileOpen, setCreateProfileOpen] = useState(false);
  const [createProfileId, setCreateProfileId] = useState("");
  const [rememberCreateProfileDraft, setRememberCreateProfileDraft] =
    useState(false);
  const [activeSessionId, setActiveSessionId] = useState<string | null>(null);
  const [terminalError, setTerminalError] = useState<string | null>(null);
  const [terminalStatus, setTerminalStatus] = useState<string | null>(null);
  const [killConfirmOpen, setKillConfirmOpen] = useState(false);
  const [skipKillConfirmDraft, setSkipKillConfirmDraft] = useState(false);
  const [killConfirmEnabled, setKillConfirmEnabled] = useState(
    () => window.localStorage.getItem(SKIP_KILL_CONFIRM_KEY) !== "true",
  );
  const [killLoading, setKillLoading] = useState(false);
  const [sessionNames, setSessionNames] = useState(loadSessionNames);
  const [scrollbackLines, setScrollbackLines] = useState(() =>
    loadStoredNumber(
      SCROLLBACK_LINES_KEY,
      DEFAULT_SCROLLBACK_LINES,
      MIN_SCROLLBACK_LINES,
      MAX_SCROLLBACK_LINES,
    ),
  );
  const [renameSessionId, setRenameSessionId] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
  const [mountedTerminalCacheKeys, setMountedTerminalCacheKeys] = useState<
    string[]
  >([]);
  const [viewportGeometry, setViewportGeometry] = useState<{
    cols: number;
    rows: number;
    sessionId: string;
  } | null>(null);
  const geometrySyncRef = useRef<{
    inFlightKey: string | null;
    pending: PendingTerminalGeometryRequest | null;
  }>({
    inFlightKey: null,
    pending: null,
  });
  const lastRequestedPaletteRef = useRef<string | null>(null);
  const renameInputRef = useRef<HTMLInputElement | null>(null);
  const activeTerminalCacheKeyRef = useRef<string | null>(null);
  const restoredStorageScopeRef = useRef<string | null>(null);
  const scrollbackLinesRef = useRef(scrollbackLines);
  const sessionsRef = useRef<TerminalSession[]>([]);
  const localDevice = devices.find((device) => device.is_self);
  const terminalStorageScope = localDevice
    ? `device:${localDevice.device_id}`
    : null;
  const updateSession = useCallback((updated: TerminalSession) => {
    setSessions((current) =>
      current.map((session) =>
        session.session_id === updated.session_id ? updated : session,
      ),
    );
  }, []);

  const clearTerminalSessionCache = useCallback(
    (device: TerminalExecutionDevice, sessionId: string) => {
      const cacheKey = terminalOutputCacheKey(
        device,
        sessionId,
        browserViewerId,
      );
      terminalRuntimeRegistry.dispose(cacheKey);
      setMountedTerminalCacheKeys((current) =>
        current.filter((mountedCacheKey) => mountedCacheKey !== cacheKey),
      );
      if (activeTerminalCacheKeyRef.current === cacheKey) {
        activeTerminalCacheKeyRef.current = null;
      }
    },
    [browserViewerId],
  );

  const clearActiveTerminalOutput = useCallback(() => {
    activeTerminalCacheKeyRef.current = null;
  }, []);

  const activateSession = useCallback(
    (sessionId: string | null, device: TerminalExecutionDevice) => {
      saveActiveExecutionDevice(terminalStorageScope, device);
      saveActiveSessionId(terminalStorageScope, device, sessionId);
      const cacheKey = sessionId
        ? terminalOutputCacheKey(device, sessionId, browserViewerId)
        : null;
      const previousCacheKey = activeTerminalCacheKeyRef.current;
      if (previousCacheKey && previousCacheKey !== cacheKey) {
        recordTerminalDiagnostic({
          event: "visibility_change",
          session_key: previousCacheKey,
          visibility_scope: "session",
          visible: false,
        });
      }
      if (cacheKey && previousCacheKey !== cacheKey) {
        recordTerminalDiagnostic({
          event: "visibility_change",
          session_key: cacheKey,
          session_id: sessionId ?? undefined,
          viewer_id: browserViewerId,
          placement: device.kind,
          visibility_scope: "session",
          visible: true,
        });
      }
      activeTerminalCacheKeyRef.current = cacheKey;
      if (cacheKey) {
        setMountedTerminalCacheKeys((current) =>
          current.includes(cacheKey) ? current : [...current, cacheKey],
        );
      }
      setActiveSessionId(sessionId);
    },
    [browserViewerId, terminalStorageScope],
  );

  useEffect(() => {
    sessionsRef.current = sessions;
  }, [sessions]);

  useEffect(() => {
    const cacheKey = activeTerminalCacheKeyRef.current;
    if (!cacheKey) return;
    recordTerminalDiagnostic({
      event: "visibility_change",
      session_key: cacheKey,
      session_id: activeSessionId ?? undefined,
      viewer_id: browserViewerId,
      placement: executionDevice.kind,
      visibility_scope: "route",
      visible: routeActive,
    });
  }, [activeSessionId, browserViewerId, executionDevice.kind, routeActive]);

  useEffect(() => {
    scrollbackLinesRef.current = scrollbackLines;
  }, [scrollbackLines]);

  useEffect(() => {
    if (!terminalStorageScope) return;
    if (!multiDeviceFeaturesAvailable) return;
    if (restoredStorageScopeRef.current === terminalStorageScope) return;
    restoredStorageScopeRef.current = terminalStorageScope;
    const storedExecutionDevice =
      loadActiveExecutionDevice(terminalStorageScope);
    if (
      executionDeviceKey(storedExecutionDevice) !==
      executionDeviceKey(executionDevice)
    ) {
      setExecutionDevice(storedExecutionDevice);
    }
  }, [executionDevice, multiDeviceFeaturesAvailable, terminalStorageScope]);

  useEffect(() => {
    if (!devicesLoaded) return;
    if (terminalStorageScope) {
      const storedExecutionDevice =
        loadActiveExecutionDevice(terminalStorageScope);
      if (
        executionDeviceKey(storedExecutionDevice) !==
        executionDeviceKey(executionDevice)
      ) {
        return;
      }
    }
    let cancelled = false;
    const storedDefault = window.localStorage.getItem(
      defaultProfileStorageKey(executionDevice),
    );
    setTerminalProfiles(null);
    setDefaultProfileId(storedDefault ?? ASK_PROFILE_ID);
    terminalApi
      .listProfiles(executionDevice)
      .then((profiles) => {
        if (cancelled) return;
        setTerminalProfiles(profiles);
        setDefaultProfileId((current) =>
          profiles.os === "android"
            ? profiles.default_profile_id
            : current ?? ASK_PROFILE_ID,
        );
        setCreateProfileId(profiles.default_profile_id);
      })
      .catch((error) => {
        if (!cancelled) setTerminalError(errorMessage(error));
      });
    clearActiveTerminalOutput();
    setTerminalError(null);
    setTerminalStatus(null);
    terminalApi
      .listSessions(executionDevice)
      .then((response) => {
        if (cancelled) return;
        const liveCacheKeys = new Set(
          response.sessions.map((session) =>
            terminalOutputCacheKey(
              executionDevice,
              session.session_id,
              browserViewerId,
            ),
          ),
        );
        setMountedTerminalCacheKeys((current) => {
          for (const cacheKey of current) {
            if (!liveCacheKeys.has(cacheKey)) {
              terminalRuntimeRegistry.dispose(cacheKey);
            }
          }
          return current.filter((cacheKey) => liveCacheKeys.has(cacheKey));
        });
        setSessions(response.sessions);
        activateSession(
          loadActiveSessionId(
            terminalStorageScope,
            executionDevice,
            response.sessions,
          ),
          executionDevice,
        );
      })
      .catch((error) => {
        if (!cancelled) {
          setSessions([]);
          setActiveSessionId(null);
          clearActiveTerminalOutput();
          setTerminalError(errorMessage(error));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [
    activateSession,
    clearActiveTerminalOutput,
    devicesLoaded,
    browserViewerId,
    executionDevice,
    terminalStorageScope,
  ]);

  const openTerminalSession = async (terminalProfileId: string) => {
    try {
      const response = await terminalApi.openSession({
        execution_device: executionDevice,
        browser_viewer_id: browserViewerId,
        scrollback_lines: scrollbackLines,
        terminal_profile_id: terminalProfileId,
      });
      setTerminalError(null);
      setTerminalStatus(null);
      setCreateProfileOpen(false);
      setSessions((current) => [...current, response.session]);
      activateSession(response.session.session_id, executionDevice);
    } catch (error) {
      setTerminalError(errorMessage(error));
    }
  };

  const handleCreate = async () => {
    if (terminalProfiles?.os === "android") {
      await openTerminalSession(terminalProfiles.default_profile_id);
      return;
    }
    const effectiveDefault = defaultProfileId || ASK_PROFILE_ID;
    if (effectiveDefault === ASK_PROFILE_ID) {
      setCreateProfileId(terminalProfiles?.default_profile_id ?? "");
      setRememberCreateProfileDraft(false);
      setCreateProfileOpen(true);
      return;
    }
    await openTerminalSession(effectiveDefault);
  };

  const handleConfirmCreate = async () => {
    if (!createProfileId) return;
    if (rememberCreateProfileDraft) {
      handleSaveDefaultProfile(createProfileId);
    }
    await openTerminalSession(createProfileId);
  };

  const handleSaveDefaultProfile = (profileId: string) => {
    setDefaultProfileId(profileId);
    window.localStorage.setItem(
      defaultProfileStorageKey(executionDevice),
      profileId,
    );
  };

  const handleSaveScrollbackLines = (value: number | null) => {
    if (value === null) return;
    const next = clampNumber(value, MIN_SCROLLBACK_LINES, MAX_SCROLLBACK_LINES);
    if (!Number.isFinite(next)) return;
    const rounded = Math.round(next);
    setScrollbackLines(rounded);
    window.localStorage.setItem(SCROLLBACK_LINES_KEY, String(rounded));
    if (activeSessionId) {
      void terminalApi
        .updateScrollback(activeSessionId, rounded, executionDevice)
        .catch((error) => setTerminalError(errorMessage(error)));
    }
  };

  const handleSaveKillConfirmEnabled = (enabled: boolean) => {
    setKillConfirmEnabled(enabled);
    if (enabled) {
      window.localStorage.removeItem(SKIP_KILL_CONFIRM_KEY);
    } else {
      window.localStorage.setItem(SKIP_KILL_CONFIRM_KEY, "true");
    }
  };

  const selectSession = (sessionId: string | null) => {
    activateSession(sessionId, executionDevice);
    setTerminalError(null);
    setTerminalStatus(null);
  };

  const selectExecutionDevice = (device: TerminalExecutionDevice) => {
    for (const session of sessions) {
      void terminalApi
        .detachSession(session.session_id, browserViewerId, executionDevice)
        .catch(() => undefined);
    }
    terminalRuntimeRegistry.disposeAll();
    saveActiveExecutionDevice(terminalStorageScope, device);
    setExecutionDevice(device);
    setSessions([]);
    setActiveSessionId(null);
    clearActiveTerminalOutput();
    setMountedTerminalCacheKeys([]);
    setTerminalError(null);
    setTerminalStatus(null);
  };

  const handleTakeInputLease = async () => {
    if (!activeSession) return;
    setViewportGeometry((current) =>
      current?.sessionId === activeSession.session_id ? null : current,
    );
    if (
      geometrySyncRef.current.pending?.sessionId === activeSession.session_id
    ) {
      geometrySyncRef.current.pending = null;
    }
    const response = await terminalApi.takeInputLease(
      activeSession.session_id,
      browserViewerId,
      executionDevice,
    );
    updateSession(response.session);
  };

  const executeKill = async () => {
    if (!activeSession) return;
    setKillLoading(true);
    const killedSessionId = activeSession.session_id;
    const nextSessionId = nextSessionIdAfterRemoval(sessions, killedSessionId);
    try {
      await terminalApi.killSession(
        killedSessionId,
        browserViewerId,
        executionDevice,
      );
      clearTerminalSessionCache(executionDevice, killedSessionId);
      setSessions((current) =>
        current.filter((session) => session.session_id !== killedSessionId),
      );
      if (activeSessionId === killedSessionId) {
        activateSession(nextSessionId, executionDevice);
      }
      setTerminalError(null);
      setTerminalStatus(null);
      setKillConfirmOpen(false);
    } catch (error) {
      setTerminalError(errorMessage(error));
    } finally {
      setKillLoading(false);
    }
  };

  const handleKill = async () => {
    if (!activeSession) return;
    if (window.localStorage.getItem(SKIP_KILL_CONFIRM_KEY) === "true") {
      await executeKill();
      return;
    }
    setSkipKillConfirmDraft(false);
    setKillConfirmOpen(true);
  };

  const handleConfirmKill = async () => {
    if (skipKillConfirmDraft) {
      window.localStorage.setItem(SKIP_KILL_CONFIRM_KEY, "true");
      setKillConfirmEnabled(false);
    }
    await executeKill();
  };

  const openRename = () => {
    if (!activeSession) return;
    setRenameSessionId(activeSession.session_id);
    setRenameDraft(
      activeSession.display_name ||
        sessionNames[activeSession.session_id] ||
        defaultSessionTitle(activeSession),
    );
  };

  const handleConfirmRename = async () => {
    if (!renameSessionId) return;
    const trimmed = renameDraft.trim();
    try {
      const response = await terminalApi.renameSession(
        renameSessionId,
        trimmed || null,
        executionDevice,
      );
      updateSession(response.session);
      const next = { ...sessionNames };
      delete next[renameSessionId];
      setSessionNames(next);
      saveSessionNames(next);
      setRenameSessionId(null);
      setRenameDraft("");
    } catch (error) {
      setTerminalError(errorMessage(error));
    }
  };

  const activeSession = sessions.find(
    (session) => session.session_id === activeSessionId,
  );
  const selectedRemoteDevice =
    executionDevice.kind === "remote"
      ? devices.find((device) => device.device_id === executionDevice.device_id)
      : undefined;
  const selectedConnectivityState =
    executionDevice.kind === "local"
      ? "connected"
      : selectedRemoteDevice?.connectivity_state ?? "not_connected";
  const selectedDeviceAcceptsInput = selectedConnectivityState !== "connecting";
  const hasInputLease =
    !!activeSession &&
    activeSession.input_lease_holder?.browser_id === browserViewerId;
  const selectedDeviceLabel =
    executionDevice.kind === "local"
      ? localDevice
        ? deviceLabel(localDevice)
        : t("terminal.localDevice", "Local")
      : devices.find((device) => device.device_id === executionDevice.device_id)
          ?.friendly_name ||
        devices.find((device) => device.device_id === executionDevice.device_id)
          ?.label ||
        executionDevice.device_id.slice(0, 8);
  const defaultSessionTitle = (session: TerminalSession) =>
    `${selectedDeviceLabel} - ${session.profile_label || cwdName(session.cwd)}`;
  const sessionTitle = (session: TerminalSession) =>
    session.display_name ||
    sessionNames[session.session_id] ||
    defaultSessionTitle(session);
  const activeSessionTitle = activeSession
    ? sessionTitle(activeSession)
    : t("terminal.noSession", "No terminal session");
  const activeSessionMeta = activeSession
    ? sessionShortId(activeSession.session_id)
    : null;
  const terminalExitedText = t("terminal.exited", "Terminal exited");
  const activeTerminalCacheKey = activeSession
    ? terminalOutputCacheKey(
        executionDevice,
        activeSession.session_id,
        browserViewerId,
      )
    : null;
  const executionDeviceOptions = [
    {
      value: "local",
      label: localDevice
        ? deviceLabel(localDevice)
        : t("terminal.localDevice", "Local"),
    },
    ...devices
      .filter((device) => !device.is_self)
      .map((device) => ({
        value: `remote:${device.device_id}`,
        label: (
          <span>
            <span>{deviceLabel(device)}</span>
            <span aria-hidden="true">
              {" · "}
              {deviceConnectivityLabel(t, device.connectivity_state)}
            </span>
          </span>
        ),
      })),
  ];
  const sessionOptions =
    sessions.length === 0
      ? [
          {
            value: "",
            label: t("terminal.noSession", "No terminal session"),
            disabled: true,
          },
        ]
      : sessions.map((session) => ({
          value: session.session_id,
          label: `${sessionTitle(session)} · ${sessionShortId(
            session.session_id,
          )}`,
        }));
  const profileOptions = terminalProfiles?.profiles ?? [];
  const terminalProfileSelectOptions = profileOptions.map((profile) => ({
    value: profile.id,
    label: profile.label,
  }));
  const defaultProfileSelectOptions =
    terminalProfiles?.os === "android"
      ? terminalProfileSelectOptions
      : [
          {
            value: ASK_PROFILE_ID,
            label: t("terminal.profileAskEveryTime", "Ask every time"),
          },
          ...terminalProfileSelectOptions,
        ];
  const defaultProfileValue = defaultProfileId ?? ASK_PROFILE_ID;

  const handleGeometryChange = useCallback(
    (sessionId: string, geometry: { cols: number; rows: number }) => {
      setViewportGeometry((current) =>
        current?.sessionId === sessionId &&
        current.cols === geometry.cols &&
        current.rows === geometry.rows
          ? current
          : { sessionId, ...geometry },
      );
    },
    [],
  );

  const flushGeometryRequests = useCallback(async () => {
    const sync = geometrySyncRef.current;
    if (sync.inFlightKey) return;
    while (sync.pending) {
      const request = sync.pending;
      sync.pending = null;
      sync.inFlightKey = request.key;
      try {
        const response = await terminalApi.adaptPtyGeometry(
          request.sessionId,
          browserViewerId,
          request.geometry.cols,
          request.geometry.rows,
          request.executionDevice,
        );
        setSessions((current) =>
          current.map((session) =>
            session.session_id === response.session.session_id
              ? {
                  ...session,
                  pty_geometry: request.geometry,
                }
              : session,
          ),
        );
        recordTerminalDiagnostic({
          event: "geometry_adapt",
          session_key: terminalOutputCacheKey(
            request.executionDevice,
            request.sessionId,
            browserViewerId,
          ),
          session_id: request.sessionId,
          viewer_id: browserViewerId,
          placement: request.executionDevice.kind,
          viewport: request.geometry,
        });
        setTerminalError(null);
      } catch (error) {
        setTerminalError(errorMessage(error));
      } finally {
        sync.inFlightKey = null;
      }
    }
  }, [browserViewerId]);

  const handlePaletteChange = useCallback(
    (palette: TerminalPalette) => {
      if (!activeSession || !hasInputLease) return;
      const sessionId = activeSession.session_id;
      const requestKey = `${executionDeviceKey(executionDevice)}:${sessionId}:${
        palette.foreground
      }:${palette.background}:${palette.cursor}`;
      if (lastRequestedPaletteRef.current === requestKey) return;
      lastRequestedPaletteRef.current = requestKey;
      void terminalApi
        .updatePalette(sessionId, browserViewerId, palette, executionDevice)
        .then(() => setTerminalError(null))
        .catch((error) => {
          if (lastRequestedPaletteRef.current === requestKey) {
            lastRequestedPaletteRef.current = null;
          }
          setTerminalError(errorMessage(error));
        });
    },
    [activeSession, browserViewerId, executionDevice, hasInputLease],
  );

  useEffect(() => {
    if (!hasInputLease) lastRequestedPaletteRef.current = null;
  }, [activeSession?.session_id, hasInputLease]);

  const handleTerminalInput = useCallback(
    async (data: string) => {
      if (!activeSession || !hasInputLease || !selectedDeviceAcceptsInput) {
        return;
      }
      return terminalApi.sendInput(
        activeSession.session_id,
        browserViewerId,
        data,
        executionDevice,
      );
    },
    [
      activeSession,
      browserViewerId,
      executionDevice,
      hasInputLease,
      selectedDeviceAcceptsInput,
    ],
  );

  useEffect(() => {
    if (!hasInputLease) {
      setViewportGeometry((current) =>
        current?.sessionId === activeSession?.session_id ? null : current,
      );
      if (
        geometrySyncRef.current.pending?.sessionId === activeSession?.session_id
      ) {
        geometrySyncRef.current.pending = null;
      }
    }
  }, [activeSession?.session_id, hasInputLease]);

  useEffect(() => {
    if (!routeActive || !activeSession || !hasInputLease) return;
    if (
      !viewportGeometry ||
      viewportGeometry.sessionId !== activeSession.session_id
    ) {
      return;
    }
    if (viewportGeometry.cols <= 0 || viewportGeometry.rows <= 0) return;
    if (
      activeSession.pty_geometry?.cols === viewportGeometry.cols &&
      activeSession.pty_geometry?.rows === viewportGeometry.rows
    ) {
      return;
    }

    const requestKey = `${executionDeviceKey(executionDevice)}:${
      activeSession.session_id
    }:${viewportGeometry.cols}x${viewportGeometry.rows}`;
    const sync = geometrySyncRef.current;
    if (sync.inFlightKey === requestKey || sync.pending?.key === requestKey) {
      return;
    }
    sync.pending = {
      executionDevice,
      geometry: {
        cols: viewportGeometry.cols,
        rows: viewportGeometry.rows,
      },
      key: requestKey,
      sessionId: activeSession.session_id,
    };
    void flushGeometryRequests();
  }, [
    activeSession,
    browserViewerId,
    executionDevice,
    flushGeometryRequests,
    hasInputLease,
    routeActive,
    viewportGeometry,
  ]);

  const handleAppliedTerminalFrame = useCallback(
    (
      cacheKey: string,
      sessionId: string,
      frame: TerminalStreamFrame,
      runtimeCursor: TerminalStreamCursor,
    ) => {
      recordTerminalDiagnostic({
        event: "frame_classified",
        session_key: cacheKey,
        session_id: sessionId,
        viewer_id: browserViewerId,
        placement: executionDevice.kind,
        cursor_after: runtimeCursor,
        activity_sequence: frame.activity_sequence,
        start_offset: frame.type === "output" ? frame.start_offset : undefined,
        end_offset: frame.type === "output" ? frame.end_offset : undefined,
        frame_classification: "apply",
      });
      if (frame.type === "output") {
        if (activeTerminalCacheKeyRef.current === cacheKey) {
          setTerminalError(null);
          setTerminalStatus(null);
        }
      } else if (frame.type === "input_lease_changed") {
        setSessions((current) =>
          current.map((session) =>
            session.session_id === sessionId
              ? { ...session, input_lease_holder: frame.holder ?? null }
              : session,
          ),
        );
      } else if (frame.type === "pty_geometry_changed") {
        setSessions((current) =>
          current.map((session) =>
            session.session_id === sessionId
              ? { ...session, pty_geometry: frame.geometry }
              : session,
          ),
        );
      } else if (frame.type === "exited") {
        const wasActive = activeTerminalCacheKeyRef.current === cacheKey;
        const nextSessionId = nextSessionIdAfterRemoval(
          sessionsRef.current,
          sessionId,
        );
        clearTerminalSessionCache(executionDevice, sessionId);
        setSessions((current) =>
          current.filter((session) => session.session_id !== sessionId),
        );
        if (wasActive) {
          activateSession(nextSessionId, executionDevice);
          setTerminalStatus(terminalExitedText);
        }
      } else if (frame.type === "error" && frame.detail) {
        if (activeTerminalCacheKeyRef.current === cacheKey) {
          setTerminalError(frame.detail);
        }
      }
    },
    [
      activateSession,
      browserViewerId,
      clearTerminalSessionCache,
      executionDevice,
      terminalExitedText,
    ],
  );

  return (
    <Page className="rua-terminal-page">
      <PageHeader
        className="rua-terminal-page-header"
        items={[{ title: t("terminal.title", "Terminal") }]}
        moreOptionsMenuItems={[
          {
            key: "terminal-options",
            label: t("terminal.options", "Terminal options"),
            onClick: () => setTerminalOptionsOpen(true),
          },
        ]}
        extra={
          <>
            {multiDeviceFeaturesAvailable && (
              <label className="rua-terminal-field rua-terminal-device-field">
                <Select
                  aria-label={t("terminal.executionDevice", "Device")}
                  className="rua-terminal-select rua-terminal-device-select"
                  value={executionDeviceKey(executionDevice)}
                  options={executionDeviceOptions}
                  onChange={(value) =>
                    selectExecutionDevice(executionDeviceFromKey(String(value)))
                  }
                />
              </label>
            )}
            <label className="rua-terminal-field rua-terminal-session-field">
              <Select
                aria-label={t("terminal.session", "Session")}
                className="rua-terminal-select rua-terminal-session-select"
                disabled={sessions.length === 0}
                value={activeSessionId ?? ""}
                options={sessionOptions}
                onChange={(value) =>
                  selectSession(value ? String(value) : null)
                }
              />
            </label>
            <Btn
              type="button"
              variant="filled"
              icon={<Plus size={14} aria-hidden="true" />}
              aria-label={t("terminal.newTerminal", "New terminal")}
              title={t("terminal.newTerminal", "New terminal")}
              onClick={handleCreate}
              disabled={!terminalProfiles}
            />
          </>
        }
      />
      <PageBody className="rua-terminal-body">
        <main className="rua-terminal-viewport">
          <div className="rua-terminal-session-header">
            <div className="rua-terminal-title-stack">
              <span>{activeSessionTitle}</span>
              {executionDevice.kind === "remote" && (
                <DeviceConnectivityTag
                  state={selectedConnectivityState}
                  className="rua-terminal-connectivity-tag"
                />
              )}
              {activeSession && (
                <small title={activeSession.session_id}>
                  {activeSessionMeta}
                </small>
              )}
            </div>
            {activeSession && (
              <div className="rua-terminal-session-actions">
                {!hasInputLease && (
                  <button
                    type="button"
                    className="rua-terminal-icon-button"
                    aria-label={t(
                      "terminal.takeInputLease",
                      "Take input lease",
                    )}
                    title={t("terminal.takeInputLease", "Take input lease")}
                    onClick={handleTakeInputLease}
                  >
                    <Lock size={14} aria-hidden="true" />
                  </button>
                )}
                <button
                  type="button"
                  className="rua-terminal-icon-button"
                  aria-label={t("terminal.rename", "Rename terminal")}
                  title={t("terminal.rename", "Rename terminal")}
                  onClick={openRename}
                >
                  <Pencil size={13} aria-hidden="true" />
                </button>
                <button
                  type="button"
                  className="rua-terminal-icon-button rua-terminal-icon-button--danger"
                  aria-label={t("terminal.kill", "Kill terminal")}
                  title={t("terminal.kill", "Kill terminal")}
                  onClick={handleKill}
                >
                  <Square size={12} fill="currentColor" aria-hidden="true" />
                </button>
              </div>
            )}
          </div>
          <div className="rua-terminal-viewport-stack">
            {mountedTerminalCacheKeys.map((cacheKey) => {
              const session = sessions.find(
                (candidate) =>
                  terminalOutputCacheKey(
                    executionDevice,
                    candidate.session_id,
                    browserViewerId,
                  ) === cacheKey,
              );
              if (!session) return null;
              const isActive =
                routeActive && cacheKey === activeTerminalCacheKey;
              const sessionHasInputLease =
                isActive &&
                session.input_lease_holder?.browser_id === browserViewerId;
              return (
                <div
                  key={cacheKey}
                  className={`rua-terminal-viewport-slot${
                    isActive ? " is-active" : ""
                  }`}
                  aria-hidden={!isActive}
                >
                  <TerminalViewport
                    active={isActive}
                    activity={{
                      browserViewerId,
                      executionDeviceQueryValue:
                        executionDeviceQuery(executionDevice),
                      placement: executionDevice.kind,
                      sessionId: session.session_id,
                      onFrame: (frame, cursor) =>
                        handleAppliedTerminalFrame(
                          cacheKey,
                          session.session_id,
                          frame,
                          cursor,
                        ),
                      onError: (error) => {
                        if (activeTerminalCacheKeyRef.current === cacheKey) {
                          setTerminalError(errorMessage(error));
                        }
                      },
                      onRecoveryComplete: () => {
                        if (activeTerminalCacheKeyRef.current === cacheKey) {
                          setTerminalError(null);
                        }
                      },
                      onRecoveryError: (reason, error) => {
                        if (terminalSessionIsGone(error)) {
                          const wasActive =
                            activeTerminalCacheKeyRef.current === cacheKey;
                          const nextSessionId = nextSessionIdAfterRemoval(
                            sessionsRef.current,
                            session.session_id,
                          );
                          clearTerminalSessionCache(
                            executionDevice,
                            session.session_id,
                          );
                          setSessions((current) =>
                            current.filter(
                              (candidate) =>
                                candidate.session_id !== session.session_id,
                            ),
                          );
                          if (wasActive) {
                            activateSession(nextSessionId, executionDevice);
                            setTerminalStatus(terminalExitedText);
                          }
                          setTerminalError(null);
                          return;
                        }
                        if (activeTerminalCacheKeyRef.current === cacheKey) {
                          setTerminalError(
                            `terminal recovery failed (${reason}): ${errorMessage(
                              error,
                            )}`,
                          );
                        }
                      },
                      requestAttachment: () =>
                        terminalApi
                          .attachSession(
                            session.session_id,
                            browserViewerId,
                            scrollbackLinesRef.current,
                            executionDevice,
                          )
                          .then((attachment) => {
                            updateSession(attachment.session);
                            return attachment;
                          }),
                    }}
                    sessionKey={cacheKey}
                    inputEnabled={
                      sessionHasInputLease && selectedDeviceAcceptsInput
                    }
                    lockedGeometry={
                      isActive && !sessionHasInputLease && session.pty_geometry
                        ? session.pty_geometry
                        : null
                    }
                    scrollbackLines={scrollbackLines}
                    onInput={handleTerminalInput}
                    onGeometryChange={handleGeometryChange}
                    onPaletteChange={handlePaletteChange}
                  />
                </div>
              );
            })}
          </div>
          {terminalStatus && <div role="status">{terminalStatus}</div>}
          {terminalError && <div role="alert">{terminalError}</div>}
        </main>
      </PageBody>
      <Modal
        open={terminalOptionsOpen}
        onClose={() => setTerminalOptionsOpen(false)}
        title={t("terminal.options", "Terminal options")}
        footer={
          <div className="rua-terminal-kill-confirm-footer">
            <Btn
              type="button"
              variant="filled"
              onClick={() => setTerminalOptionsOpen(false)}
            >
              {t("common.done", "Done")}
            </Btn>
          </div>
        }
      >
        <div className="rua-terminal-options">
          <div className="rua-terminal-options-row">
            <span>{t("terminal.currentSystem", "Current system")}</span>
            <strong>{terminalProfiles?.os ?? "-"}</strong>
          </div>
          <div className="rua-terminal-options-row">
            <span>
              {t("terminal.availableProfiles", "Available terminals")}
            </span>
            <strong>
              {profileOptions.map((profile) => profile.label).join(", ") || "-"}
            </strong>
          </div>
          <label className="rua-terminal-options-field">
            <span>{t("terminal.defaultProfile", "Default terminal")}</span>
            <Select
              aria-label={t("terminal.defaultProfile", "Default terminal")}
              value={defaultProfileValue}
              options={defaultProfileSelectOptions}
              onChange={(value) => handleSaveDefaultProfile(String(value))}
            />
          </label>
          <label className="rua-terminal-options-field">
            <span>{t("terminal.scrollbackLines", "Scrollback lines")}</span>
            <NumberInput
              aria-label={t("terminal.scrollbackLines", "Scrollback lines")}
              min={MIN_SCROLLBACK_LINES}
              max={MAX_SCROLLBACK_LINES}
              step={100}
              value={scrollbackLines}
              onChange={handleSaveScrollbackLines}
            />
          </label>
          <Check
            checked={killConfirmEnabled}
            onChange={(event) =>
              handleSaveKillConfirmEnabled(event.target.checked)
            }
          >
            {t("terminal.confirmBeforeKill", "Confirm before terminating")}
          </Check>
        </div>
      </Modal>
      <Modal
        open={createProfileOpen}
        onClose={() => {
          setCreateProfileOpen(false);
          setRememberCreateProfileDraft(false);
        }}
        title={t("terminal.chooseProfileTitle", "Choose terminal")}
        footer={
          <div className="rua-terminal-kill-confirm-footer">
            <Btn
              type="button"
              variant="outline"
              onClick={() => setCreateProfileOpen(false)}
            >
              {t("common.cancel", "Cancel")}
            </Btn>
            <Btn
              type="button"
              variant="filled"
              disabled={!createProfileId}
              onClick={handleConfirmCreate}
            >
              {t("terminal.newTerminal", "New terminal")}
            </Btn>
          </div>
        }
      >
        <label className="rua-terminal-options-field">
          <span>{t("terminal.profile", "Terminal")}</span>
          <Select
            aria-label={t("terminal.profile", "Terminal")}
            value={createProfileId}
            options={terminalProfileSelectOptions}
            onChange={(value) => setCreateProfileId(String(value))}
          />
        </label>
        <Check
          checked={rememberCreateProfileDraft}
          onChange={(event) =>
            setRememberCreateProfileDraft(event.target.checked)
          }
        >
          {t("terminal.rememberProfileChoice", "Do not ask again")}
        </Check>
      </Modal>
      <Modal
        open={killConfirmOpen}
        onClose={() => setKillConfirmOpen(false)}
        title={t("terminal.killConfirmTitle", "Terminate terminal?")}
        footer={
          <div className="rua-terminal-kill-confirm-footer">
            <Btn
              type="button"
              variant="outline"
              onClick={() => setKillConfirmOpen(false)}
              disabled={killLoading}
            >
              {t("common.cancel", "Cancel")}
            </Btn>
            <Btn
              type="button"
              variant="danger-filled"
              onClick={handleConfirmKill}
              loading={killLoading}
            >
              {t("terminal.killConfirmOk", "Terminate")}
            </Btn>
          </div>
        }
      >
        <div className="rua-terminal-kill-confirm">
          <p>
            {t(
              "terminal.killConfirmBody",
              "This will stop the shell process and remove this terminal session.",
            )}
          </p>
          <Check
            checked={skipKillConfirmDraft}
            onChange={(event) => setSkipKillConfirmDraft(event.target.checked)}
          >
            {t("terminal.killConfirmSkip", "Do not ask again")}
          </Check>
        </div>
      </Modal>
      <Modal
        open={renameSessionId !== null}
        onClose={() => setRenameSessionId(null)}
        title={t("terminal.renameTitle", "Rename terminal")}
        footer={
          <div className="rua-terminal-kill-confirm-footer">
            <Btn
              type="button"
              variant="outline"
              onClick={() => setRenameSessionId(null)}
            >
              {t("common.cancel", "Cancel")}
            </Btn>
            <Btn type="button" variant="filled" onClick={handleConfirmRename}>
              {t("common.save", "Save")}
            </Btn>
          </div>
        }
      >
        <div className="rua-terminal-rename-field">
          <input
            ref={renameInputRef}
            className="rua-terminal-rename-input"
            aria-label={t("terminal.renameInput", "Terminal name")}
            value={renameDraft}
            onChange={(event) => setRenameDraft(event.target.value)}
            onKeyDown={(event) => {
              if (event.key !== "Enter") return;
              if (event.nativeEvent.isComposing || event.keyCode === 229) {
                return;
              }
              event.preventDefault();
              void handleConfirmRename();
            }}
            autoFocus
          />
          {renameDraft && (
            <button
              type="button"
              className="rua-terminal-rename-clear"
              aria-label={t("terminal.clearRename", "Clear terminal name")}
              title={t("terminal.clearRename", "Clear terminal name")}
              onClick={() => {
                setRenameDraft("");
                renameInputRef.current?.focus();
              }}
            >
              <X size={16} aria-hidden="true" />
            </button>
          )}
        </div>
      </Modal>
    </Page>
  );
}
