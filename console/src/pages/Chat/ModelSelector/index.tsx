import { rememberConversationModel } from "../conversationLifecycle";
import { useCallback, useEffect, useRef, useState } from "react";
import { useLocation } from "react-router-dom";
import { useTranslation } from "react-i18next";
import {
  agentConversationTarget,
  conversationApi,
  type ConversationApiTarget,
} from "../../../api/modules/conversation";
import { providerApi } from "../../../api/modules/provider";
import type { ModelInfo, ModelSlot, ProviderInfo } from "../../../api/types";
import { useAgentStore } from "../../../stores/agentStore";
import {
  isLocalOrLanScopedProvider,
  isProviderReadyForModelSelection,
} from "../../../utils/providerReadiness";
import {
  ArrowLeft,
  Check,
  ChevronDown,
  ChevronRight,
  Loader2,
} from "lucide-react";
import { Spinner, toast } from "@/ui";
import styles from "./index.module.less";

interface EligibleProvider {
  id: string;
  name: string;
  is_free_tier?: boolean;
  require_api_key?: boolean;
  supports_oauth?: boolean;
  ready: boolean;
  models: ModelInfo[];
  thinking_param_style?: string | null;
  reasoning_effort_options?: string[];
  thinking_budget_range?: [number, number] | null;
}

type SelectorTab = "pro" | "free";
type PanelView = "summary" | "models";
type ThinkingStrength = "off" | "auto" | "low" | "medium" | "high";

const THINKING_STRENGTH_OPTIONS: ThinkingStrength[] = [
  "off",
  "auto",
  "low",
  "medium",
  "high",
];

interface ModelSelectorProps {
  chatId?: string;
  chatModel?: ModelSlot | null;
  onChatModelChange?: (model: ModelSlot) => void;
  target?: ConversationApiTarget;
  draftSelection?: boolean;
  disabled?: boolean;
}

function currentThinkingStrength(
  model: ModelSlot | null | undefined,
): ThinkingStrength {
  if (model?.thinking_strength) return model.thinking_strength;
  if (model?.thinking_enabled === false) return "off";
  if (
    model?.reasoning_effort === "low" ||
    model?.reasoning_effort === "medium" ||
    model?.reasoning_effort === "high"
  ) {
    return model.reasoning_effort;
  }
  return "auto";
}

function modelWithThinkingStrength(
  model: ModelSlot,
  thinkingStrength: ThinkingStrength,
): ModelSlot {
  return {
    ...model,
    thinking_strength: thinkingStrength,
    thinking_enabled: null,
    thinking_budget: null,
    reasoning_effort: null,
  };
}

function supportsThinkingStrength(
  model: ModelSlot | null | undefined,
  strength: ThinkingStrength,
): boolean {
  if (strength === "off" || strength === "auto") return true;
  const options = model?.reasoning_effort_options ?? [];
  if (model?.thinking_param_style !== "effort" || options.length === 0) {
    return true;
  }
  if (strength === "high") {
    return options.includes("high") || options.includes("max");
  }
  return options.includes(strength);
}

function draftModelSlot(
  provider: EligibleProvider,
  model: ModelInfo,
): ModelSlot {
  const slot: ModelSlot = {
    provider_id: provider.id,
    model: model.id,
  };
  const thinkingParamStyle =
    model.thinking_param_style ?? provider.thinking_param_style;
  const reasoningEffortOptions =
    model.reasoning_effort_options ?? provider.reasoning_effort_options;
  const thinkingBudgetRange =
    model.thinking_budget_range ?? provider.thinking_budget_range;

  if (model.thinking_enabled !== undefined) {
    slot.thinking_enabled = model.thinking_enabled;
  }
  if (model.thinking_budget !== undefined) {
    slot.thinking_budget = model.thinking_budget;
  }
  if (model.reasoning_effort !== undefined) {
    slot.reasoning_effort = model.reasoning_effort;
  }
  if (thinkingParamStyle !== undefined) {
    slot.thinking_param_style = thinkingParamStyle;
  }
  if (reasoningEffortOptions !== undefined) {
    slot.reasoning_effort_options = reasoningEffortOptions;
  }
  if (thinkingBudgetRange !== undefined) {
    slot.thinking_budget_range = thinkingBudgetRange;
  }
  return slot;
}

export default function ModelSelector({
  chatId,
  chatModel,
  onChatModelChange,
  target,
  draftSelection = false,
  disabled = false,
}: ModelSelectorProps) {
  const { t } = useTranslation();
  const [providers, setProviders] = useState<ProviderInfo[]>([]);
  const [templates, setTemplates] = useState<ProviderInfo[]>([]);
  const [defaultModel, setDefaultModel] = useState<ModelSlot | null>(null);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [open, setOpen] = useState(false);
  const [panelView, setPanelView] = useState<PanelView>("summary");
  const [activeTab, setActiveTab] = useState<SelectorTab>("free");
  const [expandedProviderId, setExpandedProviderId] = useState<string | null>(
    null,
  );
  const rootRef = useRef<HTMLDivElement>(null);
  const savingRef = useRef(false);
  const fetchSeqRef = useRef(0);
  const tabManuallySelectedRef = useRef(false);
  const location = useLocation();
  const { selectedAgent } = useAgentStore();
  const conversationTarget = target ?? agentConversationTarget(selectedAgent);

  const fetchData = useCallback(async () => {
    const fetchSeq = ++fetchSeqRef.current;
    setLoading(true);
    try {
      const [provData, templateData, activeData] = await Promise.all([
        providerApi.listProviders(),
        providerApi.listProviderTemplates().catch(() => []),
        chatId
          ? Promise.resolve(null)
          : providerApi.getActiveModels({
              scope: "effective",
              agent_id: selectedAgent,
            }),
      ]);
      if (fetchSeq !== fetchSeqRef.current) return;
      if (Array.isArray(provData)) setProviders(provData);
      if (Array.isArray(templateData)) setTemplates(templateData);
      if (!chatId) {
        setDefaultModel(activeData?.default_llm ?? null);
      }
    } catch (err) {
      if (fetchSeq !== fetchSeqRef.current) return;
      console.error("ModelSelector: failed to load data", err);
    } finally {
      if (fetchSeq === fetchSeqRef.current) {
        setLoading(false);
      }
    }
  }, [chatId, selectedAgent]);

  useEffect(() => {
    fetchData();
  }, [fetchData]);

  const prevPathRef = useRef(location.pathname);
  useEffect(() => {
    const prev = prevPathRef.current;
    const curr = location.pathname;
    prevPathRef.current = curr;
    const isChatPath = (path: string) =>
      /^\/agents\/[^/]+\/chats\/[^/]+$/.test(path) ||
      /^\/projects\/[^/]+\/chats\/[^/]+$/.test(path);
    const comingToChat = isChatPath(curr) && !isChatPath(prev);
    if (comingToChat) {
      fetchData();
    }
  }, [location.pathname, fetchData]);

  const allProviders: EligibleProvider[] = providers.map((provider) => ({
    id: provider.id,
    name: provider.name,
    is_free_tier: provider.is_free_tier,
    require_api_key: provider.require_api_key,
    supports_oauth: provider.supports_oauth,
    ready: isProviderReadyForModelSelection(provider),
    models: [...(provider.models ?? []), ...(provider.extra_models ?? [])],
    thinking_param_style: provider.thinking_param_style,
    reasoning_effort_options: provider.reasoning_effort_options,
    thinking_budget_range: provider.thinking_budget_range,
  }));
  const eligibleProviders = allProviders.filter((provider) => provider.ready);
  const freeProviders = allProviders
    .filter((provider) => provider.is_free_tier && provider.ready)
    .map((provider) => {
      const freeModels = provider.models.filter((model) => model.is_free);
      return {
        ...provider,
        models:
          freeModels.length > 0 || provider.require_api_key !== false
            ? freeModels
            : provider.models,
      };
    })
    .filter((provider) => provider.models.length > 0);
  const proProviders = eligibleProviders
    .map((provider) => ({
      ...provider,
      models: provider.models.filter((model) => !model.is_free),
    }))
    .filter((provider) => provider.models.length > 0);
  const hasMoreFreeProviders = templates.some(
    (template) =>
      template.is_free_tier &&
      template.require_api_key !== false &&
      !providers.some(
        (provider) =>
          provider.id === template.id &&
          isProviderReadyForModelSelection(provider),
      ),
  );

  const activeModel = draftSelection
    ? chatModel ?? defaultModel
    : chatId
    ? chatModel
    : defaultModel;
  useEffect(() => {
    if (chatModel) rememberConversationModel(chatModel);
  }, [chatModel]);

  const activeProviderId = activeModel?.provider_id;
  const activeModelId = activeModel?.model;
  const thinkingStrength = currentThinkingStrength(activeModel);
  const thinkingLabel = t(`chat.thinkingStrengthOption.${thinkingStrength}`, {
    defaultValue: thinkingStrength,
  });
  const canEditThinking = Boolean(
    !disabled &&
      activeModel?.provider_id &&
      activeModel?.model &&
      (chatId || draftSelection),
  );

  const activeProvider = eligibleProviders.find(
    (provider) => provider.id === activeProviderId,
  );
  const activeModelName = (() => {
    if (!activeProviderId || !activeModelId) {
      return t("modelSelector.selectModel");
    }
    const model = activeProvider?.models.find(
      (candidate) => candidate.id === activeModelId,
    );
    return model?.name || model?.id || activeModelId;
  })();

  useEffect(() => {
    if (!open || tabManuallySelectedRef.current) return;
    const model = activeProvider?.models.find(
      (candidate) => candidate.id === activeModelId,
    );
    if (model?.is_free) {
      setActiveTab("free");
    } else if (activeProvider && proProviders.length > 0) {
      setActiveTab("pro");
    } else if (freeProviders.length > 0 || hasMoreFreeProviders) {
      setActiveTab("free");
    } else {
      setActiveTab("pro");
    }
  }, [
    open,
    activeModelId,
    activeProvider,
    freeProviders.length,
    hasMoreFreeProviders,
    proProviders.length,
  ]);

  const showLocalScopeHint = useCallback(
    (providerId: string) => {
      const selectedProvider = providers.find(
        (provider) => provider.id === providerId,
      );
      if (selectedProvider && isLocalOrLanScopedProvider(selectedProvider)) {
        toast.info(t("modelSelector.localProviderHint"), 2500);
      }
    },
    [providers, t],
  );

  const handleOpenChange = useCallback(
    (next: boolean) => {
      if (disabled) {
        setOpen(false);
        return;
      }
      if (!next) {
        tabManuallySelectedRef.current = false;
        setPanelView("summary");
        setExpandedProviderId(null);
      }
      setOpen(next);
      if (next) {
        fetchData();
      }
    },
    [disabled, fetchData],
  );

  useEffect(() => {
    if (!open) return;
    const handlePointerDown = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) {
        handleOpenChange(false);
      }
    };
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        handleOpenChange(false);
      }
    };
    document.addEventListener("pointerdown", handlePointerDown);
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("pointerdown", handlePointerDown);
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [handleOpenChange, open]);

  const openModelList = () => {
    const providersForTab = activeTab === "free" ? freeProviders : proProviders;
    const activeVisible = providersForTab.some(
      (provider) => provider.id === activeProviderId,
    );
    setExpandedProviderId(
      activeVisible ? activeProviderId ?? null : providersForTab[0]?.id ?? null,
    );
    setPanelView("models");
  };

  const handleSelect = async (provider: EligibleProvider, model: ModelInfo) => {
    if (savingRef.current) return;
    if (provider.id === activeProviderId && model.id === activeModelId) {
      showLocalScopeHint(provider.id);
      setPanelView("summary");
      return;
    }

    savingRef.current = true;
    setSaving(true);
    try {
      if (draftSelection) {
        const nextModel = draftModelSlot(provider, model);
        setDefaultModel(nextModel);
        onChatModelChange?.(nextModel);
      } else if (chatId) {
        const updated = await conversationApi.updateModel(
          conversationTarget,
          chatId,
          { provider_id: provider.id, model: model.id },
        );
        onChatModelChange?.(
          updated.model ?? {
            provider_id: provider.id,
            model: model.id,
          },
        );
      } else {
        await providerApi.setActiveLlm({
          provider_id: provider.id,
          model: model.id,
          scope: "agent",
          agent_id: selectedAgent,
        });
        setDefaultModel({
          provider_id: provider.id,
          model: model.id,
        });
      }
      showLocalScopeHint(provider.id);
      setPanelView("summary");
      window.dispatchEvent(new CustomEvent("model-switched"));
    } catch (err) {
      const message =
        err instanceof Error ? err.message : t("modelSelector.switchFailed");
      toast.error(message);
    } finally {
      setSaving(false);
      savingRef.current = false;
    }
  };

  const handleThinkingSelect = async (value: ThinkingStrength) => {
    if (
      savingRef.current ||
      !canEditThinking ||
      !activeModel ||
      value === thinkingStrength
    ) {
      return;
    }
    const nextModel = modelWithThinkingStrength(activeModel, value);
    savingRef.current = true;
    setSaving(true);
    try {
      if (draftSelection) {
        setDefaultModel(nextModel);
        onChatModelChange?.(nextModel);
      } else if (chatId) {
        const updated = await conversationApi.updateModel(
          conversationTarget,
          chatId,
          nextModel,
        );
        onChatModelChange?.(updated.model ?? nextModel);
      }
    } catch (error) {
      toast.error(
        error instanceof Error
          ? error.message
          : t("chat.reasoningSaveFailed", {
              defaultValue: "Failed to update thinking strength",
            }),
      );
    } finally {
      setSaving(false);
      savingRef.current = false;
    }
  };

  const renderProviderGroup = (provider: EligibleProvider) => {
    const isProviderExpanded = provider.id === expandedProviderId;
    return (
      <div key={provider.id} className={styles.providerGroup}>
        <button
          type="button"
          className={styles.providerItem}
          aria-expanded={isProviderExpanded}
          onClick={() =>
            setExpandedProviderId((current) =>
              current === provider.id ? null : provider.id,
            )
          }
        >
          <span className={styles.providerMain}>
            <span className={styles.providerName}>{provider.name}</span>
            {provider.is_free_tier && (
              <span className={styles.freeBadge}>{t("models.free")}</span>
            )}
          </span>
          <ChevronRight
            className={[
              styles.providerArrow,
              isProviderExpanded ? styles.providerArrowOpen : "",
            ]
              .filter(Boolean)
              .join(" ")}
          />
        </button>
        {isProviderExpanded && (
          <div className={styles.modelList}>
            {provider.models.map((model) => {
              const isActive =
                provider.id === activeProviderId && model.id === activeModelId;
              return (
                <button
                  key={model.id}
                  type="button"
                  className={[
                    styles.modelItem,
                    isActive ? styles.modelItemActive : "",
                  ]
                    .filter(Boolean)
                    .join(" ")}
                  onClick={() => void handleSelect(provider, model)}
                >
                  <span className={styles.modelName}>
                    {model.name || model.id}
                  </span>
                  {model.is_free && (
                    <span className={styles.freeBadge}>{t("models.free")}</span>
                  )}
                  {isActive && <Check className={styles.checkIcon} />}
                </button>
              );
            })}
          </div>
        )}
      </div>
    );
  };

  const summaryPanel = (
    <div className={styles.panel}>
      <div className={styles.panelHeader}>
        <div>
          <div className={styles.panelTitle}>
            {t("modelSelector.modelAndThinking")}
          </div>
          <div className={styles.panelSubtitle}>
            {t("modelSelector.configureForChat")}
          </div>
        </div>
        {saving && <Loader2 className={styles.spin} />}
      </div>

      <button
        type="button"
        className={styles.modelSummary}
        aria-label={`${t("modelSelector.model")}: ${activeModelName}`}
        onClick={openModelList}
      >
        <span className={styles.modelSummaryText}>
          <span className={styles.sectionLabel}>
            {t("modelSelector.model")}
          </span>
          <strong>{activeModelName}</strong>
          {activeProvider?.name && <small>{activeProvider.name}</small>}
        </span>
        <ChevronRight />
      </button>

      <div className={styles.thinkingSection}>
        <div className={styles.sectionLabel}>{t("chat.reasoningControl")}</div>
        {!activeModel?.provider_id || !activeModel?.model ? (
          <div className={styles.emptyTip}>
            {t("chat.reasoningRequiresModel")}
          </div>
        ) : (
          <div className={styles.thinkingOptions}>
            {THINKING_STRENGTH_OPTIONS.map((value) => (
              <button
                key={value}
                type="button"
                className={
                  thinkingStrength === value ? styles.thinkingOptionActive : ""
                }
                disabled={
                  saving ||
                  !canEditThinking ||
                  !supportsThinkingStrength(activeModel, value)
                }
                onClick={() => void handleThinkingSelect(value)}
              >
                {t(`chat.thinkingStrengthOption.${value}`, {
                  defaultValue: value,
                })}
              </button>
            ))}
          </div>
        )}
      </div>
    </div>
  );

  const visibleProviders = activeTab === "free" ? freeProviders : proProviders;
  const modelsPanel = (
    <div className={styles.panel}>
      <div className={styles.modelPanelHeader}>
        <button
          type="button"
          className={styles.backButton}
          aria-label={t("common.back")}
          title={t("common.back")}
          onClick={() => setPanelView("summary")}
        >
          <ArrowLeft />
        </button>
        <div className={styles.panelTitle}>
          {t("modelSelector.chooseModel")}
        </div>
      </div>
      <div className={styles.selectorTabs}>
        <button
          type="button"
          className={activeTab === "pro" ? styles.selectorTabActive : ""}
          onClick={() => {
            tabManuallySelectedRef.current = true;
            setExpandedProviderId(proProviders[0]?.id ?? null);
            setActiveTab("pro");
          }}
        >
          {t("models.pro")}
        </button>
        <button
          type="button"
          className={activeTab === "free" ? styles.selectorTabActive : ""}
          onClick={() => {
            tabManuallySelectedRef.current = true;
            setExpandedProviderId(freeProviders[0]?.id ?? null);
            setActiveTab("free");
          }}
        >
          {t("models.free")}
        </button>
      </div>
      <div className={styles.modelPanelBody}>
        {loading ? (
          <div className={styles.spinWrapper}>
            <Spinner size="sm" />
          </div>
        ) : visibleProviders.length === 0 &&
          !(activeTab === "free" && hasMoreFreeProviders) ? (
          <div className={styles.emptyTip}>
            {activeTab === "pro"
              ? t("models.noProModelsConfigured")
              : t("modelSelector.noConfiguredModels")}
          </div>
        ) : (
          <>
            {activeTab === "free" && (
              <div className={styles.freeWarning}>
                {t("models.freeModelWarningShort")}
              </div>
            )}
            {visibleProviders.map(renderProviderGroup)}
            {activeTab === "free" && hasMoreFreeProviders && (
              <div className={styles.freeMoreHint}>
                {t("models.moreFreeProvidersSettingsHint")}
              </div>
            )}
          </>
        )}
      </div>
    </div>
  );

  return (
    <div ref={rootRef} className={styles.root}>
      <button
        type="button"
        className={[
          "rua-model-selector-trigger",
          styles.trigger,
          open ? styles.triggerActive : "",
        ]
          .filter(Boolean)
          .join(" ")}
        aria-label={`${t(
          "modelSelector.modelAndThinking",
        )}: ${activeModelName}, ${thinkingLabel}`}
        aria-expanded={open}
        disabled={disabled}
        onClick={() => handleOpenChange(!open)}
      >
        {saving && <Loader2 className={styles.spin} />}
        <span
          className={[
            "rua-model-selector-trigger-name",
            styles.triggerName,
          ].join(" ")}
        >
          {activeModelName}
        </span>
        <span className={styles.triggerSeparator}>·</span>
        <span className={styles.triggerThinking}>{thinkingLabel}</span>
        <ChevronDown
          className={[styles.triggerArrow, open ? styles.triggerArrowOpen : ""]
            .filter(Boolean)
            .join(" ")}
        />
      </button>
      {open && (
        <div
          className={styles.composerOverlay}
          data-rua-model-thinking-panel
          role="dialog"
          aria-label={t("modelSelector.modelAndThinking")}
        >
          {panelView === "summary" ? summaryPanel : modelsPanel}
        </div>
      )}
    </div>
  );
}
