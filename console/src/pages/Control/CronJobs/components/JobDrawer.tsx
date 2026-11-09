import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useForm, Controller } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import dayjs from "dayjs";
import type { CronJobSpecOutput } from "../../../../api/types";
import { cronJobApi } from "../../../../api/modules/cronjob";
import { TIMEZONE_OPTIONS, DEFAULT_FORM_VALUES } from "./constants";
import { parseCron } from "./parseCron";
import { InputWithExample } from "../../../../components/InputWithExample";
import styles from "../index.module.less";
import {
  Btn,
  Check,
  DatePicker,
  Drawer,
  FormField,
  NumberInput,
  Radio,
  RadioGroup,
  Select,
  TextArea,
  TimePicker,
  Toggle,
} from "@/ui";

type CronJob = CronJobSpecOutput;
type DispatchTargetOption = {
  channel: string;
  user_id: string;
  session_id: string;
};

interface JobDrawerProps {
  open: boolean;
  editingJob: CronJob | null;
  saving: boolean;
  onClose: () => void;
  onSubmit: (values: any) => void;
  defaultTimezone?: string;
}

export function JobDrawer({
  open,
  editingJob,
  saving,
  onClose,
  onSubmit,
  defaultTimezone,
}: JobDrawerProps) {
  const { t } = useTranslation();
  const [dispatchTargets, setDispatchTargets] = useState<
    DispatchTargetOption[]
  >([]);

  const schema = z
    .object({
      name: z.string().min(1, t("cronJobs.pleaseInputName")),
      enabled: z.boolean(),
      task_type: z.enum(["text", "agent"]),
      schedule: z.object({
        type: z.enum(["cron", "once"]),
        cron: z.string().optional(),
        run_at: z.string().optional(),
        timezone: z.string().optional(),
        repeat_every_days: z.number().optional().nullable(),
        repeat_end_type: z.enum(["never", "until", "count"]).optional(),
        repeat_until: z.string().optional(),
        repeat_count: z.number().optional().nullable(),
      }),
      scheduleMode: z.enum(["cron", "once"]),
      cronType: z.enum(["hourly", "daily", "weekly", "custom"]),
      cronTime: z.any().optional().nullable(),
      cronDaysOfWeek: z.array(z.string()).optional(),
      cronCustom: z.string().optional(),
      onceDate: z.string().optional(),
      onceTime: z.any().optional().nullable(),
      repeatEnabled: z.boolean().optional(),
      repeatEveryDays: z.number().optional().nullable(),
      repeatEndType: z.enum(["never", "until", "count"]).optional(),
      repeatUntilDate: z.string().optional(),
      repeatCount: z.number().optional().nullable(),
      text: z.string().optional(),
      save_result_to_inbox: z.boolean().optional(),
      request: z
        .object({
          input: z.string().optional(),
        })
        .optional(),
      dispatch: z.object({
        type: z.literal("channel"),
        channel: z.string().min(1, t("cronJobs.pleaseInputChannel")),
        target: z.object({
          user_id: z.string().min(1, t("cronJobs.pleaseInputUserId")),
          session_id: z.string().min(1, t("cronJobs.pleaseInputSessionId")),
        }),
        mode: z.enum(["stream", "final"]).optional(),
      }),
      runtime: z
        .object({
          max_concurrency: z.number().optional().nullable(),
          timeout_seconds: z.number().optional().nullable(),
          misfire_grace_seconds: z.number().optional().nullable(),
          share_session: z.boolean().optional(),
          tool_safety: z.boolean().optional(),
        })
        .optional(),
    })
    .superRefine((data, ctx) => {
      if (
        data.scheduleMode === "cron" &&
        data.cronType === "custom" &&
        !data.cronCustom?.trim()
      ) {
        ctx.addIssue({
          code: "custom",
          message: t("cronJobs.pleaseInputCron"),
          path: ["cronCustom"],
        });
      }
      if (
        data.scheduleMode === "cron" &&
        (data.cronType === "daily" || data.cronType === "weekly") &&
        !data.cronTime
      ) {
        ctx.addIssue({
          code: "custom",
          message: t("cronJobs.pleaseSelectTime"),
          path: ["cronTime"],
        });
      }
      if (
        data.scheduleMode === "cron" &&
        data.cronType === "weekly" &&
        (!data.cronDaysOfWeek || data.cronDaysOfWeek.length === 0)
      ) {
        ctx.addIssue({
          code: "custom",
          message: t("cronJobs.pleaseSelectDay"),
          path: ["cronDaysOfWeek"],
        });
      }
      if (data.scheduleMode === "once") {
        if (!data.onceDate?.trim()) {
          ctx.addIssue({
            code: "custom",
            message: t("cronJobs.pleaseSelectDate"),
            path: ["onceDate"],
          });
        }
        if (!data.onceTime) {
          ctx.addIssue({
            code: "custom",
            message: t("cronJobs.pleaseSelectTime"),
            path: ["onceTime"],
          });
        }
        if (
          data.repeatEnabled &&
          data.repeatEndType === "count" &&
          !data.repeatCount
        ) {
          ctx.addIssue({
            code: "custom",
            message: t("cronJobs.pleaseInputRepeatCount"),
            path: ["repeatCount"],
          });
        }
        if (
          data.repeatEnabled &&
          data.repeatEndType === "until" &&
          !data.repeatUntilDate?.trim()
        ) {
          ctx.addIssue({
            code: "custom",
            message: t("cronJobs.pleaseSelectRepeatUntil"),
            path: ["repeatUntilDate"],
          });
        }
      }
      if (data.task_type === "text" && !data.text?.trim()) {
        ctx.addIssue({
          code: "custom",
          message: t("cronJobs.pleaseInputMessageContent"),
          path: ["text"],
        });
      }
      if (data.task_type === "agent") {
        const input = data.request?.input;
        if (!input?.trim()) {
          ctx.addIssue({
            code: "custom",
            message: t("cronJobs.pleaseInputRequest"),
            path: ["request", "input"],
          });
        } else {
          try {
            JSON.parse(input);
          } catch {
            ctx.addIssue({
              code: "custom",
              message: t("cronJobs.invalidJsonFormat"),
              path: ["request", "input"],
            });
          }
        }
      }
    });

  const {
    control,
    handleSubmit,
    reset,
    setValue,
    watch,
    formState: { errors },
  } = useForm({
    resolver: zodResolver(schema),
    defaultValues: DEFAULT_FORM_VALUES as any,
  });

  useEffect(() => {
    if (!open) return;
    cronJobApi
      .listDispatchTargets({ limit: 500 })
      .then((res) => setDispatchTargets(res.items || []))
      .catch((err) => {
        console.warn("Failed to load cron dispatch targets", err);
        setDispatchTargets([]);
      });
    if (editingJob) {
      const schedule = (editingJob.schedule || {}) as any;
      const scheduleType = schedule.type || "cron";
      const cronParts = parseCron(
        scheduleType === "cron" ? schedule.cron || "0 9 * * *" : "0 9 * * *",
      );
      const runAt =
        scheduleType === "once" && schedule.run_at
          ? dayjs(schedule.run_at)
          : null;
      const formValues: any = {
        ...editingJob,
        scheduleMode: scheduleType,
        onceDate: runAt
          ? runAt.format("YYYY-MM-DD")
          : DEFAULT_FORM_VALUES.onceDate,
        onceTime: runAt ? runAt.format("HH:mm") : DEFAULT_FORM_VALUES.onceTime,
        repeatEnabled: !!schedule.repeat_every_days,
        repeatEveryDays:
          schedule.repeat_every_days ?? DEFAULT_FORM_VALUES.repeatEveryDays,
        repeatEndType:
          schedule.repeat_end_type ?? DEFAULT_FORM_VALUES.repeatEndType,
        repeatUntilDate: schedule.repeat_until
          ? dayjs(schedule.repeat_until).format("YYYY-MM-DD")
          : DEFAULT_FORM_VALUES.repeatUntilDate,
        repeatCount: schedule.repeat_count ?? DEFAULT_FORM_VALUES.repeatCount,
        request: {
          ...editingJob.request,
          input: editingJob.request?.input
            ? JSON.stringify(editingJob.request.input, null, 2)
            : "",
        },
        cronType: cronParts.type,
        cronTime:
          cronParts.type === "daily" || cronParts.type === "weekly"
            ? dayjs()
                .hour(cronParts.hour ?? 9)
                .minute(cronParts.minute ?? 0)
            : DEFAULT_FORM_VALUES.cronTime,
        cronDaysOfWeek:
          cronParts.type === "weekly" ? cronParts.daysOfWeek : undefined,
        cronCustom: cronParts.type === "custom" ? cronParts.rawCron : undefined,
      };
      reset(formValues);
    } else {
      reset({
        ...DEFAULT_FORM_VALUES,
        schedule: {
          ...DEFAULT_FORM_VALUES.schedule,
          timezone: defaultTimezone || "UTC",
        },
      });
    }
  }, [open, editingJob, reset, setValue, defaultTimezone]);

  const cronType = watch("cronType");
  const scheduleMode = watch("scheduleMode");
  const taskType = watch("task_type");
  const repeatEnabled = watch("repeatEnabled");
  const repeatEndType = watch("repeatEndType");

  const basicGroup = (
    <div className={styles.formGroup}>
      <h4 className={styles.formGroupTitle}>{t("cronJobs.groupBasic")}</h4>

      <FormField
        label={t("cronJobs.name")}
        error={errors.name?.message as string}
        required
      >
        <Controller
          name="name"
          control={control}
          render={({ field }) => (
            <InputWithExample
              value={field.value}
              onChange={(e: any) => field.onChange(e.target.value)}
              placeholder={t("cronJobs.jobNamePlaceholder")}
            />
          )}
        />
      </FormField>

      <FormField label={t("cronJobs.enabled")}>
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
        label={t("cronJobs.taskType")}
        error={errors.task_type?.message as string}
        required
      >
        <Controller
          name="task_type"
          control={control}
          render={({ field }) => (
            <RadioGroup value={field.value} onChange={field.onChange}>
              <Radio value="text">{t("cronJobs.taskTypeText")}</Radio>
              <Radio value="agent">{t("cronJobs.taskTypeAgent")}</Radio>
            </RadioGroup>
          )}
        />
      </FormField>
    </div>
  );

  const triggerGroup = (
    <div className={styles.formGroup}>
      <h4 className={styles.formGroupTitle}>{t("cronJobs.groupTrigger")}</h4>

      <FormField label={t("cronJobs.scheduleType")} required>
        <Controller
          name="scheduleMode"
          control={control}
          render={({ field }) => (
            <RadioGroup value={field.value} onChange={field.onChange}>
              <Radio value="cron">{t("cronJobs.scheduleTypeCron")}</Radio>
              <Radio value="once">{t("cronJobs.scheduleTypeOnce")}</Radio>
            </RadioGroup>
          )}
        />
      </FormField>

      {scheduleMode === "cron" && (
        <>
          <FormField label={t("cronJobs.scheduleCronLabel")} required>
            <Controller
              name="cronType"
              control={control}
              render={({ field }) => (
                <RadioGroup
                  className={styles.cronTypeRadio}
                  value={field.value}
                  onChange={field.onChange}
                >
                  <Radio value="hourly">{t("cronJobs.cronTypeHourly")}</Radio>
                  <Radio value="daily">{t("cronJobs.cronTypeDaily")}</Radio>
                  <Radio value="weekly">{t("cronJobs.cronTypeWeekly")}</Radio>
                  <Radio value="custom">{t("cronJobs.cronTypeCustom")}</Radio>
                </RadioGroup>
              )}
            />
          </FormField>

          {(cronType === "daily" || cronType === "weekly") && (
            <FormField
              label={t("cronJobs.cronTime")}
              error={errors.cronTime?.message as string}
              required
            >
              <Controller
                name="cronTime"
                control={control}
                render={({ field }) => (
                  <TimePicker
                    value={field.value}
                    onChange={(time: any) => field.onChange(time)}
                    format="HH:mm"
                    minuteStep={15}
                    needConfirm={false}
                    style={{ width: "100%" }}
                  />
                )}
              />
            </FormField>
          )}

          {cronType === "weekly" && (
            <FormField
              label={t("cronJobs.cronDaysOfWeek")}
              error={errors.cronDaysOfWeek?.message as string}
              required
            >
              <Controller
                name="cronDaysOfWeek"
                control={control}
                render={({ field }) => (
                  <div style={{ display: "flex", flexWrap: "wrap", gap: 8 }}>
                    {[
                      { label: t("cronJobs.cronDayMon"), value: "mon" },
                      { label: t("cronJobs.cronDayTue"), value: "tue" },
                      { label: t("cronJobs.cronDayWed"), value: "wed" },
                      { label: t("cronJobs.cronDayThu"), value: "thu" },
                      { label: t("cronJobs.cronDayFri"), value: "fri" },
                      { label: t("cronJobs.cronDaySat"), value: "sat" },
                      { label: t("cronJobs.cronDaySun"), value: "sun" },
                    ].map((opt) => (
                      <Check
                        key={opt.value}
                        checked={(field.value || []).includes(opt.value)}
                        onChange={(e) => {
                          const vals = field.value || [];
                          field.onChange(
                            e.target.checked
                              ? [...vals, opt.value]
                              : vals.filter((v: string) => v !== opt.value),
                          );
                        }}
                      >
                        {opt.label}
                      </Check>
                    ))}
                  </div>
                )}
              />
            </FormField>
          )}

          {cronType === "custom" && (
            <FormField
              label={t("cronJobs.cronCustomExpression")}
              error={errors.cronCustom?.message as string}
              required
            >
              <Controller
                name="cronCustom"
                control={control}
                render={({ field }) => (
                  <InputWithExample
                    value={field.value}
                    onChange={(e: any) => field.onChange(e.target.value)}
                    placeholder="0 9 * * *"
                  />
                )}
              />
              <div className={styles.formExtraText}>
                <div style={{ marginBottom: 4 }}>
                  {t("cronJobs.cronExample")}
                </div>
                <div>
                  {t("cronJobs.cronHelper")}{" "}
                  <a
                    href="https://crontab.guru/"
                    target="_blank"
                    rel="noopener noreferrer"
                    className={styles.formHelperLink}
                  >
                    {t("cronJobs.cronHelperLink")} →
                  </a>
                </div>
              </div>
            </FormField>
          )}
        </>
      )}

      {scheduleMode === "once" && (
        <>
          <FormField
            label={t("cronJobs.onceDate")}
            error={(errors as any).onceDate?.message as string}
            required
          >
            <Controller
              name="onceDate"
              control={control}
              render={({ field }) => (
                <DatePicker
                  value={field.value}
                  onChange={field.onChange}
                  placeholder="YYYY-MM-DD"
                />
              )}
            />
          </FormField>

          <FormField
            label={t("cronJobs.onceTime")}
            error={(errors as any).onceTime?.message as string}
            required
          >
            <Controller
              name="onceTime"
              control={control}
              render={({ field }) => (
                <TimePicker
                  value={field.value}
                  onChange={field.onChange}
                  format="HH:mm"
                  minuteStep={15}
                  style={{ width: "100%" }}
                />
              )}
            />
          </FormField>

          <FormField label={t("cronJobs.repeatEnabled")}>
            <Controller
              name="repeatEnabled"
              control={control}
              render={({ field }) => (
                <Toggle
                  checked={!!field.value}
                  onChange={(e) => field.onChange(e.target.checked)}
                />
              )}
            />
          </FormField>

          {repeatEnabled && (
            <>
              <FormField label={t("cronJobs.repeatEveryDays")}>
                <Controller
                  name="repeatEveryDays"
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

              <FormField label={t("cronJobs.repeatEndType")}>
                <Controller
                  name="repeatEndType"
                  control={control}
                  render={({ field }) => (
                    <Select value={field.value} onChange={field.onChange}>
                      <Select.Option value="never">
                        {t("cronJobs.repeatEndNever")}
                      </Select.Option>
                      <Select.Option value="until">
                        {t("cronJobs.repeatEndUntil")}
                      </Select.Option>
                      <Select.Option value="count">
                        {t("cronJobs.repeatEndCount")}
                      </Select.Option>
                    </Select>
                  )}
                />
              </FormField>

              {repeatEndType === "until" && (
                <FormField
                  label={t("cronJobs.repeatUntilDate")}
                  error={(errors as any).repeatUntilDate?.message as string}
                  required
                >
                  <Controller
                    name="repeatUntilDate"
                    control={control}
                    render={({ field }) => (
                      <DatePicker
                        value={field.value}
                        onChange={field.onChange}
                        placeholder="YYYY-MM-DD"
                      />
                    )}
                  />
                </FormField>
              )}

              {repeatEndType === "count" && (
                <FormField
                  label={t("cronJobs.repeatCount")}
                  error={(errors as any).repeatCount?.message as string}
                  required
                >
                  <Controller
                    name="repeatCount"
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
              )}
            </>
          )}
        </>
      )}

      <FormField label={t("cronJobs.scheduleTimezone")}>
        <Controller
          name="schedule.timezone"
          control={control}
          render={({ field }) => (
            <Select
              showSearch
              placeholder={t("cronJobs.selectTimezone")}
              filterOption={(input: string, option: any) =>
                (option?.label?.toString() || "")
                  .toLowerCase()
                  .includes(input.toLowerCase())
              }
              options={TIMEZONE_OPTIONS}
              value={field.value}
              onChange={field.onChange}
            />
          )}
        />
      </FormField>
    </div>
  );

  const contentGroup = (
    <div className={styles.formGroup}>
      <h4 className={styles.formGroupTitle}>{t("cronJobs.groupContent")}</h4>

      {taskType === "text" ? (
        <FormField
          label={t("cronJobs.text")}
          error={errors.text?.message as string}
          required
        >
          <Controller
            name="text"
            control={control}
            render={({ field }) => (
              <TextArea
                value={field.value}
                onChange={(e: React.ChangeEvent<HTMLTextAreaElement>) =>
                  field.onChange(e.target.value)
                }
                rows={4}
                placeholder={t("cronJobs.taskDescriptionPlaceholder")}
              />
            )}
          />
        </FormField>
      ) : (
        <FormField
          label={t("cronJobs.requestInput")}
          error={(errors as any).request?.input?.message as string}
          required
        >
          <Controller
            name="request.input"
            control={control}
            render={({ field }) => (
              <TextArea
                value={field.value}
                onChange={(e: React.ChangeEvent<HTMLTextAreaElement>) =>
                  field.onChange(e.target.value)
                }
                rows={6}
                placeholder='[{"role":"user","content":[{"text":"Hello","type":"text"}]}]'
                style={{ fontFamily: "var(--font-mono)", fontSize: 12 }}
              />
            )}
          />
          <span className={styles.formExtraText}>
            {t("cronJobs.requestInputExample")}
          </span>
        </FormField>
      )}

      <FormField label={t("cronJobs.saveResultToInbox")}>
        <Controller
          name="save_result_to_inbox"
          control={control}
          render={({ field }) => (
            <Toggle
              checked={!!field.value}
              onChange={(e) => field.onChange(e.target.checked)}
            />
          )}
        />
      </FormField>
    </div>
  );

  const collapsedAdvanced = (
    <div className={styles.formCollapse}>
      <details className="rua-collapse-item">
        <summary>
          <span className={styles.collapseLabel}>
            {t("cronJobs.groupDispatch")}
            <span className={styles.collapseLabelHint}>
              {t("cronJobs.groupDispatchHint")}
            </span>
          </span>
        </summary>
        <div className="rua-collapse-item__content">
          <FormField label={t("cronJobs.dispatchTargetPicker")}>
            <Select
              showSearch
              placeholder={t("cronJobs.dispatchTargetPickerPlaceholder")}
              filterOption={(input: string, option: any) =>
                (option?.label?.toString() || "")
                  .toLowerCase()
                  .includes(input.toLowerCase())
              }
              options={dispatchTargets.map((target) => ({
                label: `${target.channel} · ${target.user_id} · ${target.session_id}`,
                value: `${target.channel}\t${target.user_id}\t${target.session_id}`,
              }))}
              onChange={(value: string) => {
                const [channel, userId, sessionId] = String(value).split("\t");
                if (!channel || !userId || !sessionId) return;
                setValue("dispatch.channel", channel);
                setValue("dispatch.target.user_id", userId);
                setValue("dispatch.target.session_id", sessionId);
              }}
            />
          </FormField>

          <FormField
            label={t("cronJobs.dispatchChannel")}
            error={(errors as any).dispatch?.channel?.message as string}
            required
          >
            <Controller
              name="dispatch.channel"
              control={control}
              render={({ field }) => (
                <InputWithExample
                  value={field.value}
                  onChange={(e: any) => field.onChange(e.target.value)}
                  placeholder="console"
                />
              )}
            />
          </FormField>

          <FormField
            label={t("cronJobs.dispatchTargetUserId")}
            error={(errors as any).dispatch?.target?.user_id?.message as string}
            required
          >
            <Controller
              name="dispatch.target.user_id"
              control={control}
              render={({ field }) => (
                <InputWithExample
                  value={field.value}
                  onChange={(e: any) => field.onChange(e.target.value)}
                  placeholder="default"
                />
              )}
            />
          </FormField>

          <FormField
            label={t("cronJobs.dispatchTargetSessionId")}
            error={
              (errors as any).dispatch?.target?.session_id?.message as string
            }
            required
          >
            <Controller
              name="dispatch.target.session_id"
              control={control}
              render={({ field }) => (
                <InputWithExample
                  value={field.value}
                  onChange={(e: any) => field.onChange(e.target.value)}
                  placeholder="default"
                />
              )}
            />
          </FormField>

          <FormField label={t("cronJobs.dispatchMode")}>
            <Controller
              name="dispatch.mode"
              control={control}
              render={({ field }) => (
                <Select value={field.value} onChange={field.onChange}>
                  <Select.Option value="stream">stream</Select.Option>
                  <Select.Option value="final">final</Select.Option>
                </Select>
              )}
            />
          </FormField>
        </div>
      </details>
      <details className="rua-collapse-item">
        <summary>
          <span className={styles.collapseLabel}>
            {t("cronJobs.groupRuntime")}
            <span className={styles.collapseLabelHint}>
              {t("cronJobs.groupRuntimeHint")}
            </span>
          </span>
        </summary>
        <div className="rua-collapse-item__content">
          <FormField label={t("cronJobs.runtimeMaxConcurrency")}>
            <Controller
              name="runtime.max_concurrency"
              control={control}
              render={({ field }) => (
                <NumberInput
                  min={1}
                  style={{ width: "100%" }}
                  placeholder="1"
                  value={field.value}
                  onChange={field.onChange}
                />
              )}
            />
          </FormField>

          <FormField label={t("cronJobs.runtimeTimeoutSeconds")}>
            <Controller
              name="runtime.timeout_seconds"
              control={control}
              render={({ field }) => (
                <NumberInput
                  min={1}
                  style={{ width: "100%" }}
                  placeholder="300"
                  value={field.value}
                  onChange={field.onChange}
                />
              )}
            />
          </FormField>

          <FormField label={t("cronJobs.runtimeMisfireGraceSeconds")}>
            <Controller
              name="runtime.misfire_grace_seconds"
              control={control}
              render={({ field }) => (
                <NumberInput
                  min={0}
                  style={{ width: "100%" }}
                  placeholder="60"
                  value={field.value}
                  onChange={field.onChange}
                />
              )}
            />
          </FormField>

          <FormField label={t("cronJobs.runtimeShareSession")}>
            <Controller
              name="runtime.share_session"
              control={control}
              render={({ field }) => (
                <Toggle
                  checked={field.value !== false}
                  onChange={(e) => field.onChange(e.target.checked)}
                />
              )}
            />
          </FormField>

          <FormField label={t("cronJobs.runtimeToolSafety")}>
            <Controller
              name="runtime.tool_safety"
              control={control}
              render={({ field }) => (
                <Toggle
                  checked={!!field.value}
                  onChange={(e) => field.onChange(e.target.checked)}
                />
              )}
            />
          </FormField>
        </div>
      </details>
    </div>
  );

  return (
    <Drawer
      placement="right"
      title={editingJob ? t("cronJobs.editJob") : t("cronJobs.createJob")}
      open={open}
      onClose={onClose}
      destroyOnClose
      footer={
        <div className={styles.formActions}>
          <Btn variant="outline" onClick={onClose}>
            {t("common.cancel")}
          </Btn>
          <Btn loading={saving} onClick={handleSubmit(onSubmit)}>
            {t("common.save")}
          </Btn>
        </div>
      }
    >
      <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
        {basicGroup}
        {triggerGroup}
        {contentGroup}
        {collapsedAdvanced}
      </div>
    </Drawer>
  );
}
