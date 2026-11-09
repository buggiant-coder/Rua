import { useState, useEffect, useCallback } from "react";
import type { UseFormReturn, FieldValues } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { z } from "zod";
import api from "../../../api";
import { agentsApi } from "../../../api/modules/agents";
import type { AgentsRunningConfig } from "../../../api/types";
import { useAgentStore } from "../../../stores/agentStore";
import { Modal, toast } from "@/ui";

export const agentConfigSchema = z.object({
  max_iters: z.number().int().min(0),
  auto_continue_on_text_only: z.boolean(),
  show_debug_menu: z.boolean().optional().default(false),
  memory_manager_backend: z.string(),
  max_input_length: z.number().min(1000),
  history_max_length: z.number().optional(),
  llm_retry_enabled: z.boolean(),
  llm_max_retries: z.number().min(1),
  llm_backoff_base: z.number().min(0.1),
  llm_backoff_cap: z.number().min(0.5),
  llm_max_concurrent: z.number().min(1),
  llm_max_qpm: z.number().min(0),
  llm_rate_limit_pause: z.number().min(1.0),
  llm_rate_limit_jitter: z.number().min(0.0),
  llm_acquire_timeout: z.number().min(10.0),
  context_compact: z.object({
    context_compact_enabled: z.boolean(),
    token_count_estimate_divisor: z.number(),
    token_count_model: z.string().optional(),
    token_count_use_mirror: z.boolean().optional(),
    blocking_compact_ratio: z.number(),
    history_replacement_enabled: z.boolean(),
    active_work_plan_injection_enabled: z.boolean().optional().default(false),
    active_work_plan_injection_interval_ratio: z
      .number()
      .optional()
      .default(0.05),
    early_background_compaction_enabled: z.boolean(),
    early_compaction_target_ratio: z.number(),
    context_reserve_ratio: z.number(),
    compact_with_thinking_block: z.boolean(),
    compact_enhanced_prompt: z.boolean(),
    thinking_turns_to_keep: z.number().optional(),
    min_keep_messages: z.number(),
  }),
  tool_result_compact: z.object({
    enabled: z.boolean(),
    projection_truncation_enabled: z.boolean(),
    projection_truncation_batch_size: z.number().int().min(1).default(1),
    recent_n: z.number(),
    execution_output_max_tokens: z.number(),
    raw_capture_max_bytes: z.number().optional(),
    recall_slice_max_tokens: z.number().optional(),
    old_max_bytes: z.number(),
    retention_days: z.number(),
  }),
  memory_summary: z.object({
    memory_summary_enabled: z.boolean(),
    dream_cron: z.string(),
    search_mode: z.enum(["grep", "precise"]).default("precise"),
  }),
  embedding_config: z.object({
    backend: z.string().optional(),
    base_url: z.string().optional().default(""),
    model_name: z.string().optional().default(""),
    api_key: z.string().optional().default(""),
    dimensions: z.number().min(1),
    enable_cache: z.boolean(),
    use_dimensions: z.boolean().optional(),
    max_cache_size: z.number(),
    max_input_length: z.number(),
    max_batch_size: z.number(),
  }),
  session_ttl_days: z.number().optional(),
});

export type AgentConfigFormValues = z.infer<typeof agentConfigSchema>;

export interface AgentConfigInitialData {
  agentId: string;
  config: AgentsRunningConfig;
  language: string;
  timezone: string;
}

export function useAgentConfig<T extends FieldValues = AgentConfigFormValues>(
  methods: UseFormReturn<AgentConfigFormValues, unknown, T>,
  initialData?: AgentConfigInitialData,
) {
  const { t } = useTranslation();
  const selectedAgent = useAgentStore((s) => s.selectedAgent);
  const [loading, setLoading] = useState(initialData === undefined);
  const [saving, setSaving] = useState(false);
  const [resetting, setResetting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [language, setLanguage] = useState<string>("zh");
  const [savingLang, setSavingLang] = useState(false);
  const [timezone, setTimezone] = useState<string>("UTC");
  const [savingTimezone, setSavingTimezone] = useState(false);

  const applyInitialData = useCallback(
    (data: AgentConfigInitialData) => {
      methods.reset({
        ...data.config,
        auto_continue_on_text_only:
          data.config.auto_continue_on_text_only ?? false,
      } as AgentConfigFormValues);
      setLanguage(data.language);
      setTimezone(data.timezone || "UTC");
    },
    [methods],
  );

  const fetchConfig = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [config, langResp, tzResp] = await Promise.all([
        agentsApi.getAgentRunningConfig(selectedAgent),
        agentsApi.getAgentLanguage(selectedAgent),
        api.getUserTimezone(),
      ]);
      methods.reset({
        ...config,
        auto_continue_on_text_only: config.auto_continue_on_text_only ?? false,
      } as AgentConfigFormValues);
      setLanguage(langResp.language);
      setTimezone(tzResp.timezone || "UTC");
    } catch (err) {
      const errMsg =
        err instanceof Error ? err.message : t("agentConfig.loadFailed");
      setError(errMsg);
    } finally {
      setLoading(false);
    }
  }, [methods, t, selectedAgent]);

  useEffect(() => {
    if (initialData) {
      applyInitialData(initialData);
      setError(null);
      setLoading(false);
      return;
    }
    void fetchConfig();
  }, [applyInitialData, fetchConfig, initialData]);

  const handleSave = useCallback(async () => {
    const isValid = await methods.trigger();
    if (!isValid) return;
    const values = methods.getValues();
    try {
      setSaving(true);
      await agentsApi.updateAgentRunningConfig(
        selectedAgent,
        values as unknown as AgentsRunningConfig,
      );
      toast.success(t("agentConfig.saveSuccess"));
    } catch (err) {
      const errMsg =
        err instanceof Error ? err.message : t("agentConfig.saveFailed");
      toast.error(errMsg);
    } finally {
      setSaving(false);
    }
  }, [methods, t, selectedAgent]);

  const handleReset = useCallback(() => {
    Modal.confirm({
      title: t("agentConfig.resetTitle"),
      content: t("agentConfig.resetDescription"),
      okText: t("common.reset"),
      cancelText: t("common.cancel"),
      onOk: async () => {
        setResetting(true);
        try {
          const config = await agentsApi.resetAgentRunningConfig(selectedAgent);
          methods.reset({
            ...config,
            auto_continue_on_text_only:
              config.auto_continue_on_text_only ?? false,
          } as AgentConfigFormValues);
          toast.success(t("agentConfig.resetSuccess"));
        } catch (err) {
          const errMsg =
            err instanceof Error ? err.message : t("agentConfig.resetFailed");
          toast.error(errMsg);
        } finally {
          setResetting(false);
        }
      },
    });
  }, [methods, selectedAgent, t]);

  const handleLanguageChange = useCallback(
    (value: string): void => {
      if (value === language) return;
      Modal.confirm({
        title: t("agentConfig.languageConfirmTitle"),
        content: (
          <span style={{ whiteSpace: "pre-line" }}>
            {t("agentConfig.languageConfirmContent")}
          </span>
        ),
        okText: t("agentConfig.languageConfirmOk"),
        cancelText: t("common.cancel"),
        onOk: async () => {
          setSavingLang(true);
          try {
            const resp = await agentsApi.updateAgentLanguage(
              selectedAgent,
              value,
            );
            setLanguage(resp.language);
            if (resp.copied_files && resp.copied_files.length > 0) {
              toast.success(
                t("agentConfig.languageSaveSuccessWithFiles", {
                  count: resp.copied_files.length,
                }),
              );
            } else {
              toast.success(t("agentConfig.languageSaveSuccess"));
            }
          } catch (err) {
            const errMsg =
              err instanceof Error
                ? err.message
                : t("agentConfig.languageSaveFailed");
            toast.error(errMsg);
          } finally {
            setSavingLang(false);
          }
        },
      });
    },
    [language, t, selectedAgent],
  );

  const handleTimezoneChange = useCallback(
    async (value: string) => {
      if (value === timezone) return;
      setSavingTimezone(true);
      try {
        await api.updateUserTimezone(value);
        setTimezone(value);
        toast.success(t("agentConfig.timezoneSaveSuccess"));
      } catch (err) {
        const errMsg =
          err instanceof Error
            ? err.message
            : t("agentConfig.timezoneSaveFailed");
        toast.error(errMsg);
      } finally {
        setSavingTimezone(false);
      }
    },
    [timezone, t],
  );

  return {
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
  };
}
