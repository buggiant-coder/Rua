/* eslint-disable react-refresh/only-export-components */
import { useCallback, useEffect, useState } from "react";
import { useLoaderData } from "react-router-dom";
import { useForm, Controller } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import dayjs from "dayjs";
import customParseFormat from "dayjs/plugin/customParseFormat";
import { useTranslation } from "react-i18next";
import api from "../../../api";
import { useAgentStore } from "../../../stores/agentStore";
import type { HeartbeatConfig } from "../../../api/types/heartbeat";
import { parseEvery, serializeEvery, type EveryUnit } from "./parseEvery";
import { PageHeader } from "@/components/PageHeader";
import { Page, PageBody } from "@/components/Page";
import { RuaCard } from "@/components/RuaCard";
import Skeleton from "@/components/Skeleton";
import styles from "./index.module.less";
import { Save } from "lucide-react";
import {
  Btn,
  FormField,
  NumberInput,
  Select,
  TimePicker,
  Toggle,
  toast,
} from "@/ui";
import { routeAgentId } from "@/routes/routeAgent";
dayjs.extend(customParseFormat);

const TIME_FORMAT = "HH:mm";

function TimePickerHHmm({
  value,
  onChange,
}: {
  value?: string | null;
  onChange?: (s: string) => void;
}) {
  const strVal =
    typeof value === "string" ? value : Array.isArray(value) ? value[0] : null;
  return (
    <TimePicker
      format={TIME_FORMAT}
      value={strVal ? dayjs(strVal, TIME_FORMAT) : null}
      onChange={(_, str) => {
        const s = typeof str === "string" ? str : str?.[0];
        if (s) onChange?.(s);
      }}
      minuteStep={15}
      needConfirm={false}
      style={{ width: "100%" }}
    />
  );
}

const EVERY_UNIT_OPTIONS: { value: EveryUnit; labelKey: string }[] = [
  { value: "m", labelKey: "heartbeat.unitMinutes" },
  { value: "h", labelKey: "heartbeat.unitHours" },
];

const TARGET_OPTIONS = [
  { value: "main", labelKey: "heartbeat.targetMain" },
  { value: "last", labelKey: "heartbeat.targetLast" },
];

const schema = z.object({
  enabled: z.boolean(),
  everyNumber: z.number().min(1),
  everyUnit: z.enum(["m", "h"]),
  target: z.string(),
  useActiveHours: z.boolean(),
  activeHoursStart: z.string(),
  activeHoursEnd: z.string(),
});

type FormValues = z.infer<typeof schema>;

interface HeartbeatInitialData {
  agentId: string;
  config: HeartbeatConfig;
}

interface HeartbeatPageProps {
  initialData?: HeartbeatInitialData;
}

export async function loader(): Promise<HeartbeatInitialData> {
  const agentId = routeAgentId();
  return {
    agentId,
    config: await api.getHeartbeatConfig(),
  };
}

export function RouteComponent() {
  return (
    <HeartbeatPage initialData={useLoaderData() as HeartbeatInitialData} />
  );
}

function HeartbeatPage({ initialData }: HeartbeatPageProps) {
  const { t } = useTranslation();
  const { selectedAgent } = useAgentStore();
  const [loading, setLoading] = useState(initialData === undefined);
  const [saving, setSaving] = useState(false);
  const { control, handleSubmit, getValues, reset, watch } =
    useForm<FormValues>({
      resolver: zodResolver(schema),
      defaultValues: {
        enabled: false,
        everyNumber: 6,
        everyUnit: "h",
        target: "main",
        useActiveHours: false,
        activeHoursStart: "08:00",
        activeHoursEnd: "22:00",
      },
    });

  const useActiveHours = watch("useActiveHours");

  const applyConfig = useCallback(
    (data: HeartbeatConfig) => {
      const everyParts = parseEvery(data.every ?? "6h");
      reset({
        enabled: data.enabled ?? false,
        everyNumber: everyParts.number,
        everyUnit: everyParts.unit,
        target: data.target ?? "main",
        useActiveHours: !!data.activeHours,
        activeHoursStart: data.activeHours?.start ?? "08:00",
        activeHoursEnd: data.activeHours?.end ?? "22:00",
      });
    },
    [reset],
  );

  const fetchConfig = async () => {
    setLoading(true);
    try {
      const data = await api.getHeartbeatConfig();
      applyConfig(data);
    } catch (e) {
      console.error("Failed to load heartbeat config:", e);
      toast.error(t("heartbeat.loadFailed"));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    if (initialData) {
      applyConfig(initialData.config);
      setLoading(false);
      return;
    }
    void fetchConfig();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [applyConfig, initialData, selectedAgent]);

  const onFinish = async (values: FormValues) => {
    const every =
      values.everyNumber != null && values.everyUnit
        ? serializeEvery({
            number: values.everyNumber,
            unit: values.everyUnit,
          })
        : "6h";
    const body: HeartbeatConfig = {
      enabled: values.enabled ?? false,
      every,
      target: values.target ?? "main",
      activeHours:
        values.useActiveHours &&
        values.activeHoursStart &&
        values.activeHoursEnd
          ? {
              start: values.activeHoursStart,
              end: values.activeHoursEnd,
            }
          : undefined,
    };
    setSaving(true);
    try {
      await api.updateHeartbeatConfig(body);
      toast.success(t("heartbeat.saveSuccess"));
    } catch (e) {
      console.error("Failed to save heartbeat config:", e);
      toast.error(t("heartbeat.saveFailed"));
    } finally {
      setSaving(false);
    }
  };

  if (loading) {
    return (
      <Page className={styles.heartbeatPage}>
        <PageHeader
          agentScoped
          items={[{ title: t("heartbeat.title") }]}
          onRefresh={fetchConfig}
          refreshing={loading}
          extra={
            <>
              <Btn icon={<Save />} aria-label={t("common.save")} disabled />
            </>
          }
        />
        <PageBody>
          <Skeleton variant="detail" count={2} />
        </PageBody>
      </Page>
    );
  }

  return (
    <Page className={styles.heartbeatPage}>
      <PageHeader
        agentScoped
        items={[{ title: t("heartbeat.title") }]}
        onRefresh={fetchConfig}
        extra={
          <>
            <Btn
              icon={<Save />}
              aria-label={t("common.save")}
              loading={saving}
              onClick={handleSubmit(onFinish)}
            />
          </>
        }
      />
      <PageBody>
        <div className={styles.heartbeatContent}>
          <RuaCard className={styles.card} bare>
            <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
              <FormField label={t("heartbeat.enabled")}>
                <Controller
                  name="enabled"
                  control={control}
                  render={({ field }) => (
                    <Toggle
                      checked={field.value}
                      disabled={saving}
                      onChange={(event) => {
                        const enabled = event.target.checked;
                        field.onChange(event);
                        void onFinish({ ...getValues(), enabled });
                      }}
                    />
                  )}
                />
              </FormField>

              <FormField label={t("heartbeat.every")} required>
                <div className={styles.everyRow}>
                  <Controller
                    name="everyNumber"
                    control={control}
                    render={({ field }) => (
                      <NumberInput
                        min={1}
                        className={styles.everyNumber}
                        value={field.value}
                        onChange={field.onChange}
                      />
                    )}
                  />
                  <Controller
                    name="everyUnit"
                    control={control}
                    render={({ field }) => (
                      <Select
                        className={styles.everyUnit}
                        value={field.value}
                        onChange={field.onChange}
                        options={EVERY_UNIT_OPTIONS.map((opt) => ({
                          value: opt.value,
                          label: t(opt.labelKey),
                        }))}
                      />
                    )}
                  />
                </div>
              </FormField>

              <FormField label={t("heartbeat.target")} required>
                <Controller
                  name="target"
                  control={control}
                  render={({ field }) => (
                    <Select
                      value={field.value}
                      onChange={field.onChange}
                      options={TARGET_OPTIONS.map((opt) => ({
                        value: opt.value,
                        label: t(opt.labelKey),
                      }))}
                    />
                  )}
                />
              </FormField>

              <FormField label={t("heartbeat.activeHours")}>
                <Controller
                  name="useActiveHours"
                  control={control}
                  render={({ field }) => (
                    <Toggle checked={field.value} onChange={field.onChange} />
                  )}
                />
              </FormField>

              {useActiveHours && (
                <div className={styles.activeHoursRow}>
                  <FormField label={t("heartbeat.activeStart")}>
                    <Controller
                      name="activeHoursStart"
                      control={control}
                      render={({ field }) => (
                        <TimePickerHHmm
                          value={field.value}
                          onChange={field.onChange}
                        />
                      )}
                    />
                  </FormField>
                  <FormField label={t("heartbeat.activeEnd")}>
                    <Controller
                      name="activeHoursEnd"
                      control={control}
                      render={({ field }) => (
                        <TimePickerHHmm
                          value={field.value}
                          onChange={field.onChange}
                        />
                      )}
                    />
                  </FormField>
                </div>
              )}
            </div>
          </RuaCard>
        </div>
      </PageBody>
    </Page>
  );
}

export default HeartbeatPage;
