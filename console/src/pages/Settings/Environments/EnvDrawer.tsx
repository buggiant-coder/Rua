import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { Btn, Drawer, FormField, TextArea, TextInput } from "@/ui";
import type { EnvVar } from "../../../api/types";

interface EnvDrawerProps {
  open: boolean;
  editingEnv: EnvVar | null;
  existingKeys: string[];
  saving: boolean;
  onClose: () => void;
  onSubmit: (values: { key: string; value: string }) => void;
}

const KEY_PATTERN = /^[A-Za-z_][A-Za-z0-9_]*$/;

export function EnvDrawer({
  open,
  editingEnv,
  existingKeys,
  saving,
  onClose,
  onSubmit,
}: EnvDrawerProps) {
  const { t } = useTranslation();

  const schema = z.object({
    key: z
      .string()
      .min(1, t("environments.keyRequired"))
      .refine((v: string) => KEY_PATTERN.test(v.trim()), {
        message: t("environments.invalidKeyFormat"),
      })
      .refine(
        (v: string) => {
          const trimmed = v.trim();
          return !(
            existingKeys.includes(trimmed) && trimmed !== editingEnv?.key
          );
        },
        { message: t("environments.duplicateKey") },
      ),
    value: z.string(),
  });

  const {
    register,
    handleSubmit,
    reset,
    formState: { errors },
  } = useForm<{ key: string; value: string }>({
    resolver: zodResolver(schema),
    defaultValues: { key: "", value: "" },
  });

  useEffect(() => {
    if (!open) return;
    if (editingEnv) {
      reset({ key: editingEnv.key, value: editingEnv.value });
    } else {
      reset({ key: "", value: "" });
    }
  }, [open, editingEnv, reset]);

  const onValid = (values: { key: string; value: string }) => {
    onSubmit({ key: values.key.trim(), value: values.value ?? "" });
  };

  return (
    <Drawer
      title={
        editingEnv
          ? t("environments.editTitle", { name: editingEnv.key })
          : t("environments.createTitle")
      }
      open={open}
      onClose={onClose}
      footer={
        <>
          <Btn variant="outline" onClick={onClose}>
            {t("common.cancel")}
          </Btn>
          <Btn loading={saving} onClick={handleSubmit(onValid)}>
            {t("common.save")}
          </Btn>
        </>
      }
    >
      <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
        <FormField
          label={t("environments.key")}
          error={errors.key?.message}
          required
        >
          <TextInput {...register("key")} placeholder="MY_VAR" autoFocus />
        </FormField>
        <FormField
          label={t("environments.value")}
          error={errors.value?.message}
        >
          <TextArea
            {...register("value")}
            rows={4}
            placeholder={t("environments.valuePlaceholder")}
          />
        </FormField>
      </div>
    </Drawer>
  );
}
