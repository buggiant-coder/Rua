import { useCallback, useMemo, useState } from "react";
import { EmptyState } from "@/components/EmptyState";
import type { ACPRunnerInfo, ACPRunnerUpdateRequest } from "../../../api/types";
import { ACPRunnerCard } from "./components";
import { useACP } from "./useACP";
import { useTranslation } from "react-i18next";
import { PageHeader } from "@/components/PageHeader";
import { Page, PageBody } from "@/components/Page";
import { useForm, Controller } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import styles from "./index.module.less";
import { Plus } from "lucide-react";
import {
  Btn,
  Drawer,
  FormField,
  Loading,
  NumberInput,
  TextArea,
  TextInput,
  Toggle,
} from "@/ui";

type FilterMode = "all" | "builtin" | "custom";

const BUILTIN_ORDER = ["opencode", "qwen", "claude", "codex"];

function ACPPage() {
  const { t } = useTranslation();
  const {
    runners,
    installingKeys,
    installTasks,
    loading,
    loadError,
    loadRunners,
    installRunner,
    createRunner,
    updateRunner,
    toggleEnabled,
    deleteRunner,
  } = useACP();

  const [createModalOpen, setCreateModalOpen] = useState(false);
  const [searchTerm, setSearchTerm] = useState("");
  const [filterMode, setFilterMode] = useState<FilterMode>("all");

  const createSchema = useMemo(
    () =>
      z.object({
        key: z
          .string()
          .min(1, t("acp.createKeyCommandRequired"))
          .refine((v) => v.trim().length > 0, {
            message: t("acp.createKeyCommandRequired"),
          }),
        command: z
          .string()
          .min(1, t("acp.createKeyCommandRequired"))
          .refine((v) => v.trim().length > 0, {
            message: t("acp.createKeyCommandRequired"),
          }),
        argsText: z.string(),
        envText: z.string().refine(
          (val) => {
            for (const rawLine of val.split(/\r?\n/)) {
              const line = rawLine.trim();
              if (!line) continue;
              if (line.indexOf("=") <= 0) return false;
            }
            return true;
          },
          { message: t("acp.invalidEnvText") },
        ),
        enabled: z.boolean(),
        trusted: z.boolean(),
        stdio_buffer_limit_bytes: z.number().min(1).nullable(),
      }),
    [t],
  );

  type CreateFormValues = z.infer<typeof createSchema>;

  const {
    register,
    handleSubmit,
    reset,
    control,
    formState: { errors },
  } = useForm<CreateFormValues>({
    resolver: zodResolver(createSchema),
    defaultValues: {
      key: "",
      enabled: true,
      command: "",
      argsText: "",
      envText: "",
      trusted: true,
      stdio_buffer_limit_bytes: 50 * 1024 * 1024,
    },
  });

  const sortedRunners = useMemo(() => {
    const sorted = [...runners];
    sorted.sort((a, b) => {
      if (a.enabled !== b.enabled) return a.enabled ? -1 : 1;
      const ai = BUILTIN_ORDER.indexOf(a.key);
      const bi = BUILTIN_ORDER.indexOf(b.key);
      if (ai !== -1 && bi !== -1) return ai - bi;
      if (ai !== -1) return -1;
      if (bi !== -1) return 1;
      return a.key.localeCompare(b.key);
    });
    return sorted;
  }, [runners]);

  const visibleRunners = useMemo<ACPRunnerInfo[]>(() => {
    let list = sortedRunners;

    if (filterMode === "builtin") {
      list = list.filter((r) => r.is_builtin);
    } else if (filterMode === "custom") {
      list = list.filter((r) => !r.is_builtin);
    }

    const q = searchTerm.trim().toLowerCase();
    if (q) {
      list = list.filter(
        (r) =>
          r.key.toLowerCase().includes(q) ||
          r.command.toLowerCase().includes(q),
      );
    }

    return list;
  }, [sortedRunners, filterMode, searchTerm]);

  const handleUpdate = useCallback(
    async (key: string, updates: ACPRunnerUpdateRequest) => {
      return await updateRunner(key, updates);
    },
    [updateRunner],
  );

  const onCreateValid = useCallback(
    async (values: CreateFormValues) => {
      const key = values.key.trim();
      const command = values.command.trim();

      const envObj: Record<string, string> = {};
      for (const rawLine of values.envText.split(/\r?\n/)) {
        const line = rawLine.trim();
        if (!line) continue;
        const index = line.indexOf("=");
        envObj[line.slice(0, index).trim()] = line.slice(index + 1);
      }

      const args = values.argsText
        .split(/\r?\n/)
        .map((s) => s.trim())
        .filter((s) => s.length > 0);

      const success = await createRunner({
        key,
        command,
        args,
        env: envObj,
        enabled: values.enabled,
        trusted: values.trusted,
        stdio_buffer_limit_bytes: values.stdio_buffer_limit_bytes ?? null,
      });

      if (success) {
        setCreateModalOpen(false);
        reset();
      }
    },
    [createRunner, reset],
  );

  return (
    <Page>
      <PageHeader
        agentScoped
        items={[{ title: t("acp.title") }]}
        onRefresh={loadRunners}
        refreshing={loading}
        extra={
          <div style={{ display: "flex", gap: 8 }}>
            <Btn
              icon={<Plus />}
              onClick={() => {
                reset({
                  key: "",
                  enabled: true,
                  command: "",
                  argsText: "",
                  envText: "",
                  trusted: true,
                  stdio_buffer_limit_bytes: 50 * 1024 * 1024,
                });
                setCreateModalOpen(true);
              }}
            />
          </div>
        }
      />

      <PageBody>
        {loading ? (
          <div className={styles.loading}>
            <Loading size="large" tip={t("common.loading")} />
          </div>
        ) : loadError ? (
          <EmptyState
            variant="error"
            description={loadError}
            onRetry={loadRunners}
          />
        ) : runners.length === 0 ? (
          <EmptyState description={t("acp.emptyState")} />
        ) : (
          <>
            <div className={styles.filterRow}>
              <div className={styles.filterBar}>
                <TextInput
                  placeholder={t("acp.searchPlaceholder")}
                  value={searchTerm}
                  onChange={(e) => setSearchTerm(e.target.value)}
                  className={styles.filterInput}
                />
                <div className={styles.filterTabs}>
                  {(["all", "builtin", "custom"] as FilterMode[]).map(
                    (mode) => (
                      <Btn
                        key={mode}
                        variant={filterMode === mode ? "filled" : "outline"}
                        size="sm"
                        className={styles.filterTab}
                        onClick={() => setFilterMode(mode)}
                      >
                        {t(`acp.filter.${mode}`)}
                      </Btn>
                    ),
                  )}
                </div>
              </div>
            </div>

            {visibleRunners.length === 0 ? (
              <EmptyState description={t("acp.noMatch")} />
            ) : (
              <div className={styles.acpGrid}>
                {visibleRunners.map((runner) => (
                  <ACPRunnerCard
                    key={runner.key}
                    runner={runner}
                    onToggle={toggleEnabled}
                    onDelete={deleteRunner}
                    onInstall={() => installRunner(runner.key)}
                    onUpdate={handleUpdate}
                    installTask={installTasks[runner.key]}
                    installing={Boolean(installingKeys[runner.key])}
                    onMouseEnter={() => {}}
                    onMouseLeave={() => {}}
                  />
                ))}
              </div>
            )}
          </>
        )}
      </PageBody>

      <Drawer
        title={t("acp.createCustom")}
        open={createModalOpen}
        onClose={() => setCreateModalOpen(false)}
        destroyOnClose
        footer={
          <div className={styles.drawerFooter}>
            <Btn variant="outline" onClick={() => setCreateModalOpen(false)}>
              {t("common.cancel")}
            </Btn>
            <Btn variant="filled" onClick={handleSubmit(onCreateValid)}>
              {t("common.create")}
            </Btn>
          </div>
        }
      >
        <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
          <FormField
            label={t("acp.form.key")}
            error={errors.key?.message}
            required
          >
            <TextInput {...register("key")} placeholder="my_agent" />
          </FormField>
          <FormField label={t("acp.form.enabled")}>
            <Controller
              name="enabled"
              control={control}
              render={({ field }) => (
                <Toggle
                  checked={field.value}
                  onChange={(e) => field.onChange(e.target.checked)}
                />
              )}
            />
          </FormField>
          <FormField
            label={t("acp.form.command")}
            error={errors.command?.message}
            required
          >
            <TextInput {...register("command")} placeholder="npx" />
          </FormField>
          <FormField
            label={t("acp.form.args")}
            error={errors.argsText?.message}
          >
            <TextArea
              {...register("argsText")}
              rows={4}
              placeholder={"-y\n@agentclientprotocol/codex-acp"}
            />
          </FormField>
          <FormField label={t("acp.form.env")} error={errors.envText?.message}>
            <TextArea
              {...register("envText")}
              rows={4}
              placeholder="OPENAI_API_KEY=..."
            />
          </FormField>
          <FormField label={t("acp.form.trusted")}>
            <Controller
              name="trusted"
              control={control}
              render={({ field }) => (
                <Toggle
                  checked={field.value}
                  onChange={(e) => field.onChange(e.target.checked)}
                />
              )}
            />
          </FormField>
          <FormField
            label={t("acp.form.stdioBufferLimit")}
            error={errors.stdio_buffer_limit_bytes?.message}
          >
            <Controller
              name="stdio_buffer_limit_bytes"
              control={control}
              render={({ field }) => (
                <NumberInput
                  min={1}
                  style={{ width: "100%" }}
                  value={field.value}
                  onChange={field.onChange}
                />
              )}
            />
          </FormField>
        </div>
      </Drawer>
    </Page>
  );
}

export default ACPPage;
