import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { PageHeader } from "../../components/PageHeader";
import { Page, PageBody } from "@/components/Page";
import { RuaCard } from "@/components/RuaCard";
import styles from "./index.module.less";
import dayjs from "dayjs";
import {
  debugApi,
  type BackendDebugLogsResponse,
} from "../../api/modules/debug";
import { copyText } from "../../utils/clipboard";
import {
  Btn,
  InfoBox,
  Loading,
  Select,
  Tag,
  TextInput,
  Toggle,
  toast,
} from "@/ui";

const BACKEND_LOG_LINES = 200;
const BACKEND_REFRESH_MS = 3000;

type BackendLevelFilter = "all" | "debug" | "info" | "warning" | "error";

function backendLevelColor(
  level: BackendLevelFilter,
): "primary" | "danger" | "success" | "warning" | "default" {
  if (level === "error") return "danger";
  if (level === "warning") return "warning";
  if (level === "info") return "primary";
  if (level === "debug") return "success";
  return "default";
}

function escapeRegExp(input: string): string {
  return input.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function highlightLine(line: string, needle: string): ReactNode {
  const q = needle.trim();
  if (!q) return line;
  const re = new RegExp(escapeRegExp(q), "ig");
  const parts: ReactNode[] = [];
  let lastIndex = 0;
  let match: RegExpExecArray | null;
  while ((match = re.exec(line))) {
    const start = match.index;
    const end = start + match[0].length;
    if (start > lastIndex) {
      parts.push(line.slice(lastIndex, start));
    }
    parts.push(
      <mark
        key={`${start}-${end}`}
        style={{
          background: "var(--warning-bg)",
          padding: 0,
        }}
      >
        {line.slice(start, end)}
      </mark>,
    );
    lastIndex = end;
  }
  if (lastIndex < line.length) parts.push(line.slice(lastIndex));
  return parts;
}

export default function DebugPage() {
  const { t } = useTranslation();
  const [backendLogs, setBackendLogs] =
    useState<BackendDebugLogsResponse | null>(null);
  const [initialLoading, setInitialLoading] = useState(true);
  const firstFetchDone = useRef(false);
  const [backendError, setBackendError] = useState("");
  const [autoRefresh, setAutoRefresh] = useState(true);
  const [backendNewestFirst, setBackendNewestFirst] = useState(true);
  const [backendLevel, setBackendLevel] = useState<BackendLevelFilter>("all");
  const [backendQuery, setBackendQuery] = useState("");

  const loadBackendLogs = useCallback(
    async (opts?: { successToast?: boolean }) => {
      const isFirstFetch = !firstFetchDone.current;
      try {
        const res = await debugApi.getBackendLogs(BACKEND_LOG_LINES);
        setBackendLogs(res);
        setBackendError("");
        if (opts?.successToast) {
          toast.success(t("debug.actions.refreshSuccess", "Logs refreshed"));
        }
      } catch (error) {
        setBackendError(
          error instanceof Error
            ? error.message
            : t("debug.backend.loadFailed", "Failed to load backend logs"),
        );
        if (opts?.successToast) {
          toast.error(
            error instanceof Error
              ? error.message
              : t("debug.backend.loadFailed", "Failed to load backend logs"),
          );
        }
      } finally {
        if (isFirstFetch) {
          firstFetchDone.current = true;
          setInitialLoading(false);
        }
      }
    },
    [t],
  );

  useEffect(() => {
    void loadBackendLogs();
  }, [loadBackendLogs]);

  useEffect(() => {
    if (!autoRefresh) return;
    let cancelled = false;
    let timeoutId: number | undefined;

    const tick = async () => {
      if (cancelled) return;
      await loadBackendLogs();
      if (cancelled) return;
      timeoutId = window.setTimeout(() => {
        void tick();
      }, BACKEND_REFRESH_MS);
    };

    timeoutId = window.setTimeout(() => {
      void tick();
    }, BACKEND_REFRESH_MS);
    return () => {
      cancelled = true;
      if (timeoutId !== undefined) {
        window.clearTimeout(timeoutId);
      }
    };
  }, [autoRefresh, loadBackendLogs]);

  const handleCopyBackend = async () => {
    const ok = await copyText(filteredBackendText);
    if (ok) {
      toast.success(t("common.copied"));
    } else {
      toast.error(t("common.copyFailed"));
    }
  };

  const backendLines = useMemo(() => {
    const raw = backendLogs?.content || "";
    if (!raw.trim()) return [] as string[];
    const lines = raw.split("\n");
    return backendNewestFirst ? [...lines].reverse() : lines;
  }, [backendLogs?.content, backendNewestFirst]);

  const filteredBackendLines = useMemo(() => {
    const q = backendQuery.trim().toLowerCase();
    return backendLines.filter((line) => {
      if (backendLevel !== "all") {
        const lvl = backendLevel.toUpperCase();
        const levelHit =
          line.includes(` ${lvl} `) ||
          line.includes(`| ${lvl} `) ||
          line.includes(`${lvl} `);
        if (!levelHit) return false;
      }
      if (!q) return true;
      return line.toLowerCase().includes(q);
    });
  }, [backendLines, backendLevel, backendQuery]);

  const filteredBackendText = useMemo(
    () => filteredBackendLines.join("\n"),
    [filteredBackendLines],
  );

  return (
    <Page>
      <PageHeader
        current={t("debug.backend.title", "Backend logs")}
        showMoreMenu
        extra={
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: "var(--space-3)",
            }}
          >
            <span style={{ color: "var(--text-2)" }}>
              {t("debug.backend.newestFirst", "Newest first")}
            </span>
            <Toggle
              checked={backendNewestFirst}
              onChange={(e) => setBackendNewestFirst(e.target.checked)}
            />
            <span style={{ color: "var(--text-2)" }}>
              {t("debug.backend.autoRefresh", "Auto refresh")}
            </span>
            <Toggle
              checked={autoRefresh}
              onChange={(e) => setAutoRefresh(e.target.checked)}
            />
          </div>
        }
      />
      <PageBody>
        <div
          style={{
            padding: "16px 20px",
            height: "100%",
            display: "flex",
            flexDirection: "column",
            minHeight: 0,
          }}
        >
          <RuaCard
            className={styles.debugLogCard}
            bare
            style={{
              flex: 1,
              minHeight: 0,
              display: "flex",
              flexDirection: "column",
            }}
          >
            <div
              style={{
                display: "flex",
                flexDirection: "column",
                gap: 16,
                flex: 1,
                minHeight: 0,
              }}
            >
              <div
                style={{
                  display: "flex",
                  flexWrap: "wrap",
                  gap: "var(--space-2)",
                }}
              >
                <Btn
                  onClick={() => void loadBackendLogs({ successToast: true })}
                >
                  {t("debug.actions.refreshBackend", "Refresh")}
                </Btn>
                <Btn onClick={() => void handleCopyBackend()}>
                  {t("debug.actions.copyBackend", "Copy")}
                </Btn>
                <Select
                  style={{ width: 197 }}
                  value={backendLevel}
                  onChange={(v) => setBackendLevel(v)}
                  options={[
                    { value: "all", label: t("debug.level.all", "All") },
                    {
                      value: "error",
                      label: (
                        <Tag color={backendLevelColor("error")}>ERROR</Tag>
                      ),
                    },
                    {
                      value: "warning",
                      label: (
                        <Tag color={backendLevelColor("warning")}>WARNING</Tag>
                      ),
                    },
                    {
                      value: "info",
                      label: <Tag color={backendLevelColor("info")}>INFO</Tag>,
                    },
                    {
                      value: "debug",
                      label: (
                        <Tag color={backendLevelColor("debug")}>DEBUG</Tag>
                      ),
                    },
                  ]}
                />
                <TextInput
                  style={{ width: 320 }}
                  value={backendQuery}
                  onChange={(e) => setBackendQuery(e.target.value)}
                  placeholder={t(
                    "debug.backend.searchPlaceholder",
                    "Search backend logs...",
                  )}
                />
                {backendLogs?.updated_at && (
                  <span style={{ color: "var(--text-2)" }}>
                    {t("debug.backend.updatedAt", "Updated at")}:{" "}
                    {dayjs(backendLogs.updated_at * 1000).format(
                      "YYYY-MM-DD HH:mm:ss",
                    )}
                  </span>
                )}
              </div>

              {backendError ? (
                <InfoBox type="error" title={backendError} />
              ) : !backendLogs?.exists ? (
                <InfoBox
                  type="warning"
                  title={t(
                    "debug.backend.notFound",
                    "Backend log file was not found yet.",
                  )}
                />
              ) : null}

              <Loading
                spinning={initialLoading}
                tip={t("common.loading", "Loading")}
                className={styles.debugLogSpin}
              >
                <div
                  style={{
                    border: "1px solid rgba(0,0,0,0.06)",
                    padding: 12,
                    background: "var(--bg-disabled)",
                    fontFamily: "var(--font-mono)",
                    fontSize: 12,
                    lineHeight: 1.5,
                    whiteSpace: "pre-wrap",
                    wordBreak: "break-word",
                    flex: 1,
                    minHeight: 0,
                    overflow: "auto",
                  }}
                >
                  {filteredBackendLines.length ? (
                    filteredBackendLines.map((line, idx) => (
                      <div key={idx}>{highlightLine(line, backendQuery)}</div>
                    ))
                  ) : (
                    <span style={{ color: "var(--text-2)" }}>
                      {t(
                        "debug.backend.placeholder",
                        "Backend log output will appear here.",
                      )}
                    </span>
                  )}
                </div>
              </Loading>
            </div>
          </RuaCard>
        </div>
      </PageBody>
    </Page>
  );
}
