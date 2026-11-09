import { useEffect, useState } from "react";
import { GripVertical, KeyRound } from "lucide-react";
import { useTranslation } from "react-i18next";
import { agentsApi } from "../../../api/modules/agents";
import type { ToolConfig } from "../../../api/modules/tools";
import { useAgentStore } from "../../../stores/agentStore";
import { Btn, Modal, TextInput, Toggle, toast } from "@/ui";
import styles from "./index.module.less";

const TOOL_NAME = "web_search";

type ProviderId = "brave" | "serper" | "tavily" | "exa";

interface ProviderDefinition {
  id: ProviderId;
  label: string;
  mode: string;
  keyMode: "none" | "optional" | "required";
  note: string;
}

interface ProviderConfig extends ProviderDefinition {
  enabled: boolean;
  api_key?: string;
}

interface WebSearchConfig {
  mode: "ordered_fallback";
  providers: ProviderConfig[];
}

interface Props {
  open: boolean;
  onClose: () => void;
}

const PROVIDERS: ProviderDefinition[] = [
  {
    id: "tavily",
    label: "Tavily",
    mode: "json",
    keyMode: "optional",
    note: "Uses keyless agent search by default; add your API key for your own quota.",
  },
  {
    id: "exa",
    label: "Exa",
    mode: "neural",
    keyMode: "optional",
    note: "Uses Exa's hosted MCP without a key; add your API key for direct API access.",
  },
  {
    id: "brave",
    label: "Brave Search API",
    mode: "json",
    keyMode: "required",
    note: "Production search API with broad web coverage.",
  },
  {
    id: "serper",
    label: "Serper Google",
    mode: "json",
    keyMode: "required",
    note: "Google SERP API; strong general technical search.",
  },
];

const DEFAULT_ENABLED = new Set<ProviderId>(["tavily", "exa"]);

function defaultConfig(): WebSearchConfig {
  return {
    mode: "ordered_fallback",
    providers: PROVIDERS.map((provider) => ({
      ...provider,
      enabled: DEFAULT_ENABLED.has(provider.id),
      api_key: "",
    })),
  };
}

function mergeConfig(raw: ToolConfig): WebSearchConfig {
  const existing = Array.isArray(raw.providers) ? raw.providers : [];
  const byId = new Map<string, Record<string, unknown>>();
  for (const item of existing) {
    if (item && typeof item === "object" && "id" in item) {
      byId.set(
        String((item as Record<string, unknown>).id),
        item as Record<string, unknown>,
      );
    }
  }

  const known = PROVIDERS.map((provider) => {
    const saved = byId.get(provider.id);
    return {
      ...provider,
      enabled:
        typeof saved?.enabled === "boolean"
          ? saved.enabled
          : DEFAULT_ENABLED.has(provider.id),
      api_key: typeof saved?.api_key === "string" ? saved.api_key : "",
    };
  });

  known.sort((a, b) => {
    const ai = existing.findIndex(
      (item) => (item as Record<string, unknown>)?.id === a.id,
    );
    const bi = existing.findIndex(
      (item) => (item as Record<string, unknown>)?.id === b.id,
    );
    return (
      (ai < 0 ? Number.MAX_SAFE_INTEGER : ai) -
      (bi < 0 ? Number.MAX_SAFE_INTEGER : bi)
    );
  });

  return { mode: "ordered_fallback", providers: known };
}

function toWire(config: WebSearchConfig): ToolConfig {
  return {
    mode: config.mode,
    providers: config.providers.map(({ id, enabled, api_key }) => ({
      id,
      enabled,
      ...(api_key ? { api_key } : {}),
    })),
  };
}

export function WebSearchConfigModal({ open, onClose }: Props) {
  const { t } = useTranslation();
  const { selectedAgent } = useAgentStore();
  const [config, setConfig] = useState<WebSearchConfig>(defaultConfig);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [dragging, setDragging] = useState<ProviderId | null>(null);

  useEffect(() => {
    if (!open) return;
    setLoading(true);
    agentsApi
      .getAgentToolConfig(selectedAgent, TOOL_NAME)
      .then((data) => setConfig(mergeConfig(data)))
      .catch((error: unknown) => {
        console.error("Failed to load web search config:", error);
        toast.error(t("tools.webSearchConfig.loadError"));
        setConfig(defaultConfig());
      })
      .finally(() => setLoading(false));
  }, [open, selectedAgent, t]);

  const updateProvider = (id: ProviderId, patch: Partial<ProviderConfig>) => {
    setConfig((prev) => ({
      ...prev,
      providers: prev.providers.map((provider) =>
        provider.id === id ? { ...provider, ...patch } : provider,
      ),
    }));
  };

  const moveProvider = (targetId: ProviderId) => {
    if (!dragging || dragging === targetId) return;
    setConfig((prev) => {
      const providers = [...prev.providers];
      const from = providers.findIndex((provider) => provider.id === dragging);
      const to = providers.findIndex((provider) => provider.id === targetId);
      if (from < 0 || to < 0) return prev;
      const [moved] = providers.splice(from, 1);
      providers.splice(to, 0, moved);
      return { ...prev, providers };
    });
  };

  const save = async () => {
    setSaving(true);
    try {
      const saved = await agentsApi.updateAgentToolConfig(
        selectedAgent,
        TOOL_NAME,
        toWire(config),
      );
      setConfig(mergeConfig(saved));
      toast.success(t("tools.webSearchConfig.saveSuccess"));
      onClose();
    } catch (error: unknown) {
      console.error("Failed to save web search config:", error);
      toast.error(t("tools.webSearchConfig.saveError"));
    } finally {
      setSaving(false);
    }
  };

  return (
    <Modal
      className={`modal-fullscreen-mobile ${styles.webSearchConfigModal}`}
      width={760}
      open={open}
      onClose={onClose}
      title={t("tools.webSearchConfig.title")}
      footer={
        <>
          <Btn variant="outline" onClick={() => setConfig(defaultConfig())}>
            {t("common.reset", { defaultValue: "Reset" })}
          </Btn>
          <div className={styles.webSearchConfigFooterActions}>
            <Btn variant="outline" onClick={onClose}>
              {t("common.cancel", { defaultValue: "Cancel" })}
            </Btn>
            <Btn onClick={save} loading={saving}>
              {t("common.save", { defaultValue: "Save" })}
            </Btn>
          </div>
        </>
      }
    >
      {loading ? (
        <div className={styles.webSearchConfigLoading}>
          {t("common.loading", { defaultValue: "Loading..." })}
        </div>
      ) : (
        <div className={styles.webSearchProviderList}>
          {config.providers.map((provider) => (
            <div
              key={provider.id}
              className={`${styles.webSearchProviderRow}${
                dragging === provider.id ? ` ${styles.dragging}` : ""
              }`}
              draggable
              onDragStart={() => setDragging(provider.id)}
              onDragEnd={() => setDragging(null)}
              onDragOver={(event) => event.preventDefault()}
              onDrop={() => moveProvider(provider.id)}
            >
              <button
                type="button"
                className={styles.webSearchDragHandle}
                aria-label={t("tools.webSearchConfig.dragProvider", {
                  defaultValue: "Drag provider",
                })}
              >
                <GripVertical size={16} />
              </button>
              <div className={styles.webSearchProviderMain}>
                <div className={styles.webSearchProviderTitle}>
                  <span>{provider.label}</span>
                  <span className={styles.webSearchProviderTag}>
                    {provider.mode}
                  </span>
                  <span
                    className={`${styles.webSearchProviderTag} ${
                      provider.keyMode === "required"
                        ? styles.keyRequired
                        : provider.keyMode === "optional"
                        ? styles.keyOptional
                        : styles.noKey
                    }`}
                  >
                    {provider.keyMode === "required"
                      ? t("tools.webSearchConfig.keyRequired")
                      : provider.keyMode === "optional"
                      ? t("tools.webSearchConfig.keyOptional")
                      : t("tools.webSearchConfig.noKey")}
                  </span>
                </div>
                <div className={styles.webSearchProviderNote}>
                  {provider.note}
                </div>
              </div>
              <div className={styles.webSearchProviderSecret}>
                <KeyRound size={14} />
                <TextInput
                  type="password"
                  value={provider.api_key || ""}
                  disabled={provider.keyMode === "none"}
                  placeholder={
                    provider.keyMode === "required"
                      ? t("tools.webSearchConfig.keyPlaceholder")
                      : provider.keyMode === "optional"
                      ? t("tools.webSearchConfig.optionalKeyPlaceholder")
                      : t("tools.webSearchConfig.noKeyPlaceholder")
                  }
                  onChange={(event) =>
                    updateProvider(provider.id, { api_key: event.target.value })
                  }
                />
              </div>
              <div className={styles.webSearchProviderToggle}>
                <Toggle
                  checked={provider.enabled}
                  onChange={(event) =>
                    updateProvider(provider.id, {
                      enabled: event.target.checked,
                    })
                  }
                />
              </div>
            </div>
          ))}
        </div>
      )}
    </Modal>
  );
}
