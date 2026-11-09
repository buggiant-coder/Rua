import { useCallback, useMemo, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { EmptyState } from "@/components/EmptyState";
import { sharedMcpApi } from "../../../api";
import { useTranslation } from "react-i18next";
import { PageHeader } from "@/components/PageHeader";
import { Page, PageBody } from "@/components/Page";
import Skeleton from "@/components/Skeleton";
import type { MCPClientInfo } from "../../../api/types";
import { useSharedMcp } from "./useSharedMcp";
import { SharedMCPCard, BroadcastModal } from "./components";
import styles from "./index.module.less";
import {
  ArrowUpNarrowWide,
  MoreVertical,
  Plus,
  RefreshCw,
  Send,
  Trash2,
} from "lucide-react";
import {
  Btn,
  Dropdown,
  FormField,
  Modal,
  Select,
  Tabs,
  TextArea,
  TextInput,
  Tooltip,
  toast,
  type MenuProps,
} from "@/ui";

const SORT_MODE_KEY = "rua.sharedMcp.sortField";
const SORT_ORDER_KEY = "rua.sharedMcp.sortOrder";
const UPDATED_MAP_KEY = "rua.sharedMcp.updatedAt";

type SortMode = "name" | "updated";
type SortOrder = "asc" | "desc";
type CreateTab = "json" | "form";

const loadSortMode = (): SortMode => {
  try {
    const v = localStorage.getItem(SORT_MODE_KEY);
    if (v === "name" || v === "updated") return v;
  } catch {}
  return "name";
};

const loadSortOrder = (): SortOrder => {
  try {
    const v = localStorage.getItem(SORT_ORDER_KEY);
    if (v === "asc" || v === "desc") return v;
  } catch {}
  return "desc";
};

const loadUpdatedMap = (): Record<string, number> => {
  try {
    const raw = localStorage.getItem(UPDATED_MAP_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw);
    return typeof parsed === "object" && parsed !== null ? parsed : {};
  } catch {
    return {};
  }
};

const persistUpdatedAt = (key: string): Record<string, number> => {
  const map = loadUpdatedMap();
  map[key] = Date.now();
  try {
    localStorage.setItem(UPDATED_MAP_KEY, JSON.stringify(map));
  } catch {}
  return map;
};

type MCPTransport = "stdio" | "streamable_http" | "sse";

const defaultCreateForm = {
  key: "",
  name: "",
  description: "",
  transport: "streamable_http" as MCPTransport,
  url: "",
  command: "",
  args: "",
  headers: "",
  env: "",
  cwd: "",
};

type CreateForm = typeof defaultCreateForm;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function toStringValue(value: unknown): string {
  if (value == null) return "";
  return String(value);
}

function toStringRecord(value: unknown): Record<string, string> {
  if (!isRecord(value)) return {};
  return Object.fromEntries(
    Object.entries(value).map(([key, item]) => [key, toStringValue(item)]),
  );
}

function normalizeTransport(raw?: unknown): MCPTransport | undefined {
  if (typeof raw !== "string") return undefined;
  const value = raw.trim().toLowerCase();
  switch (value) {
    case "stdio":
      return "stdio";
    case "sse":
      return "sse";
    case "streamablehttp":
    case "streamable_http":
    case "streamable-http":
    case "http":
      return "streamable_http";
    default:
      return undefined;
  }
}

function normalizeClientData(key: string, rawData: Record<string, unknown>) {
  const transport =
    normalizeTransport(rawData.transport ?? rawData.type) ??
    (rawData.url || rawData.baseUrl || !rawData.command
      ? "streamable_http"
      : "stdio");

  const command =
    transport === "stdio" ? (rawData.command ?? "").toString() : "";

  return {
    name: toStringValue(rawData.name) || key,
    description: toStringValue(rawData.description),
    enabled: typeof rawData.enabled === "boolean" ? rawData.enabled : true,
    transport,
    url: toStringValue(rawData.url || rawData.baseUrl),
    headers: toStringRecord(rawData.headers),
    command,
    args: Array.isArray(rawData.args) ? rawData.args.map(String) : [],
    env: toStringRecord(rawData.env),
    cwd: toStringValue(rawData.cwd),
  };
}

function parseKeyValueLines(text: string): Record<string, string> {
  const values: Record<string, string> = {};
  for (const rawLine of text.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line) continue;
    const index = line.indexOf("=");
    if (index <= 0) continue;
    values[line.slice(0, index).trim()] = line.slice(index + 1).trim();
  }
  return values;
}

function parseArgs(text: string): string[] {
  return text
    .split(/[\n, ]+/)
    .map((part) => part.trim())
    .filter(Boolean);
}

interface SharedMCPPageProps {
  embedded?: boolean;
  headerExtraContainer?: HTMLElement | null;
}

export default function SharedMCPPage({
  embedded = false,
  headerExtraContainer,
}: SharedMCPPageProps) {
  const { t } = useTranslation();
  const {
    clients,
    loading,
    loadClients,
    createClient,
    updateClient,
    deleteClient,
  } = useSharedMcp();
  const [hoverKey, setHoverKey] = useState<string | null>(null);
  const [createModalOpen, setCreateModalOpen] = useState(false);
  const [broadcastOpen, setBroadcastOpen] = useState(false);
  const [selectedKeys, setSelectedKeys] = useState<string[]>([]);

  const [selectMode, setSelectMode] = useState(false);
  const [deleteSelectedKeys, setDeleteSelectedKeys] = useState<Set<string>>(
    new Set(),
  );

  const enterSelectMode = useCallback(() => {
    setSelectMode(true);
    setDeleteSelectedKeys(new Set());
  }, []);

  const exitSelectMode = useCallback(() => {
    setSelectMode(false);
    setDeleteSelectedKeys(new Set());
  }, []);

  const toggleDeleteSelect = useCallback((key: string) => {
    setDeleteSelectedKeys((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  }, []);

  const confirmBatchDelete = useCallback(() => {
    if (deleteSelectedKeys.size === 0) return;
    Modal.confirm({
      title: t("common.batchDeleteTitle"),
      content: t("common.batchDeleteConfirm", {
        count: deleteSelectedKeys.size,
      }),
      okText: t("common.delete"),
      danger: true,
      cancelText: t("common.cancel"),
      onOk: async () => {
        const keys = Array.from(deleteSelectedKeys);
        let deleted = 0;
        let failed = 0;
        for (const key of keys) {
          try {
            await sharedMcpApi.delete(key);
            deleted++;
          } catch {
            failed++;
          }
        }
        if (failed === 0) {
          toast.success(t("common.batchDeleteSuccess", { count: deleted }));
        } else {
          toast.warning(t("common.batchDeletePartial", { deleted, failed }));
        }
        exitSelectMode();
        await loadClients();
      },
    });
  }, [deleteSelectedKeys, t, exitSelectMode, loadClients]);

  const [searchTerm, setSearchTerm] = useState("");
  const [sortMode, setSortMode] = useState<SortMode>(loadSortMode);
  const [sortOrder, setSortOrder] = useState<SortOrder>(loadSortOrder);
  const [updatedMap, setUpdatedMap] =
    useState<Record<string, number>>(loadUpdatedMap);

  const updateSortMode = useCallback((mode: SortMode) => {
    setSortMode(mode);
    try {
      localStorage.setItem(SORT_MODE_KEY, mode);
    } catch {}
  }, []);

  const updateSortOrder = useCallback((order: SortOrder) => {
    setSortOrder(order);
    try {
      localStorage.setItem(SORT_ORDER_KEY, order);
    } catch {}
  }, []);

  const touchUpdatedAt = useCallback((key: string) => {
    setUpdatedMap(persistUpdatedAt(key));
  }, []);

  const collator = useMemo(() => new Intl.Collator(undefined), []);

  const visibleClients = useMemo<MCPClientInfo[]>(() => {
    const q = searchTerm.trim().toLowerCase();
    let list = clients;

    if (q) {
      list = list.filter((c) => {
        const name = (c.name || "").toLowerCase();
        return name.includes(q) || c.key.toLowerCase().includes(q);
      });
    }

    const sorted = [...list];
    const dir = sortOrder === "asc" ? 1 : -1;
    if (sortMode === "name") {
      sorted.sort(
        (a, b) => dir * collator.compare(a.name || a.key, b.name || b.key),
      );
    } else if (sortMode === "updated") {
      sorted.sort((a, b) => {
        const ua = updatedMap[a.key] ?? 0;
        const ub = updatedMap[b.key] ?? 0;
        if (ua === ub) {
          return dir * collator.compare(a.name || a.key, b.name || b.key);
        }
        return dir * (ua - ub);
      });
    }
    return sorted;
  }, [clients, searchTerm, sortMode, sortOrder, updatedMap, collator]);

  const handleUpdate = useCallback(
    async (key: string, updates: Parameters<typeof updateClient>[1]) => {
      const ok = await updateClient(key, updates);
      if (ok) touchUpdatedAt(key);
      return ok;
    },
    [updateClient, touchUpdatedAt],
  );
  const [newClientJson, setNewClientJson] = useState(`{
  "mcpServers": {
    "example-client": {
      "command": "npx",
      "args": ["-y", "@example/mcp-server"],
      "env": {
        "API_KEY": "<YOUR_API_KEY>"
      }
    }
  }
}`);
  const [createTab, setCreateTab] = useState<CreateTab>("json");
  const [createForm, setCreateForm] = useState<CreateForm>({
    ...defaultCreateForm,
  });

  const resetCreateModal = useCallback(() => {
    setNewClientJson(`{
  "mcpServers": {
    "example-client": {
      "command": "npx",
      "args": ["-y", "@example/mcp-server"],
      "env": {
        "API_KEY": "<YOUR_API_KEY>"
      }
    }
  }
}`);
    setCreateForm({ ...defaultCreateForm });
    setCreateTab("json");
  }, []);

  const closeCreateModal = useCallback(() => {
    setCreateModalOpen(false);
    resetCreateModal();
  }, [resetCreateModal]);

  const updateCreateForm = useCallback(
    <K extends keyof CreateForm>(key: K, value: CreateForm[K]) => {
      setCreateForm((prev) => ({ ...prev, [key]: value }));
    },
    [],
  );

  const handleCreateClientFromJson = async () => {
    try {
      const parsed: unknown = JSON.parse(newClientJson);
      if (!isRecord(parsed)) {
        alert("Invalid JSON format");
        return;
      }

      const clientsToCreate: Array<{
        key: string;
        data: ReturnType<typeof normalizeClientData>;
      }> = [];

      if (isRecord(parsed.mcpServers)) {
        Object.entries(parsed.mcpServers).forEach(([key, data]) => {
          if (isRecord(data)) {
            clientsToCreate.push({ key, data: normalizeClientData(key, data) });
          }
        });
      } else if (
        typeof parsed.key === "string" &&
        (parsed.command || parsed.url || parsed.baseUrl)
      ) {
        const { key, ...clientData } = parsed;
        clientsToCreate.push({
          key,
          data: normalizeClientData(key, clientData),
        });
      } else {
        Object.entries(parsed).forEach(([key, data]) => {
          if (isRecord(data) && (data.command || data.url || data.baseUrl)) {
            clientsToCreate.push({ key, data: normalizeClientData(key, data) });
          }
        });
      }

      let allSuccess = true;
      for (const { key, data } of clientsToCreate) {
        const success = await createClient(key, data);
        if (!success) allSuccess = false;
        else touchUpdatedAt(key);
      }

      if (allSuccess) {
        closeCreateModal();
      }
    } catch {
      alert("Invalid JSON format");
    }
  };

  const handleCreateClientFromForm = async () => {
    const key = createForm.key.trim();
    const name = createForm.name.trim();
    if (!key) {
      toast.error(t("mcp.form.keyRequired"));
      return;
    }
    if (!name) {
      toast.error(t("mcp.form.nameRequired"));
      return;
    }

    const isHttp =
      createForm.transport === "streamable_http" ||
      createForm.transport === "sse";
    if (isHttp && !createForm.url.trim()) {
      toast.error(t("mcp.form.urlRequired"));
      return;
    }
    if (createForm.transport === "stdio" && !createForm.command.trim()) {
      toast.error(t("mcp.form.commandRequired"));
      return;
    }

    const data = {
      name,
      description: createForm.description.trim(),
      enabled: true,
      transport: createForm.transport,
      url: isHttp ? createForm.url.trim() : "",
      headers: isHttp ? parseKeyValueLines(createForm.headers) : {},
      command:
        createForm.transport === "stdio" ? createForm.command.trim() : "",
      args: createForm.transport === "stdio" ? parseArgs(createForm.args) : [],
      env:
        createForm.transport === "stdio"
          ? parseKeyValueLines(createForm.env)
          : {},
      cwd: createForm.transport === "stdio" ? createForm.cwd.trim() : "",
    };

    const success = await createClient(key, data);
    if (success) {
      touchUpdatedAt(key);
      closeCreateModal();
    }
  };

  const isCreateHttpTransport =
    createForm.transport === "streamable_http" ||
    createForm.transport === "sse";

  const handleBroadcastClick = () => {
    setBroadcastOpen(true);
  };

  const sharedMcpMoreMenuItems: MenuProps["items"] = [
    {
      key: "broadcast",
      icon: <Send />,
      label: t("sharedMcp.broadcastToAgents"),
      onClick: handleBroadcastClick,
      disabled: clients.length === 0,
    },
    {
      key: "batchDelete",
      icon: <Trash2 />,
      label: t("common.batchDelete"),
      onClick: enterSelectMode,
      disabled: clients.length === 0,
    },
  ];
  const headerMoreMenuItems: MenuProps["items"] = [
    ...sharedMcpMoreMenuItems,
    { type: "divider" },
    {
      key: "refresh",
      icon: <RefreshCw size={14} className={loading ? "icon-spin" : ""} />,
      label: t("common.refreshPage"),
      onClick: loadClients,
      disabled: loading,
    },
  ];

  const toggleSelect = (key: string) => {
    setSelectedKeys((prev) =>
      prev.includes(key) ? prev.filter((k) => k !== key) : [...prev, key],
    );
  };

  const headerExtra: ReactNode = selectMode ? (
    <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
      <span style={{ color: "var(--text-2)", fontSize: 13 }}>
        {t("common.selectedN", { count: deleteSelectedKeys.size })}
      </span>
      <Btn variant="outline" onClick={exitSelectMode}>
        {t("common.cancel")}
      </Btn>
      <Btn
        variant="danger"
        disabled={deleteSelectedKeys.size === 0}
        onClick={confirmBatchDelete}
      >
        {t("common.delete")}
      </Btn>
    </div>
  ) : (
    <div style={{ display: "flex", gap: 8 }}>
      <Tooltip title={t("sharedMcp.create")}>
        <Btn
          variant="filled"
          icon={<Plus />}
          onClick={() => setCreateModalOpen(true)}
          aria-label={t("sharedMcp.create")}
          title={t("sharedMcp.create")}
        />
      </Tooltip>
      <Dropdown
        menu={{
          items: headerMoreMenuItems,
        }}
        trigger={["click"]}
      >
        <Btn variant="filled" icon={<MoreVertical />} />
      </Dropdown>
    </div>
  );

  return (
    <Page>
      {embedded && headerExtraContainer
        ? createPortal(headerExtra, headerExtraContainer)
        : null}
      {!embedded && (
        <PageHeader
          items={[{ title: t("nav.sharedMcp") }]}
          onRefresh={loadClients}
          refreshing={loading}
          showMoreMenu={!selectMode}
          moreMenuItems={sharedMcpMoreMenuItems}
          extra={
            selectMode ? (
              headerExtra
            ) : (
              <div style={{ display: "flex", gap: 8 }}>
                <Tooltip title={t("sharedMcp.create")}>
                  <Btn
                    variant="filled"
                    icon={<Plus />}
                    onClick={() => setCreateModalOpen(true)}
                    aria-label={t("sharedMcp.create")}
                    title={t("sharedMcp.create")}
                  />
                </Tooltip>
              </div>
            )
          }
        />
      )}

      <PageBody>
        {loading ? (
          <Skeleton variant="card" count={6} />
        ) : clients.length === 0 ? (
          <EmptyState description={t("sharedMcp.emptyState")} />
        ) : (
          <>
            <div className={styles.filterRow}>
              <div className={styles.filterBar}>
                <TextInput
                  placeholder={t("sharedMcp.searchPlaceholder")}
                  value={searchTerm}
                  onChange={(e) => setSearchTerm(e.target.value)}
                  className={styles.filterInput}
                />
                <Dropdown
                  menu={{
                    items: [
                      {
                        key: "name-asc",
                        label: t("sharedMcp.sortByName") + " A→Z",
                      },
                      {
                        key: "name-desc",
                        label: t("sharedMcp.sortByName") + " Z→A",
                      },
                      {
                        key: "updated-desc",
                        label: t("sharedMcp.sortByLastUpdated") + " ↓",
                      },
                      {
                        key: "updated-asc",
                        label: t("sharedMcp.sortByLastUpdated") + " ↑",
                      },
                    ],
                    selectedKeys: [`${sortMode}-${sortOrder}`],
                    onClick: ({ key }: { key: string }) => {
                      const idx = key.lastIndexOf("-");
                      updateSortMode(key.slice(0, idx) as SortMode);
                      updateSortOrder(key.slice(idx + 1) as SortOrder);
                    },
                  }}
                  trigger={["click"]}
                >
                  <Btn icon={<ArrowUpNarrowWide />} />
                </Dropdown>
              </div>
            </div>

            {visibleClients.length === 0 ? (
              <EmptyState description={t("sharedMcp.noMatch")} />
            ) : (
              <div className={styles.mcpGrid}>
                {visibleClients.map((client) => (
                  <SharedMCPCard
                    key={client.key}
                    client={client}
                    onDelete={deleteClient}
                    onUpdate={handleUpdate}
                    isHovered={hoverKey === client.key}
                    onMouseEnter={() => setHoverKey(client.key)}
                    onMouseLeave={() => setHoverKey(null)}
                    selectable={selectMode}
                    selected={
                      selectMode
                        ? deleteSelectedKeys.has(client.key)
                        : selectedKeys.includes(client.key)
                    }
                    onSelect={(c) =>
                      selectMode
                        ? toggleDeleteSelect(c.key)
                        : toggleSelect(c.key)
                    }
                  />
                ))}
              </div>
            )}
          </>
        )}
      </PageBody>

      <Modal
        className="modal-fullscreen-mobile"
        title={t("sharedMcp.create")}
        open={createModalOpen}
        onClose={closeCreateModal}
        footer={
          <div className={styles.modalFooter}>
            <Btn variant="outline" onClick={closeCreateModal}>
              {t("common.cancel")}
            </Btn>
            <Btn
              variant="filled"
              onClick={
                createTab === "json"
                  ? handleCreateClientFromJson
                  : handleCreateClientFromForm
              }
            >
              {t("common.create")}
            </Btn>
          </div>
        }
        width={800}
      >
        <Tabs
          activeKey={createTab}
          onChange={(key) => setCreateTab(key as CreateTab)}
          className={styles.createTabs}
          items={[
            {
              key: "json",
              label: t("mcp.tab.json"),
              children: (
                <div className={styles.createTabPanel}>
                  <div className={styles.importHint}>
                    <p className={styles.importHintTitle}>
                      {t("mcp.formatSupport")}:
                    </p>
                    <ul className={styles.importHintList}>
                      <li>
                        {t("mcp.standardFormat")}:{" "}
                        <code>{`{ "mcpServers": { "key": {...} } }`}</code>
                      </li>
                      <li>
                        {t("mcp.directFormat")}:{" "}
                        <code>{`{ "key": {...} }`}</code>
                      </li>
                      <li>
                        {t("mcp.singleFormat")}:{" "}
                        <code>{`{ "key": "...", "name": "...", "command": "..." }`}</code>
                      </li>
                    </ul>
                  </div>
                  <TextArea
                    value={newClientJson}
                    onChange={(e: React.ChangeEvent<HTMLTextAreaElement>) =>
                      setNewClientJson(e.target.value)
                    }
                    rows={20}
                    className={styles.jsonTextArea}
                  />
                </div>
              ),
            },
            {
              key: "form",
              label: t("mcp.tab.form"),
              children: (
                <div className={styles.createForm}>
                  <div className={styles.createFormGrid}>
                    <FormField label={t("mcp.form.key")} required>
                      <TextInput
                        placeholder={t("mcp.form.keyPlaceholder")}
                        value={createForm.key}
                        onChange={(e) =>
                          updateCreateForm("key", e.target.value)
                        }
                      />
                    </FormField>
                    <FormField label={t("mcp.form.name")} required>
                      <TextInput
                        placeholder={t("mcp.form.namePlaceholder")}
                        value={createForm.name}
                        onChange={(e) =>
                          updateCreateForm("name", e.target.value)
                        }
                      />
                    </FormField>
                  </div>

                  <FormField label={t("mcp.form.transport")}>
                    <Select
                      value={createForm.transport}
                      onChange={(value) =>
                        updateCreateForm("transport", value as MCPTransport)
                      }
                      options={[
                        {
                          label: "Streamable HTTP",
                          value: "streamable_http",
                        },
                        { label: "SSE", value: "sse" },
                        { label: "Stdio", value: "stdio" },
                      ]}
                    />
                  </FormField>

                  {isCreateHttpTransport ? (
                    <>
                      <FormField label={t("mcp.form.url")} required>
                        <TextInput
                          placeholder="https://mcp.example.com/mcp"
                          value={createForm.url}
                          onChange={(e) =>
                            updateCreateForm("url", e.target.value)
                          }
                        />
                      </FormField>
                      <FormField label={t("mcp.form.headers")}>
                        <TextArea
                          placeholder={t("mcp.form.headersPlaceholder")}
                          value={createForm.headers}
                          onChange={(e) =>
                            updateCreateForm("headers", e.target.value)
                          }
                          rows={4}
                        />
                      </FormField>
                    </>
                  ) : (
                    <>
                      <FormField label={t("mcp.form.command")} required>
                        <TextInput
                          placeholder="npx"
                          value={createForm.command}
                          onChange={(e) =>
                            updateCreateForm("command", e.target.value)
                          }
                        />
                      </FormField>
                      <FormField label={t("mcp.form.args")}>
                        <TextInput
                          placeholder="-y @example/mcp-server"
                          value={createForm.args}
                          onChange={(e) =>
                            updateCreateForm("args", e.target.value)
                          }
                        />
                      </FormField>
                      <FormField label={t("mcp.form.env")}>
                        <TextArea
                          placeholder={t("mcp.form.envPlaceholder")}
                          value={createForm.env}
                          onChange={(e) =>
                            updateCreateForm("env", e.target.value)
                          }
                          rows={4}
                        />
                      </FormField>
                      <FormField label={t("mcp.form.cwd")}>
                        <TextInput
                          placeholder="/Users/me/project"
                          value={createForm.cwd}
                          onChange={(e) =>
                            updateCreateForm("cwd", e.target.value)
                          }
                        />
                      </FormField>
                    </>
                  )}

                  <FormField label={t("mcp.form.description")}>
                    <TextInput
                      placeholder={t("mcp.form.descriptionPlaceholder")}
                      value={createForm.description}
                      onChange={(e) =>
                        updateCreateForm("description", e.target.value)
                      }
                    />
                  </FormField>
                </div>
              ),
            },
          ]}
        />
      </Modal>

      <BroadcastModal
        open={broadcastOpen}
        onClose={() => setBroadcastOpen(false)}
        clients={clients}
        defaultSelectedKeys={selectedKeys}
      />
    </Page>
  );
}
