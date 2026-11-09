import { useForm, Controller } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { useCallback, useEffect, useRef, useState } from "react";
import type {
  ChannelSchema,
  ChannelSchemaField,
} from "../../../../api/types/channel";
import { ChannelSchemaFields } from "./ChannelSchemaFields";
import { channelFormValues } from "./channelForm";
import api from "../../../../api";
import { getChannelLabel, type ChannelKey } from "./constants";
import { useChannelQrcode } from "./useChannelQrcode";
import styles from "../index.module.less";
import { openExternal } from "../../../../utils/openExternal";
import { Link } from "lucide-react";
import { Btn, Drawer, FormField, InfoBox, Modal, Toggle, toast } from "@/ui";

interface ConflictResponse {
  conflict: boolean;
  conflicting_agent_id: string;
  conflicting_agent_name: string;
  channel_type: string;
  identifier: string;
}

function isConflictResponse(data: unknown): data is ConflictResponse {
  return (
    typeof data === "object" &&
    data !== null &&
    (data as Record<string, unknown>).conflict === true
  );
}

interface ChannelDrawerProps {
  open: boolean;
  activeKey: ChannelKey | null;
  activeLabel: string;
  saving: boolean;
  initialValues: Record<string, unknown> | undefined;
  isBuiltin: boolean;
  schema?: ChannelSchema;
  onClose: () => void;
  onSubmit: (values: Record<string, unknown>) => void;
  onSaved?: () => void;
}

export function ChannelDrawer({
  open,
  activeKey,
  activeLabel,
  saving,
  initialValues,
  isBuiltin,
  schema,
  onClose,
  onSubmit,
  onSaved,
}: ChannelDrawerProps) {
  const { t, i18n } = useTranslation();
  const currentLang = i18n.language?.startsWith("zh") ? "zh" : "en";
  const label = activeKey ? getChannelLabel(activeKey, t) : activeLabel;
  const [conflictSaving, setConflictSaving] = useState(false);
  const [applicationError, setApplicationError] = useState<string | null>(null);
  const [reapplying, setReapplying] = useState(false);
  const reapplyingRef = useRef(false);
  const currentKey = useRef(activeKey);
  currentKey.current = activeKey;

  const { control, handleSubmit, reset, setValue, watch } = useForm<
    Record<string, any>
  >({
    defaultValues: channelFormValues(schema, initialValues),
  });

  useEffect(() => {
    if (open) reset(channelFormValues(schema, initialValues));
    setApplicationError(null);
  }, [open, activeKey, initialValues, schema, reset]);

  const handleFormSubmit = useCallback(
    async (values: Record<string, any>) => {
      if (!activeKey) {
        onSubmit(values);
        return;
      }
      const { isBuiltin: _ib, ...cleanValues } = values;
      for (const spec of schema?.fields ?? []) {
        if (
          spec.default !== null &&
          typeof spec.default === "object" &&
          !Array.isArray(spec.default) &&
          typeof cleanValues[spec.name] === "string"
        ) {
          cleanValues[spec.name] = JSON.parse(cleanValues[spec.name]);
        }
      }
      if (activeKey === "console") {
        cleanValues.enabled = true;
      }
      try {
        const result = await api.updateChannelConfig(activeKey, cleanValues);
        if (currentKey.current !== activeKey) return;
        if (isConflictResponse(result)) {
          Modal.confirm({
            title: t("channels.conflictTitle", {
              defaultValue: "Channel Conflict Detected",
            }),
            content: t("channels.conflictMessage", {
              defaultValue: `This identifier is already used by agent "${result.conflicting_agent_name}". Continuing will disable the ${result.channel_type} channel on that agent.`,
              agent_name: result.conflicting_agent_name,
              channel_type: result.channel_type,
            }),
            okText: t("channels.conflictConfirm", {
              defaultValue: "Continue",
            }),
            cancelText: t("common.cancel"),
            onOk: async () => {
              setConflictSaving(true);
              try {
                await api.updateChannelConfig(activeKey, {
                  ...cleanValues,
                  force: true,
                });
                toast.success(t("channels.configSaved"));
                onSaved?.();
              } catch {
                toast.error(t("channels.configFailed"));
              } finally {
                setConflictSaving(false);
              }
            },
          });
          return;
        }
        toast.success(t("channels.configSaved"));
        onSaved?.();
      } catch (error) {
        if (currentKey.current !== activeKey) return;
        const message =
          error instanceof Error ? error.message : t("channels.configFailed");
        toast.error(message);
        if (message.includes("configuration saved"))
          setApplicationError(message);
      }
    },
    [activeKey, schema, t, onSaved, onSubmit],
  );

  const weixinQrcode = useChannelQrcode({
    channel: "weixin",
    successStatus: "confirmed",
    successCredentialKey: "bot_token",
    pollInterval: 2000,
    onSuccess: useCallback(
      (credentials: Record<string, string>) => {
        setValue("bot_token", credentials.bot_token);
        toast.success(t("channels.weixinLoginSuccess"));
      },
      [setValue, t],
    ),
    onError: useCallback(
      (type: "fetch" | "expired" | "fail") => {
        if (type === "expired") {
          toast.warning(t("channels.weixinQrcodeExpired"));
        } else {
          toast.error(t("channels.weixinQrcodeFailed"));
        }
      },
      [t],
    ),
  });

  const feishuDomain = watch("domain") || "feishu";

  const dingtalkQrcode = useChannelQrcode({
    channel: "dingtalk",
    successStatus: "success",
    successCredentialKey: "client_id",
    pollInterval: 5000,
    onSuccess: useCallback(
      (credentials: Record<string, string>) => {
        setValue("client_id", credentials.client_id);
        setValue("client_secret", credentials.client_secret);
        toast.success(t("channels.dingtalkAuthSuccess"));
      },
      [setValue, t],
    ),
    onExpired: useCallback(() => {
      toast.warning(t("channels.dingtalkQrcodeExpired"));
    }, [t]),
    onError: useCallback(
      (type: "fetch" | "expired" | "fail") => {
        if (type === "expired") {
          toast.warning(t("channels.dingtalkQrcodeExpired"));
        } else {
          toast.error(t("channels.dingtalkQrcodeFailed"));
        }
      },
      [t],
    ),
  });

  const feishuQrcode = useChannelQrcode({
    channel: "feishu",
    successStatus: "success",
    successCredentialKey: "app_id",
    pollInterval: 2000,
    params: { domain: feishuDomain },
    onSuccess: useCallback(
      (credentials: Record<string, string>) => {
        setValue("app_id", credentials.app_id);
        setValue("app_secret", credentials.app_secret);
        toast.success(t("channels.feishuAuthSuccess"));
      },
      [setValue, t],
    ),
    onExpired: useCallback(() => {
      toast.warning(t("channels.feishuQrcodeExpired"));
    }, [t]),
    onError: useCallback(
      (type: "fetch" | "expired" | "fail") => {
        if (type === "expired") {
          toast.warning(t("channels.feishuQrcodeExpired"));
        } else {
          toast.error(t("channels.feishuQrcodeFailed"));
        }
      },
      [t],
    ),
  });

  const wecomQrcode = useChannelQrcode({
    channel: "wecom",
    successStatus: "success",
    successCredentialKey: "bot_id",
    pollInterval: 3000,
    onSuccess: useCallback(
      (credentials: Record<string, string>) => {
        setValue("bot_id", credentials.bot_id);
        setValue("secret", credentials.secret);
        toast.success(t("channels.wecomAuthSuccess"));
      },
      [setValue, t],
    ),
    onError: useCallback(
      (_type: "fetch" | "expired" | "fail") => {
        toast.error(t("channels.wecomQrcodeFailed"));
      },
      [t],
    ),
  });

  useEffect(() => {
    if (!open || (activeKey !== "wechat" && activeKey !== "weixin"))
      weixinQrcode.reset();
    if (!open || activeKey !== "dingtalk") dingtalkQrcode.reset();
    if (!open || activeKey !== "feishu") feishuQrcode.reset();
    if (!open || activeKey !== "wecom") wecomQrcode.reset();
  }, [
    open,
    activeKey,
    weixinQrcode.reset,
    dingtalkQrcode.reset,
    feishuQrcode.reset,
    wecomQrcode.reset,
  ]);

  const qr =
    activeKey === "wechat" || activeKey === "weixin"
      ? weixinQrcode
      : activeKey === "dingtalk"
      ? dingtalkQrcode
      : activeKey === "feishu"
      ? feishuQrcode
      : activeKey === "wecom"
      ? wecomQrcode
      : undefined;
  const qrKey = activeKey === "wechat" ? "weixin" : activeKey;
  const fields: ChannelSchemaField[] =
    schema?.fields ??
    (isBuiltin
      ? []
      : Object.entries(initialValues ?? {})
          .filter(([name]) => name !== "enabled" && name !== "isBuiltin")
          .map(([name, value]) => ({
            name,
            label: { en: name, zh: name },
            required: false,
            secret: /token|secret|password/i.test(name),
            default: value,
            options: [],
            kind:
              typeof value === "boolean"
                ? "toggle"
                : typeof value === "number"
                ? "number"
                : Array.isArray(value)
                ? "tags"
                : typeof value === "object" && value !== null
                ? "textarea"
                : "text",
          })));
  const enabled = activeKey === "console" || Boolean(watch("enabled"));

  const drawerTitle = (
    <div className={styles.drawerTitle}>
      <span>
        {label
          ? `${label} ${t("channels.settings")}`
          : t("channels.channelSettings")}
      </span>
      {schema?.docs[currentLang] && (
        <Btn
          variant="outline"
          size="sm"
          icon={<Link />}
          onClick={() => openExternal(schema.docs[currentLang])}
          className={styles.dingtalkDocBtn}
        >
          {label} Doc
        </Btn>
      )}
    </div>
  );

  const drawerFooter = (
    <div className={styles.formActions}>
      <Btn variant="outline" onClick={onClose}>
        {t("common.cancel")}
      </Btn>
      <Btn
        loading={saving || conflictSaving}
        disabled={isBuiltin && !schema}
        onClick={handleSubmit(handleFormSubmit)}
      >
        {t("common.save")}
      </Btn>
    </div>
  );

  return (
    <Drawer
      placement="right"
      title={drawerTitle}
      open={open}
      onClose={onClose}
      destroyOnClose
      footer={drawerFooter}
    >
      {applicationError && activeKey && (
        <div role="alert">
          <p>{applicationError}</p>
          <Btn
            loading={reapplying}
            onClick={async () => {
              if (reapplyingRef.current) return;
              reapplyingRef.current = true;
              setReapplying(true);
              try {
                await api.reapplyChannel(activeKey);
                if (currentKey.current !== activeKey) return;
                setApplicationError(null);
                onSaved?.();
              } catch (error) {
                toast.error(
                  error instanceof Error
                    ? error.message
                    : t("channels.configFailed"),
                );
              } finally {
                reapplyingRef.current = false;
                setReapplying(false);
              }
            }}
          >
            {t("channels.reapplySaved", {
              defaultValue: "Reapply saved configuration",
            })}
          </Btn>
        </div>
      )}
      {activeKey && (
        <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
          <FormField label={t("common.enabled")}>
            <Controller
              name="enabled"
              control={control}
              render={({ field }) => (
                <Toggle
                  checked={activeKey === "console" ? true : !!field.value}
                  disabled={activeKey === "console"}
                  onChange={field.onChange}
                />
              )}
            />
          </FormField>

          {qr && (
            <FormField
              label={t("channels." + qrKey + "ScanAuth", {
                defaultValue: "Scan to connect",
              })}
            >
              <Btn loading={qr.loading} onClick={qr.fetchQrcode}>
                {t(
                  activeKey === "wecom"
                    ? "channels.loginWeCom"
                    : "channels." + qrKey + "GetQrcode",
                )}
              </Btn>
              {qr.qrcodeImg && (
                <div style={{ textAlign: "center", marginTop: 12 }}>
                  <img
                    src={"data:image/png;base64," + qr.qrcodeImg}
                    alt={label + " QR Code"}
                    width={200}
                    height={200}
                  />
                </div>
              )}
            </FormField>
          )}
          {isBuiltin && !schema ? (
            <InfoBox
              type="info"
              title={t("channels.schemaUnavailable", {
                defaultValue:
                  "Channel settings are loading. Refresh if they do not appear.",
              })}
            />
          ) : (
            <ChannelSchemaFields
              fields={fields.filter((field) => field.name !== "enabled")}
              control={control}
              enabled={enabled}
            />
          )}
        </div>
      )}
    </Drawer>
  );
}
