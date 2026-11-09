/* eslint-disable react-refresh/only-export-components */
import { Btn, EnableDisableSwitch, Slider, Tooltip } from "@/ui";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useLoaderData } from "react-router-dom";
import { Controller, useForm, FormProvider } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import {
  useAgentConfig,
  agentConfigSchema,
  type AgentConfigFormValues,
  type AgentConfigInitialData,
} from "./useAgentConfig.tsx";
import api from "../../../api";
import { agentsApi } from "../../../api/modules/agents";
import {
  ReactAgentCard,
  MemorySummaryCard,
  LlmRetryCard,
  LlmRateLimiterCard,
  EmbeddingConfigCard,
} from "./components";
import { PageHeader } from "@/components/PageHeader";
import { Page, PageBody } from "@/components/Page";
import {
  ChatOptionRow,
  ChatOptionSection,
  ChatOptionsLayout,
  RunningConfigOptions,
  type RunningConfigSectionId,
} from "@/components/RunningConfigOptions";
import Skeleton from "@/components/Skeleton";
import styles from "./index.module.less";
import { RotateCcw, Save } from "lucide-react";
import { routeAgentId } from "@/routes/routeAgent";
import type { ChatRunningConfig } from "@/api/types";

interface AgentConfigPageProps {
  initialData?: AgentConfigInitialData;
}

const SECTION_STORAGE_KEY = "rua:agent-config-section-open";
const DEFAULT_SECTIONS = {
  runtime: false,
  compaction: false,
  aggressive: false,
  token: false,
  other: false,
};

type SectionId = keyof typeof DEFAULT_SECTIONS;

function readSectionOpen(): Record<SectionId, boolean> {
  try {
    const raw = window.localStorage.getItem(SECTION_STORAGE_KEY);
    if (!raw) return { ...DEFAULT_SECTIONS };
    const parsed = JSON.parse(raw) as Record<string, unknown>;
    return Object.fromEntries(
      Object.entries(DEFAULT_SECTIONS).map(([key, fallback]) => [
        key,
        typeof parsed[key] === "boolean" ? parsed[key] : fallback,
      ]),
    ) as Record<SectionId, boolean>;
  } catch {
    return { ...DEFAULT_SECTIONS };
  }
}

export async function loader(): Promise<AgentConfigInitialData> {
  const agentId = routeAgentId();
  const [config, language, timezone] = await Promise.all([
    agentsApi.getAgentRunningConfig(agentId),
    agentsApi.getAgentLanguage(agentId),
    api.getUserTimezone(),
  ]);
  return {
    agentId,
    config,
    language: language.language,
    timezone: timezone.timezone || "UTC",
  };
}

export function RouteComponent() {
  return (
    <AgentConfigPage initialData={useLoaderData() as AgentConfigInitialData} />
  );
}

function AgentConfigPage({ initialData }: AgentConfigPageProps) {
  const { t } = useTranslation();
  const [openSections, setOpenSections] =
    useState<Record<SectionId, boolean>>(readSectionOpen);
  const methods = useForm<AgentConfigFormValues>({
    defaultValues: initialData
      ? ({
          ...initialData.config,
          auto_continue_on_text_only:
            initialData.config.auto_continue_on_text_only ?? false,
        } as AgentConfigFormValues)
      : undefined,
    resolver: zodResolver(agentConfigSchema) as never,
  });
  const {
    loading,
    saving,
    resetting,
    error,
    language,
    savingLang,
    timezone,
    savingTimezone,
    fetchConfig,
    handleSave,
    handleReset,
    handleLanguageChange,
    handleTimezoneChange,
  } = useAgentConfig(methods, initialData);

  const llmRetryEnabled = methods.watch("llm_retry_enabled") ?? true;
  const runningConfig = methods.watch();

  useEffect(() => {
    try {
      window.localStorage.setItem(
        SECTION_STORAGE_KEY,
        JSON.stringify(openSections),
      );
    } catch {
      // Section state persistence is best-effort.
    }
  }, [openSections]);

  function toggleSection(section: SectionId) {
    setOpenSections((current) => ({
      ...current,
      [section]: !current[section],
    }));
  }

  function setRunningConfigField(
    key: "max_iters" | "auto_continue_on_text_only" | "max_input_length",
    value: number | boolean,
  ) {
    methods.setValue(key, value as never, {
      shouldDirty: true,
      shouldValidate: true,
    });
  }

  function setContextConfigField(
    key: keyof AgentConfigFormValues["context_compact"],
    value: AgentConfigFormValues["context_compact"][keyof AgentConfigFormValues["context_compact"]],
  ) {
    methods.setValue(`context_compact.${key}` as never, value as never, {
      shouldDirty: true,
      shouldValidate: true,
    });
  }

  function setToolResultConfigField(
    key: keyof AgentConfigFormValues["tool_result_compact"],
    value: AgentConfigFormValues["tool_result_compact"][keyof AgentConfigFormValues["tool_result_compact"]],
  ) {
    methods.setValue(`tool_result_compact.${key}` as never, value as never, {
      shouldDirty: true,
      shouldValidate: true,
    });
  }

  function setThinkingTurnsToKeep(value: number) {
    methods.setValue(
      "context_compact.compact_with_thinking_block",
      value !== 0,
      {
        shouldDirty: true,
        shouldValidate: true,
      },
    );
    methods.setValue("context_compact.thinking_turns_to_keep", value, {
      shouldDirty: true,
      shouldValidate: true,
    });
  }

  if (loading) {
    return (
      <Page className={styles.configPage}>
        <Skeleton variant="detail" count={3} />
      </Page>
    );
  }

  if (error) {
    return (
      <Page className={styles.configPage}>
        <div className={styles.centerState}>
          <span className={styles.stateTextError}>{error}</span>
          <Btn size="sm" onClick={fetchConfig} style={{ marginTop: 12 }}>
            {t("environments.retry")}
          </Btn>
        </div>
      </Page>
    );
  }

  return (
    <Page className={styles.configPage}>
      <PageHeader
        agentScoped
        current={t("agentConfig.title")}
        onRefresh={fetchConfig}
        refreshing={saving || resetting}
        extra={
          <div style={{ display: "flex", gap: "var(--space-2)" }}>
            <Tooltip title={t("agentConfig.resetToApplicationDefaults")}>
              <Btn
                variant="outline"
                onClick={handleReset}
                loading={resetting}
                disabled={saving}
                aria-label={t("agentConfig.resetToApplicationDefaults")}
                icon={<RotateCcw />}
              />
            </Tooltip>
            <Tooltip title={t("common.save")}>
              <Btn
                variant="filled"
                onClick={handleSave}
                loading={saving}
                disabled={resetting}
                aria-label={t("common.save")}
                icon={<Save />}
              />
            </Tooltip>
          </div>
        }
      />
      <PageBody>
        <div className={styles.formContainer}>
          <FormProvider {...methods}>
            <div className={styles.form}>
              <ChatOptionsLayout>
                <RunningConfigOptions
                  value={runningConfig as unknown as ChatRunningConfig}
                  showCompactionMasterControls
                  runtimeExtras={
                    <>
                      <ReactAgentCard
                        language={language}
                        savingLang={savingLang}
                        onLanguageChange={handleLanguageChange}
                        timezone={timezone}
                        savingTimezone={savingTimezone}
                        onTimezoneChange={handleTimezoneChange}
                      />
                      <Controller
                        name="show_debug_menu"
                        control={methods.control}
                        render={({ field }) => (
                          <ChatOptionRow
                            label={t("agentConfig.showDebugMenu")}
                            tooltip={t("agentConfig.showDebugMenuTooltip")}
                          >
                            <EnableDisableSwitch
                              enabled={Boolean(field.value)}
                              onChange={field.onChange}
                            />
                          </ChatOptionRow>
                        )}
                      />
                      <Controller
                        name="tool_result_compact.retention_days"
                        control={methods.control}
                        render={({ field }) => (
                          <ChatOptionRow
                            label={t(
                              "agentConfig.toolResultCompactRetentionDays",
                            )}
                            tooltip={t(
                              "agentConfig.toolResultCompactRetentionDaysTooltip",
                            )}
                          >
                            <Slider
                              min={1}
                              max={10}
                              step={1}
                              value={field.value}
                              onChange={field.onChange}
                            />
                          </ChatOptionRow>
                        )}
                      />
                      <LlmRetryCard llmRetryEnabled={llmRetryEnabled} />
                      <LlmRateLimiterCard />
                    </>
                  }
                  openSections={{
                    runtime: openSections.runtime,
                    compaction: openSections.compaction,
                    aggressive: openSections.aggressive,
                    token: openSections.token,
                  }}
                  onToggleSection={(section: RunningConfigSectionId) =>
                    toggleSection(section)
                  }
                  onRunningChange={setRunningConfigField}
                  onContextChange={setContextConfigField}
                  onToolResultChange={setToolResultConfigField}
                  onThinkingTurnsChange={setThinkingTurnsToKeep}
                />

                <ChatOptionSection
                  title={t("agentConfig.otherRunningConfig")}
                  open={openSections.other}
                  onToggle={() => toggleSection("other")}
                >
                  <MemorySummaryCard />
                  <EmbeddingConfigCard />
                </ChatOptionSection>
              </ChatOptionsLayout>
            </div>
          </FormProvider>
        </div>
      </PageBody>
    </Page>
  );
}

export default AgentConfigPage;
