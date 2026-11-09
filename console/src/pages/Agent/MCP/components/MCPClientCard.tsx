import type { MCPClientInfo, MCPToolInfo } from "../../../../api/types";
import type { McpStatusEntry } from "../../../../api/modules/mcp";
import type { ColumnDef } from "@/ui/Table";
import { useTranslation } from "react-i18next";
import { useState, useEffect, useMemo } from "react";
import { ArrowUpNarrowWide } from "lucide-react";
import { agentsApi } from "../../../../api/modules/agents";
import { EmptyState } from "@/components/EmptyState";
import { RuaCard } from "@/components/RuaCard";
import styles from "../index.module.less";
import {
  Btn,
  Check,
  Dropdown,
  Loading,
  Modal,
  Table,
  TextArea,
  Toggle,
  Tooltip,
  toast,
} from "@/ui";
import { openOAuthAuthorization } from "@/desktop/oauth";

interface MCPClientUpdate {
  name?: string;
  description?: string;
  command?: string;
  enabled?: boolean;
  transport?: "stdio" | "streamable_http" | "sse";
  url?: string;
  headers?: Record<string, string>;
  args?: string[];
  env?: Record<string, string>;
  cwd?: string;
}

interface MCPClientCardProps {
  client: MCPClientInfo;
  agentId: string;

  status?: McpStatusEntry;
  onToggle: (client: MCPClientInfo, e: React.MouseEvent) => void;
  onDelete: (client: MCPClientInfo, e: React.MouseEvent) => void;
  onUpdate: (key: string, updates: MCPClientUpdate) => Promise<boolean>;
  onRefresh: () => Promise<void>;
  isHovered: boolean;
  onMouseEnter: () => void;
  onMouseLeave: () => void;

  selectMode?: boolean;
  selected?: boolean;
  onSelectToggle?: () => void;
}

type ToolSortMode = "name" | "enabled";
type SortOrder = "asc" | "desc";

export function MCPClientCard({
  client,
  agentId,
  status,
  onToggle,
  onDelete,
  onUpdate,
  onRefresh,
  onMouseEnter,
  onMouseLeave,
  selectMode = false,
  selected = false,
  onSelectToggle,
}: MCPClientCardProps) {
  const { t } = useTranslation();
  const [jsonModalOpen, setJsonModalOpen] = useState(false);
  const [deleteModalOpen, setDeleteModalOpen] = useState(false);
  const [toolsModalOpen, setToolsModalOpen] = useState(false);
  const [errorModalOpen, setErrorModalOpen] = useState(false);
  const [editedJson, setEditedJson] = useState("");
  const [isEditing, setIsEditing] = useState(false);
  const [enteredFromEdit, setEnteredFromEdit] = useState(false);
  const [tools, setTools] = useState<MCPToolInfo[]>([]);
  const [toolsLoading, setToolsLoading] = useState(false);
  const [toolSearch, setToolSearch] = useState("");
  const [toolSortMode, setToolSortMode] = useState<ToolSortMode>("enabled");
  const [toolSortOrder, setToolSortOrder] = useState<SortOrder>("asc");
  const [toolToggleBusy, setToolToggleBusy] = useState<string | null>(null);
  const [schemaTool, setSchemaTool] = useState<MCPToolInfo | null>(null);
  const [oauthBusy, setOauthBusy] = useState(false);

  useEffect(() => {
    if (!client.enabled) {
      setTools([]);
      return;
    }
    agentsApi
      .listAgentMCPClientTools(agentId, client.key)
      .then(setTools)
      .catch(() => setTools([]));
  }, [agentId, client.key, client.enabled]);

  const isRemote =
    client.transport === "streamable_http" || client.transport === "sse";
  const clientType = isRemote ? "Remote" : "Local";
  const oauth = client.oauth;
  const nowSeconds = Date.now() / 1000;
  const hasOAuthToken = !!oauth?.access_token;
  const oauthExpired =
    !!oauth && oauth.expires_at > 0 && oauth.expires_at <= nowSeconds;
  const oauthAuthorized =
    status?.auth?.authorized ?? (hasOAuthToken && !oauthExpired);
  const oauthNeedsAction =
    isRemote &&
    ((status?.auth?.required && !oauthAuthorized) ||
      status?.auth?.expired ||
      oauthExpired);
  const showOAuthPanel =
    isRemote &&
    (status?.auth?.kind === "oauth2" || !!oauth || oauthNeedsAction);
  const oauthScope = status?.auth?.scope || oauth?.scope || "";
  const oauthExpiresAt = status?.auth?.expires_at || oauth?.expires_at || 0;
  const enabledToolCount = tools.filter(
    (tool) => tool.enabled !== false,
  ).length;

  const openViewMode = () => {
    const jsonStr = JSON.stringify(client, null, 2);
    setEditedJson(jsonStr);
    setIsEditing(false);
    setEnteredFromEdit(false);
    setJsonModalOpen(true);
  };

  const openEditMode = () => {
    const jsonStr = JSON.stringify(client, null, 2);
    setEditedJson(jsonStr);
    setIsEditing(true);
    setEnteredFromEdit(true);
    setJsonModalOpen(true);
  };

  const handleSaveJson = async () => {
    try {
      const parsed = JSON.parse(editedJson);
      const { key: _key, ...updates } = parsed;
      const success = await onUpdate(client.key, updates);
      if (success) {
        setJsonModalOpen(false);
        setIsEditing(false);
      }
    } catch {
      alert("Invalid JSON format");
    }
  };

  const handleCancelEdit = () => {
    if (isEditing && !enteredFromEdit) {
      const jsonStr = JSON.stringify(client, null, 2);
      setEditedJson(jsonStr);
      setIsEditing(false);
    } else {
      setJsonModalOpen(false);
      setIsEditing(false);
    }
  };

  const refreshTools = async () => {
    if (!client.enabled) {
      setTools([]);
      return;
    }
    setToolsLoading(true);
    try {
      setTools(await agentsApi.listAgentMCPClientTools(agentId, client.key));
    } catch (error: unknown) {
      toast.error(
        error instanceof Error ? error.message : t("mcp.toolsLoadError"),
      );
    } finally {
      setToolsLoading(false);
    }
  };

  const handleOpenTools = () => {
    setToolsModalOpen(true);
    if (client.enabled) {
      refreshTools();
    }
  };

  const handleToggleTool = async (
    tool: MCPToolInfo,
    event?: React.ChangeEvent<HTMLInputElement>,
  ) => {
    event?.stopPropagation();
    if (toolToggleBusy) return;
    const nextEnabled = tool.enabled === false;
    setToolToggleBusy(tool.name);
    setTools((prev) =>
      prev.map((item) =>
        item.name === tool.name ? { ...item, enabled: nextEnabled } : item,
      ),
    );
    try {
      await agentsApi.toggleAgentMCPClientTool(
        agentId,
        client.key,
        tool.name,
        nextEnabled,
      );
      toast.success(
        nextEnabled ? t("mcp.toolEnableSuccess") : t("mcp.toolDisableSuccess"),
      );
    } catch {
      toast.error(t("mcp.toolToggleError"));
      await refreshTools();
    } finally {
      setToolToggleBusy(null);
    }
  };

  const handleAuthorizeOAuth = async () => {
    if (!isRemote || oauthBusy) return;
    setOauthBusy(true);
    try {
      const response = await agentsApi.startAgentMCPOAuth(agentId, client.key, {
        url: client.url,
        scope: oauth?.scope || "",
        client_id: oauth?.client_id || "",
        auth_endpoint: oauth?.auth_endpoint || "",
        token_endpoint: oauth?.token_endpoint || "",
      });
      const launch = await openOAuthAuthorization(
        response.auth_url,
        "mcp-oauth",
        "width=720,height=820",
      );
      const popup = launch.popup;
      const refreshOnResult = async (event?: MessageEvent) => {
        const data = event?.data;
        if (data && data.type !== "mcp-oauth") return;
        window.removeEventListener("message", refreshOnResult);
        await onRefresh();
        if (toolsModalOpen) await refreshTools();
        setOauthBusy(false);
      };
      window.addEventListener("message", refreshOnResult);
      const timer = window.setInterval(async () => {
        if (popup?.closed) {
          window.clearInterval(timer);
          window.removeEventListener("message", refreshOnResult);
          await onRefresh();
          if (toolsModalOpen) await refreshTools();
          setOauthBusy(false);
          return;
        }
        try {
          const status = await agentsApi.getAgentMCPOAuthStatus(
            agentId,
            client.key,
          );
          if (status.authorized) {
            window.clearInterval(timer);
            window.removeEventListener("message", refreshOnResult);
            await onRefresh();
            if (toolsModalOpen) await refreshTools();
            setOauthBusy(false);
          }
        } catch {}
      }, 800);
    } catch (error: unknown) {
      toast.error(
        error instanceof Error ? error.message : t("mcp.oauthStartError"),
      );
      setOauthBusy(false);
    }
  };

  const handleRevokeOAuth = async () => {
    if (!isRemote || oauthBusy) return;
    setOauthBusy(true);
    try {
      await agentsApi.revokeAgentMCPOAuth(agentId, client.key);
      toast.success(t("mcp.oauthRevokeSuccess"));
      await onRefresh();
    } catch (error: unknown) {
      toast.error(
        error instanceof Error ? error.message : t("mcp.oauthRevokeError"),
      );
    } finally {
      setOauthBusy(false);
    }
  };

  const filteredTools = useMemo(() => {
    const query = toolSearch.trim().toLowerCase();
    const filtered = tools.filter((tool) => {
      if (!query) return true;
      return (
        tool.name.toLowerCase().includes(query) ||
        (tool.description || "").toLowerCase().includes(query)
      );
    });

    return [...filtered].sort((a, b) => {
      if (toolSortMode === "enabled") {
        const av = a.enabled === false ? 1 : 0;
        const bv = b.enabled === false ? 1 : 0;
        if (av !== bv) return toolSortOrder === "asc" ? av - bv : bv - av;
        return a.name.localeCompare(b.name);
      }
      const direction = toolSortOrder === "asc" ? 1 : -1;
      const byName = a.name.localeCompare(b.name);
      return byName * direction;
    });
  }, [tools, toolSearch, toolSortMode, toolSortOrder]);

  const toolSortItems = useMemo(
    () => [
      { key: "name-asc", label: t("mcp.toolSortNameAsc") },
      { key: "name-desc", label: t("mcp.toolSortNameDesc") },
      { key: "enabled-asc", label: t("mcp.toolSortEnabledAsc") },
      { key: "enabled-desc", label: t("mcp.toolSortEnabledDesc") },
    ],
    [t],
  );

  const toolColumns: ColumnDef<MCPToolInfo>[] = [
    {
      key: "name",
      title: t("mcp.toolColumnName"),
      dataIndex: "name",
      width: "minmax(180px, 0.9fr)",
      render: (name: string) => (
        <div className={styles.toolsModalItemName}>{name}</div>
      ),
    },
    {
      key: "description",
      title: t("mcp.toolColumnDescription"),
      dataIndex: "description",
      width: "minmax(260px, 1.5fr)",
      render: (description: string | undefined) => (
        <div className={styles.toolsModalItemDesc}>{description || "-"}</div>
      ),
    },
    {
      key: "schema",
      title: t("mcp.toolColumnSchema"),
      width: "110px",
      render: (_: unknown, tool) => (
        <Btn variant="outline" size="sm" onClick={() => setSchemaTool(tool)}>
          {t("mcp.viewSchema")}
        </Btn>
      ),
    },
    {
      key: "operation",
      title: t("mcp.toolColumnExposure"),
      width: "72px",
      align: "right",
      render: (_: unknown, tool) => (
        <div className={styles.toolExposureCell}>
          <Toggle
            checked={tool.enabled !== false}
            disabled={toolToggleBusy === tool.name}
            aria-label={t("mcp.toolColumnExposure")}
            onClick={(e) => e.stopPropagation()}
            onChange={(e) => handleToggleTool(tool, e)}
          />
        </div>
      ),
    },
  ];

  const clientJson = JSON.stringify(client, null, 2);

  const oauthPanel = showOAuthPanel ? (
    <div className={styles.oauthStrip}>
      <div className={styles.oauthInfo}>
        <div className={styles.oauthTitleRow}>
          <span className={styles.oauthTitle}>OAuth</span>
          <span
            className={`${styles.oauthBadge} ${
              oauthAuthorized
                ? styles.oauthOk
                : oauthExpired || status?.auth?.expired
                ? styles.oauthExpired
                : styles.oauthMissing
            }`}
          >
            {oauthAuthorized
              ? t("mcp.oauthAuthorized")
              : oauthExpired || status?.auth?.expired
              ? t("mcp.oauthExpired")
              : t("mcp.oauthMissing")}
          </span>
          {oauthScope && (
            <span className={styles.oauthScope}>{oauthScope}</span>
          )}
        </div>
        <div className={styles.oauthMeta}>
          {oauthExpiresAt > 0
            ? `${t("mcp.oauthExpiresAt")} ${new Date(
                oauthExpiresAt * 1000,
              ).toLocaleString()}`
            : t("mcp.oauthNoExpiry")}
        </div>
      </div>
      <div className={styles.oauthActions}>
        <Btn
          variant="outline"
          size="sm"
          disabled={oauthBusy}
          onClick={(e) => {
            e.stopPropagation();
            handleAuthorizeOAuth();
          }}
        >
          {oauthAuthorized
            ? t("mcp.oauthReauthorize")
            : t("mcp.oauthAuthorize")}
        </Btn>
        {hasOAuthToken && (
          <Btn
            variant="danger"
            size="sm"
            disabled={oauthBusy}
            onClick={(e) => {
              e.stopPropagation();
              handleRevokeOAuth();
            }}
          >
            {t("mcp.oauthRevoke")}
          </Btn>
        )}
      </div>
    </div>
  ) : null;

  const titleNode = (
    <Tooltip title={client.name}>
      <span className={styles.mcpTitle}>{client.name}</span>
    </Tooltip>
  );

  const tagNodes = [
    <span
      key="type"
      className={`${styles.typeBadge} ${
        isRemote ? styles.remote : styles.local
      }`}
    >
      {clientType}
    </span>,
  ];

  const actionNodes = selectMode
    ? [
        <Check
          key="check"
          checked={selected}
          onChange={() => onSelectToggle?.()}
          onClick={(e) => e.stopPropagation()}
        />,
      ]
    : [
        <Toggle
          key="sw"
          checked={client.enabled}
          onChange={() => onToggle(client, null as unknown as React.MouseEvent)}
          onClick={(e) => e.stopPropagation()}
        />,
      ];

  const handleCardClick = () => {
    if (selectMode) {
      onSelectToggle?.();
      return;
    }
    openViewMode();
  };

  return (
    <>
      <RuaCard
        hoverable
        selected={selectMode && selected}
        onClick={handleCardClick}
        onMouseEnter={onMouseEnter}
        onMouseLeave={onMouseLeave}
        className={`${styles.mcpCard}${
          !client.enabled ? ` ${styles.dimmed}` : ""
        }`}
        title={titleNode}
        tags={tagNodes}
        actions={actionNodes}
        footer={
          !selectMode ? (
            <>
              <Btn
                variant="outline"
                onClick={(e) => {
                  e.stopPropagation();
                  openEditMode();
                }}
              >
                {t("common.edit")}
              </Btn>
              <Btn
                variant="danger"
                onClick={(e) => {
                  e.stopPropagation();
                  setDeleteModalOpen(true);
                }}
              >
                {t("common.delete")}
              </Btn>
            </>
          ) : undefined
        }
      >
        <p className={styles.mcpDescription}>{client.description || "-"}</p>

        {!selectMode && (
          <div className={styles.toolsRow}>
            <div className={styles.statusActionGroup}>
              {renderStatusPill(client, status, t, () =>
                setErrorModalOpen(true),
              )}
              {oauthNeedsAction && (
                <Btn
                  className={styles.oauthInlineButton}
                  variant="outline"
                  size="sm"
                  disabled={oauthBusy}
                  onClick={(e) => {
                    e.stopPropagation();
                    handleAuthorizeOAuth();
                  }}
                >
                  {t("mcp.oauthAuthorize")}
                </Btn>
              )}
            </div>
            <Btn
              className={styles.toolsButton}
              variant="outline"
              size="sm"
              onClick={(e) => {
                e.stopPropagation();
                handleOpenTools();
              }}
            >
              {t("mcp.toolsList")} ({enabledToolCount}/{tools.length})
            </Btn>
          </div>
        )}
      </RuaCard>

      {}
      <Modal
        title={t("common.confirm")}
        open={deleteModalOpen}
        onClose={() => setDeleteModalOpen(false)}
        footer={
          <>
            <Btn variant="outline" onClick={() => setDeleteModalOpen(false)}>
              {t("common.cancel")}
            </Btn>
            <Btn
              variant="danger"
              onClick={() => {
                setDeleteModalOpen(false);
                onDelete(client, null as unknown as React.MouseEvent);
              }}
            >
              {t("common.confirm")}
            </Btn>
          </>
        }
      >
        <p>{t("mcp.deleteConfirm")}</p>
      </Modal>

      {}
      <Modal
        title={t("mcp.status.failedTitle")}
        open={errorModalOpen}
        onClose={() => setErrorModalOpen(false)}
        footer={null}
        width={560}
        maskClosable
      >
        <pre
          style={{
            maxHeight: 320,
            overflow: "auto",
            whiteSpace: "pre-wrap",
            wordBreak: "break-word",
            fontSize: 12,
            margin: 0,
            padding: 12,
            background: "var(--surface-2)",
            border: "1px solid var(--border-soft)",
            borderRadius: "var(--radius-sm)",
          }}
        >
          {status?.last_error || t("mcp.status.failedNoDetail")}
        </pre>
      </Modal>

      {}
      <Modal
        className={`modal-fullscreen-mobile ${styles.toolsModal}`}
        title={`${client.name} - ${t("mcp.toolsList")}`}
        open={toolsModalOpen}
        onClose={() => setToolsModalOpen(false)}
        footer={null}
        width={960}
      >
        {toolsLoading ? (
          <div className={styles.toolsModalLoading}>
            <Loading tip={t("common.loading")} />
          </div>
        ) : !client.enabled ? (
          <EmptyState
            variant="not-available"
            description={t("mcp.clientDisabled")}
          />
        ) : (
          <div className={styles.toolsManager}>
            {oauthPanel}
            {tools.length === 0 ? (
              <EmptyState description={t("mcp.noTools")} />
            ) : (
              <>
                <div className={styles.toolsModalToolbar}>
                  <div className={styles.toolsSearchGroup}>
                    <input
                      className={styles.toolsSearchInput}
                      value={toolSearch}
                      onChange={(e) => setToolSearch(e.target.value)}
                      placeholder={t("mcp.toolSearchPlaceholder")}
                    />
                    <Dropdown
                      menu={{
                        items: toolSortItems,
                        selectedKeys: [`${toolSortMode}-${toolSortOrder}`],
                        onClick: ({ key }: { key: string }) => {
                          const idx = key.lastIndexOf("-");
                          setToolSortMode(key.slice(0, idx) as ToolSortMode);
                          setToolSortOrder(key.slice(idx + 1) as SortOrder);
                        },
                      }}
                      trigger={["click"]}
                    >
                      <Btn
                        variant="outline"
                        size="sm"
                        icon={<ArrowUpNarrowWide />}
                        aria-label={t("mcp.toolSort")}
                      />
                    </Dropdown>
                  </div>
                  <div className={styles.toolsCountText}>
                    {enabledToolCount}/{tools.length} {t("mcp.toolsExposed")}
                  </div>
                </div>

                <div className={styles.toolsTableRegion}>
                  <Table<MCPToolInfo>
                    className={styles.toolsTable}
                    dataSource={filteredTools}
                    columns={toolColumns}
                    rowKey="name"
                    pagination={false}
                    responsive="stack"
                    emptyText={t("mcp.noTools")}
                    onRow={(tool) => ({
                      className:
                        tool.enabled === false ? styles.toolDisabled : "",
                    })}
                  />
                </div>
              </>
            )}
          </div>
        )}
      </Modal>

      <Modal
        className="modal-fullscreen-mobile"
        title={
          schemaTool
            ? `${schemaTool.name} - ${t("mcp.toolColumnSchema")}`
            : t("mcp.toolColumnSchema")
        }
        open={!!schemaTool}
        onClose={() => setSchemaTool(null)}
        footer={
          <Btn variant="filled" onClick={() => setSchemaTool(null)}>
            {t("common.close")}
          </Btn>
        }
        width={720}
      >
        <pre className={styles.schemaPreview}>
          {JSON.stringify(schemaTool?.schema ?? {}, null, 2)}
        </pre>
      </Modal>

      {}
      <Modal
        className="modal-fullscreen-mobile"
        title={`${client.name} - Configuration`}
        open={jsonModalOpen}
        onClose={() => setJsonModalOpen(false)}
        footer={
          <div style={{ display: "flex", justifyContent: "flex-end", gap: 8 }}>
            <Btn variant="outline" onClick={handleCancelEdit}>
              {isEditing ? t("common.cancel") : t("common.close")}
            </Btn>
            {isEditing ? (
              <Btn variant="filled" onClick={handleSaveJson}>
                {t("common.save")}
              </Btn>
            ) : (
              <Btn variant="filled" onClick={() => setIsEditing(true)}>
                {t("common.edit")}
              </Btn>
            )}
          </div>
        }
        width={700}
      >
        <div className={styles.maskedFieldHint}>{t("mcp.maskedFieldHint")}</div>
        {isEditing ? (
          <TextArea
            value={editedJson}
            onChange={(e) => setEditedJson(e.target.value)}
            rows={20}
            style={{
              fontFamily: "var(--font-mono)",
              fontSize: 13,
              lineHeight: 1.6,
              padding: 16,
              borderRadius: "var(--radius-sm)",
            }}
          />
        ) : (
          <pre
            style={{
              backgroundColor: "var(--surface-2)",
              color: "var(--text)",
              padding: 16,
              borderRadius: "var(--radius-sm)",
              border: "1px solid var(--border-soft)",
              maxHeight: 500,
              overflow: "auto",
              fontFamily: "var(--font-mono)",
              fontSize: 13,
              lineHeight: 1.6,
            }}
          >
            {clientJson}
          </pre>
        )}
      </Modal>
    </>
  );
}

type TFn = (key: string) => string;

interface StatusPresentation {
  label: string;
  color: string;
  bg: string;
  border: string;

  errorContent?: string | null;
}

function statusPresentation(
  client: MCPClientInfo,
  status: McpStatusEntry | undefined,
  t: TFn,
): StatusPresentation {
  if (!client.enabled) {
    return {
      label: t("mcp.status.disabled"),
      color: "var(--text-mute)",
      bg: "var(--surface-2)",
      border: "var(--border-soft)",
    };
  }
  if (!status) {
    return {
      label: t("mcp.status.unknown"),
      color: "var(--text-mute)",
      bg: "var(--surface-2)",
      border: "var(--border-soft)",
    };
  }
  if (status.auth?.required && !status.auth.authorized) {
    return status.auth.expired
      ? {
          label: t("mcp.oauthExpired"),
          color: "var(--warning-fg)",
          bg: "var(--warning-bg)",
          border: "var(--warning-fg)",
        }
      : {
          label: t("mcp.oauthMissing"),
          color: "var(--danger-fg)",
          bg: "var(--danger-bg)",
          border: "var(--danger-fg)",
        };
  }
  switch (status.status) {
    case "ready":
      return {
        label: t("mcp.status.connected"),
        color: "var(--success-fg)",
        bg: "var(--success-bg)",
        border: "var(--success-fg)",
      };
    case "connecting":
    case "not_connected":
      return {
        label: t("mcp.status.connecting"),
        color: "var(--warning-fg)",
        bg: "var(--warning-bg)",
        border: "var(--warning-fg)",
      };
    case "failed":
    case "degraded":
      return {
        label: t("mcp.status.failed"),
        color: "var(--danger-fg)",
        bg: "var(--danger-bg)",
        border: "var(--danger-fg)",
        errorContent: status.last_error || t("mcp.status.failedNoDetail"),
      };
    case "stopping":
      return {
        label: t("mcp.status.stopping"),
        color: "var(--text-mute)",
        bg: "var(--surface-2)",
        border: "var(--border-soft)",
      };
    case "disabled":
      return {
        label: t("mcp.status.disabled"),
        color: "var(--text-mute)",
        bg: "var(--surface-2)",
        border: "var(--border-soft)",
      };
    default:
      return {
        label: t("mcp.status.unknown"),
        color: "var(--text-mute)",
        bg: "var(--surface-2)",
        border: "var(--border-soft)",
      };
  }
}

function renderStatusPill(
  client: MCPClientInfo,
  status: McpStatusEntry | undefined,
  t: TFn,
  onErrorClick: () => void,
) {
  const p = statusPresentation(client, status, t);
  const clickable = !!p.errorContent;
  const pillStyle: React.CSSProperties = {
    display: "inline-flex",
    alignItems: "center",
    padding: "2px 8px",
    borderRadius: "var(--radius-sm)",
    fontSize: 12,
    fontWeight: 500,
    color: p.color,
    background: p.bg,
    border: `1px solid ${p.border}`,
    cursor: clickable ? "pointer" : "default",
    userSelect: "none",
  };
  return (
    <span
      style={pillStyle}
      onClick={
        clickable
          ? (e) => {
              e.stopPropagation();
              onErrorClick();
            }
          : undefined
      }
    >
      {p.label}
    </span>
  );
}
