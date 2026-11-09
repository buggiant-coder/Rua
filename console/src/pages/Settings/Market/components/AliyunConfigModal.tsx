import { useCallback, useState } from "react";
import { useTranslation } from "react-i18next";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import api from "../../../../api";
import { FormField, Modal, TextInput, toast } from "@/ui";

interface AliyunConfigModalProps {
  open: boolean;
  onClose: () => void;
  onSuccess: () => void;
}

export function AliyunConfigModal({
  open,
  onClose,
  onSuccess,
}: AliyunConfigModalProps) {
  const { t } = useTranslation();
  const [saving, setSaving] = useState(false);

  const schema = z.object({
    accessKeyId: z
      .string()
      .min(1, t("market.aliyunConfig.accessKeyIdRequired")),
    accessKeySecret: z
      .string()
      .min(1, t("market.aliyunConfig.accessKeySecretRequired")),
  });

  const {
    register,
    handleSubmit,
    reset,
    formState: { errors },
  } = useForm<{ accessKeyId: string; accessKeySecret: string }>({
    resolver: zodResolver(schema),
    defaultValues: { accessKeyId: "", accessKeySecret: "" },
  });

  const onFinish = useCallback(
    async (values: { accessKeyId: string; accessKeySecret: string }) => {
      setSaving(true);
      try {
        const existing = (await api.listEnvs()) ?? [];
        const dict: Record<string, string> = {};
        for (const e of existing) {
          dict[e.key] = e.value ?? "";
        }
        dict["ALIBABA_CLOUD_ACCESS_KEY_ID"] = values.accessKeyId.trim();
        dict["ALIBABA_CLOUD_ACCESS_KEY_SECRET"] = values.accessKeySecret.trim();
        await api.saveEnvs(dict);
        toast.success(t("market.aliyunConfig.saveSuccess"));
        reset();
        onClose();
        onSuccess();
      } catch (err) {
        const msg =
          err instanceof Error
            ? err.message
            : t("market.aliyunConfig.saveFailed");
        toast.error(msg);
      } finally {
        setSaving(false);
      }
    },
    [onClose, onSuccess, t, reset],
  );

  const handleOk = handleSubmit(onFinish);

  const handleCancel = useCallback(() => {
    reset();
    onClose();
  }, [reset, onClose]);

  return (
    <Modal
      title={t("market.aliyunConfig.title")}
      open={open}
      onOk={handleOk}
      onClose={handleCancel}
      okText={t("common.save")}
      cancelText={t("common.cancel")}
      confirmLoading={saving}
      destroyOnClose
      width={480}
    >
      <div
        style={{
          marginTop: 16,
          display: "flex",
          flexDirection: "column",
          gap: 12,
        }}
      >
        <FormField
          label={t("market.aliyunConfig.accessKeyId")}
          error={errors.accessKeyId?.message}
          required
        >
          <TextInput {...register("accessKeyId")} placeholder="LTAI..." />
        </FormField>
        <FormField
          label={t("market.aliyunConfig.accessKeySecret")}
          error={errors.accessKeySecret?.message}
          required
        >
          <TextInput
            {...register("accessKeySecret")}
            type="password"
            placeholder="••••••••"
          />
        </FormField>
      </div>
    </Modal>
  );
}
