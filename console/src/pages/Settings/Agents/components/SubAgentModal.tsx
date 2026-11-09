import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { useForm, Controller } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import type { SubAgentTemplate } from "@/api/types/subagentTemplates";
import type { AgentSummary } from "@/api/types/agents";
import { FormField, Modal, Select, TextArea, TextInput } from "@/ui";

const AVAILABLE_TOOLS = [
  "Bash",
  "Read",
  "Write",
  "Edit",
  "Grep",
  "Glob",
  "web_fetch",
  "web_search",
  "view_image",
  "view_video",
];

export interface SubAgentFormValues {
  name: string;
  description: string;
  system_prompt: string;
  tools: string[];
  visible_to: string[];
}

interface SubAgentModalProps {
  open: boolean;
  editingTemplate: SubAgentTemplate | null;
  agents: AgentSummary[];
  onSave: (values: SubAgentFormValues) => Promise<void>;
  onClose: () => void;
}

export function SubAgentModal({
  open,
  editingTemplate,
  agents,
  onSave,
  onClose,
}: SubAgentModalProps) {
  const { t } = useTranslation();

  const schema = z.object({
    name: z
      .string()
      .min(1, t("subAgent.nameRequired"))
      .regex(/^[a-zA-Z0-9_-]{1,32}$/, t("subAgent.namePattern")),
    description: z.string(),
    system_prompt: z.string().min(1, t("subAgent.systemPromptRequired")),
    tools: z.array(z.string()),
    visible_to: z.array(z.string()),
  });

  const {
    register,
    handleSubmit,
    reset,
    control,
    formState: { errors },
  } = useForm<SubAgentFormValues>({
    resolver: zodResolver(schema),
    defaultValues: {
      name: "",
      description: "",
      system_prompt: "",
      tools: [],
      visible_to: [],
    },
  });

  useEffect(() => {
    if (!open) return;
    if (editingTemplate) {
      reset({
        name: editingTemplate.name ?? "",
        description: editingTemplate.description ?? "",
        system_prompt: editingTemplate.system_prompt ?? "",
        tools: editingTemplate.tools ?? [],
        visible_to: editingTemplate.visible_to ?? [],
      });
    } else {
      reset({
        name: "",
        description: "",
        system_prompt: "",
        tools: [],
        visible_to: [],
      });
    }
  }, [open, editingTemplate?.id, reset]);

  return (
    <Modal
      className="modal-fullscreen-mobile"
      title={
        editingTemplate ? t("subAgent.editTitle") : t("subAgent.createTitle")
      }
      open={open}
      onOk={handleSubmit((values) => onSave(values))}
      onClose={onClose}
      width={640}
      okText={t("common.save")}
      cancelText={t("common.cancel")}
    >
      <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
        <FormField
          label={t("subAgent.name")}
          error={errors.name?.message}
          required
        >
          <TextInput
            {...register("name")}
            placeholder={t("subAgent.namePlaceholder")}
            disabled={!!editingTemplate}
          />
        </FormField>

        <FormField
          label={t("subAgent.description")}
          error={errors.description?.message}
        >
          <TextArea
            {...register("description")}
            placeholder={t("subAgent.descriptionPlaceholder")}
            rows={2}
          />
        </FormField>

        <FormField
          label={t("subAgent.systemPrompt")}
          error={errors.system_prompt?.message}
          required
        >
          <TextArea
            {...register("system_prompt")}
            placeholder={t("subAgent.systemPromptPlaceholder")}
            rows={6}
          />
        </FormField>

        <FormField label={t("subAgent.tools")} error={errors.tools?.message}>
          <Controller
            name="tools"
            control={control}
            render={({ field }) => (
              <Select
                {...field}
                mode="multiple"
                placeholder={t("subAgent.toolsPlaceholder")}
                options={AVAILABLE_TOOLS.map((tool) => ({
                  value: tool,
                  label: tool,
                }))}
              />
            )}
          />
        </FormField>

        <FormField
          label={t("subAgent.visibleTo")}
          error={errors.visible_to?.message}
        >
          <Controller
            name="visible_to"
            control={control}
            render={({ field }) => (
              <Select
                {...field}
                mode="multiple"
                allowClear
                placeholder={t("subAgent.visibleToAll")}
                options={agents.map((agent) => ({
                  value: agent.id,
                  label: agent.name || agent.id,
                }))}
              />
            )}
          />
          <div style={{ fontSize: 12, color: "var(--text-3)", marginTop: 2 }}>
            {t("subAgent.visibleToHelp")}
          </div>
        </FormField>
      </div>
    </Modal>
  );
}
