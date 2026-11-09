import { useForm, Controller } from "react-hook-form";
import type {
  ACPRunnerInfo,
  ACPRunnerInstallTask,
  ACPRunnerUpdateRequest,
} from "../../../../api/types";
import { useTranslation } from "react-i18next";
import { useEffect, useState } from "react";
import { Download } from "lucide-react";
import { RuaCard } from "@/components/RuaCard";
import styles from "../index.module.less";
import {
  Btn,
  Drawer,
  FormField,
  Modal,
  NumberInput,
  Tag,
  TextArea,
  TextInput,
  Toggle,
} from "@/ui";

interface ACPRunnerCardProps {
  runner: ACPRunnerInfo;
  onToggle: (runner: ACPRunnerInfo) => void;
  onDelete: (runner: ACPRunnerInfo) => void;
  onInstall: (runner: ACPRunnerInfo) => void;
  onUpdate: (key: string, updates: ACPRunnerUpdateRequest) => Promise<boolean>;
  installTask?: ACPRunnerInstallTask;
  installing?: boolean;
  onMouseEnter: () => void;
  onMouseLeave: () => void;
}

type RunnerFormValues = {
  enabled: boolean;
  command: string;
  argsText: string;
  envText: string;
  trusted: boolean;
  stdio_buffer_limit_bytes: number | null;
};

function stringifyArgs(args: string[] = []) {
  return args.join("\n");
}

function parseArgs(text: string | undefined) {
  return (text || "")
    .split(/\r?\n/)
    .map((item) => item.trim())
    .filter(Boolean);
}

function stringifyEnv(env: Record<string, string> = {}) {
  return Object.entries(env)
    .map(([key, value]) => `${key}=${value}`)
    .join("\n");
}

function parseEnv(text: string | undefined) {
  const env: Record<string, string> = {};
  for (const rawLine of (text || "").split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line) continue;
    const index = line.indexOf("=");
    if (index <= 0) {
      throw new Error("invalid env");
    }
    env[line.slice(0, index).trim()] = line.slice(index + 1);
  }
  return env;
}

function formValuesFromRunner(runner: ACPRunnerInfo): RunnerFormValues {
  return {
    enabled: runner.enabled,
    command: runner.command,
    argsText: stringifyArgs(runner.args),
    envText: stringifyEnv(runner.env),
    trusted: runner.trusted,
    stdio_buffer_limit_bytes:
      runner.stdio_buffer_limit_bytes ?? 50 * 1024 * 1024,
  };
}

export function ACPRunnerCard({
  runner,
  onToggle,
  onDelete,
  onInstall,
  onUpdate,
  installTask,
  installing = false,
  onMouseEnter,
  onMouseLeave,
}: ACPRunnerCardProps) {
  const { t } = useTranslation();
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [deleteModalOpen, setDeleteModalOpen] = useState(false);
  const [saving, setSaving] = useState(false);

  const {
    register,
    handleSubmit,
    control,
    reset,
    setError,
    formState: { errors },
  } = useForm<RunnerFormValues>({
    defaultValues: formValuesFromRunner(runner),
  });

  useEffect(() => {
    if (drawerOpen) {
      reset(formValuesFromRunner(runner));
    }
  }, [drawerOpen, reset, runner]);

  const onSave = async (values: RunnerFormValues) => {
    let env: Record<string, string>;
    try {
      env = parseEnv(values.envText);
    } catch {
      setError("envText", { message: t("acp.invalidEnvText") });
      return;
    }

    setSaving(true);
    try {
      const success = await onUpdate(runner.key, {
        enabled: Boolean(values.enabled),
        command: values.command.trim(),
        args: parseArgs(values.argsText),
        env,
        trusted: Boolean(values.trusted),
        stdio_buffer_limit_bytes: values.stdio_buffer_limit_bytes ?? null,
      });
      if (success) {
        setDrawerOpen(false);
      }
    } finally {
      setSaving(false);
    }
  };

  const commandDisplay = runner.command
    ? `${runner.command} ${runner.args.join(" ")}`.trim()
    : "-";
  const installInFlight =
    installing ||
    installTask?.status === "pending" ||
    installTask?.status === "running";
  const installTag = installInFlight ? (
    <Tag key="installing" color="primary">
      {t("acp.installStatus.running")}
    </Tag>
  ) : installTask?.status === "failed" ? (
    <Tag key="install-failed" color="danger">
      {t("acp.installStatus.failed")}
    </Tag>
  ) : null;
  const configuredStatusTag = runner.enabled ? (
    <Tag key="configured-status" color="success">
      {t("common.enabled")}
    </Tag>
  ) : (
    <Tag key="configured-status" color="default">
      {t("common.disabled")}
    </Tag>
  );
  const actions = [
    ...(runner.is_builtin
      ? [
          <Btn
            key="install"
            variant="outline"
            size="sm"
            icon={<Download size={14} />}
            loading={installing}
            disabled={installInFlight}
            title={t("acp.install")}
            aria-label={t("acp.install")}
            onClick={(e) => {
              e.stopPropagation();
              onInstall(runner);
            }}
          />,
        ]
      : []),
    <span key="sw" onClick={(e) => e.stopPropagation()}>
      <Toggle
        checked={runner.enabled}
        disabled={installing}
        onChange={() => onToggle(runner)}
      />
    </span>,
  ];

  return (
    <>
      <RuaCard
        hoverable
        onClick={() => setDrawerOpen(true)}
        onMouseEnter={onMouseEnter}
        onMouseLeave={onMouseLeave}
        className={`${styles.runnerCard}${
          !runner.enabled ? ` ${styles.dimmed}` : ""
        }`}
        title={<span className={styles.runnerTitle}>{runner.key}</span>}
        tags={[
          runner.is_builtin ? (
            <Tag key="builtin" color="primary">
              {t("acp.builtin")}
            </Tag>
          ) : (
            <Tag key="custom" color="default">
              {t("acp.custom")}
            </Tag>
          ),
          configuredStatusTag,
          ...(installTag ? [installTag] : []),
        ]}
        actions={actions}
      >
        <p className={styles.runnerCommand}>{commandDisplay}</p>
        <div className={styles.runnerMeta}>
          <span>
            {t("acp.card.command")}: {runner.command || "-"}
          </span>
          <span>
            {t("acp.card.args")}: {runner.args.join(" ") || "-"}
          </span>
        </div>
      </RuaCard>

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
                onDelete(runner);
              }}
            >
              {t("common.confirm")}
            </Btn>
          </>
        }
      >
        <p>{t("acp.deleteConfirm")}</p>
      </Modal>

      <Drawer
        title={t("acp.editTitle", { key: runner.key })}
        open={drawerOpen}
        onClose={() => setDrawerOpen(false)}
        destroyOnClose
        footer={
          <div className={styles.drawerFooter}>
            <Btn variant="outline" onClick={() => setDrawerOpen(false)}>
              {t("common.cancel")}
            </Btn>
            {!runner.is_builtin && (
              <Btn variant="danger" onClick={() => setDeleteModalOpen(true)}>
                {t("common.delete")}
              </Btn>
            )}
            <Btn
              variant="filled"
              loading={saving}
              onClick={handleSubmit(onSave)}
            >
              {t("common.save")}
            </Btn>
          </div>
        }
      >
        <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
          <div className={styles.runnerDiagnostics}>
            <div className={styles.runnerDiagnosticRow}>
              <span>{t("acp.form.enabled")}</span>
              <strong>
                {runner.enabled ? t("common.enabled") : t("common.disabled")}
              </strong>
            </div>
            {installTask && (
              <>
                <div className={styles.runnerDiagnosticRow}>
                  <span>{t("acp.diagnostics.installStage")}</span>
                  <strong>{installTask.stage}</strong>
                </div>
                <div className={styles.runnerDiagnosticDetail}>
                  {installTask.message}
                </div>
                {installTask.detail && (
                  <pre className={styles.runnerDiagnosticPre}>
                    {installTask.detail}
                  </pre>
                )}
              </>
            )}
            {installTask?.check_result?.message && (
              <pre className={styles.runnerDiagnosticPre}>
                {installTask.check_result.message}
              </pre>
            )}
          </div>
          <FormField label={t("acp.form.key")}>
            <TextInput value={runner.key} disabled />
          </FormField>
          <FormField label={t("acp.form.enabled")}>
            <Controller
              name="enabled"
              control={control}
              render={({ field }) => (
                <Toggle
                  checked={field.value}
                  disabled={installing}
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
            <TextInput
              {...register("command", {
                required: t("acp.createKeyCommandRequired"),
              })}
              placeholder="npx"
            />
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
    </>
  );
}
