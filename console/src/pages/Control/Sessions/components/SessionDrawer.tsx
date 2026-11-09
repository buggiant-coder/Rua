import { useEffect } from "react";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { useTranslation } from "react-i18next";
import type { Session } from "./constants";
import { Btn, Drawer, FormField, TextInput } from "@/ui";
const schema = z.object({
  name: z.string().optional(),
});

type FormValues = z.infer<typeof schema>;

interface SessionDrawerProps {
  open: boolean;
  editingSession: Session | null;
  saving: boolean;
  onClose: () => void;
  onSubmit: (values: Session) => void;
}

export function SessionDrawer({
  open,
  editingSession,
  saving,
  onClose,
  onSubmit,
}: SessionDrawerProps) {
  const { t } = useTranslation();

  const {
    register,
    handleSubmit,
    reset,
    formState: { errors },
  } = useForm<FormValues>({
    resolver: zodResolver(schema),
    defaultValues: { name: "" },
  });

  useEffect(() => {
    if (open && editingSession) {
      reset({ name: editingSession.name || "" });
    }
  }, [open, editingSession, reset]);

  const onValid = (values: FormValues) => {
    if (!editingSession) return;
    onSubmit({ ...editingSession, name: values.name || "" });
  };

  const drawerFooter = (
    <>
      <Btn variant="outline" onClick={onClose}>
        {t("common.cancel")}
      </Btn>
      <Btn loading={saving} onClick={handleSubmit(onValid)}>
        {t("common.save")}
      </Btn>
    </>
  );

  return (
    <Drawer
      placement="right"
      title={t("sessions.editSession")}
      open={open}
      onClose={onClose}
      destroyOnClose
      footer={drawerFooter}
    >
      <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
        <FormField label={t("sessions.fieldName")} error={errors.name?.message}>
          <TextInput
            {...register("name")}
            placeholder={t("sessions.sessionNamePlaceholder")}
          />
        </FormField>

        {editingSession && (
          <>
            <FormField label={t("sessions.fieldId")}>
              <TextInput value={editingSession.id} disabled />
            </FormField>

            <FormField label={t("sessions.fieldUserId")}>
              <TextInput value={editingSession.user_id} disabled />
            </FormField>

            <FormField label={t("sessions.fieldChannel")}>
              <TextInput value={editingSession.channel} disabled />
            </FormField>
          </>
        )}
      </div>
    </Drawer>
  );
}
