import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useForm, Controller } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import type { AgentSummary } from "@/api/types/agents";
import { getAgentDisplayName } from "@/utils/agentDisplayName";
import type { PoolSkillSpec } from "@/api/types/skill";
import { skillApi } from "@/api/modules/skill";
import { agentTemplatesApi } from "@/api/modules/agentTemplates";
import type { AgentTemplate } from "@/api/types/agentTemplates";
import { EmptyState } from "@/components/EmptyState";
import { InputWithExample } from "@/components/InputWithExample";
import styles from "../index.module.less";
import { Check } from "lucide-react";
import {
  Btn,
  FormField,
  Loading,
  Modal,
  Select,
  Tag,
  TextArea,
  TextInput,
} from "@/ui";

export interface AgentFormValues {
  id: string;
  name: string;
  description: string;
  workspace_dir: string;
  template_id?: string;
}

interface AgentModalProps {
  open: boolean;
  editingAgent: AgentSummary | null;
  initialValues?: Partial<AgentFormValues>;
  selectedSkills: string[];
  onSelectedSkillsChange: (skills: string[]) => void;
  onInstalledSkillsLoaded: (skills: string[]) => void;
  onSave: (values: AgentFormValues) => Promise<void>;
  onClose: () => void;
}

export function AgentModal({
  open,
  editingAgent,
  initialValues,
  selectedSkills,
  onSelectedSkillsChange,
  onInstalledSkillsLoaded,
  onSave,
  onClose,
}: AgentModalProps) {
  const { t } = useTranslation();
  const [poolSkills, setPoolSkills] = useState<PoolSkillSpec[]>([]);
  const [_installedSkills, setInstalledSkills] = useState<string[]>([]);
  const [loadingSkills, setLoadingSkills] = useState(false);
  const [templates, setTemplates] = useState<AgentTemplate[]>([]);
  const [templateFiles, setTemplateFiles] = useState<string[]>([]);

  const schema = z.object({
    id: z
      .string()
      .refine((v) => !v || /^[a-zA-Z0-9][a-zA-Z0-9_-]*[a-zA-Z0-9]$/.test(v), {
        message: t("agent.idPattern"),
      }),
    name: z.string().min(1, t("agent.nameRequired")),
    description: z.string(),
    workspace_dir: z.string(),
    template_id: z.string().optional(),
  });

  const {
    register,
    handleSubmit,
    reset,
    control,
    setValue,
    watch,
    formState: { errors },
  } = useForm<AgentFormValues>({
    resolver: zodResolver(schema),
    defaultValues: {
      id: "",
      name: "",
      description: "",
      workspace_dir: "",
      template_id: undefined,
    },
  });

  const selectedTemplateId = watch("template_id");

  useEffect(() => {
    if (!open) return;
    if (editingAgent && initialValues) {
      reset({
        id: initialValues.id ?? editingAgent.id ?? "",
        name: initialValues.name ?? "",
        description: initialValues.description ?? "",
        workspace_dir: initialValues.workspace_dir ?? "",
        template_id: initialValues.template_id ?? undefined,
      });
    } else {
      reset({
        id: "",
        name: "",
        description: "",
        workspace_dir: "",
        template_id: undefined,
      });
      setTemplateFiles([]);
    }
  }, [open, editingAgent, initialValues, reset]);

  useEffect(() => {
    if (!open) return;
    setLoadingSkills(true);

    const fetchPool = skillApi.listSkillPoolSkills();
    const fetchInstalled = editingAgent
      ? skillApi.listSkills(editingAgent.id)
      : Promise.resolve([]);
    const fetchTemplates = editingAgent
      ? Promise.resolve([])
      : agentTemplatesApi.list().catch(() => []);

    Promise.all([fetchPool, fetchInstalled, fetchTemplates])
      .then(([pool, workspaceSkills, tmpls]) => {
        const poolSkillNames = new Set(pool.map((skill) => skill.name));
        const installedSkills = workspaceSkills
          .filter((skill) => poolSkillNames.has(skill.name))
          .map((skill) => skill.name);

        setPoolSkills(pool);
        setInstalledSkills(installedSkills);
        onInstalledSkillsLoaded(installedSkills);
        setTemplates(tmpls);
        if (editingAgent) {
          onSelectedSkillsChange(installedSkills);
        } else {
          onSelectedSkillsChange([]);
        }
      })
      .finally(() => setLoadingSkills(false));
  }, [editingAgent, onInstalledSkillsLoaded, onSelectedSkillsChange, open]);

  const handleTemplateChange = async (value: string | undefined) => {
    setValue("template_id", value);

    if (!value) {
      setTemplateFiles([]);
      return;
    }

    try {
      const detail = await agentTemplatesApi.get(value);
      setTemplateFiles(detail.prompt_files || []);
    } catch {
      setTemplateFiles([]);
    }
  };

  const toggleSkill = (name: string) => {
    if (selectedSkills.includes(name)) {
      onSelectedSkillsChange(selectedSkills.filter((s) => s !== name));
    } else {
      onSelectedSkillsChange([...selectedSkills, name]);
    }
  };

  const handleSelectAll = () => {
    const allNames = poolSkills.map((s) => s.name);
    onSelectedSkillsChange(allNames);
  };

  const handleSelectBuiltin = () => {
    const builtinNames = poolSkills
      .filter((s) => s.source === "builtin")
      .map((s) => s.name);
    onSelectedSkillsChange(builtinNames);
  };

  const handleSelectNone = () => {
    onSelectedSkillsChange([]);
  };

  return (
    <Modal
      className="modal-fullscreen-mobile"
      title={
        editingAgent
          ? t("agent.editTitle", {
              name: getAgentDisplayName(editingAgent),
            })
          : t("agent.createTitle")
      }
      open={open}
      onOk={handleSubmit((values) => onSave(values))}
      onClose={onClose}
      width={640}
      okText={t("common.save")}
      cancelText={t("common.cancel")}
    >
      <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
        {!editingAgent && templates.length > 0 && (
          <FormField label={t("template.selectTemplate")}>
            <Select
              allowClear
              placeholder={t("template.selectTemplatePlaceholder")}
              value={selectedTemplateId}
              onChange={handleTemplateChange}
              options={[
                {
                  value: "",
                  label: t("template.noTemplate"),
                },
                ...templates.map((tmpl) => ({
                  value: tmpl.id,
                  label: `${tmpl.name}${tmpl.builtin ? "" : ""}`,
                })),
              ]}
            />
          </FormField>
        )}

        {!editingAgent && templateFiles.length > 0 && (
          <div>
            <span style={{ color: "var(--text-2)", fontSize: 12 }}>
              {t("template.templateWillCopy")}:
            </span>
            <div
              style={{
                marginTop: 4,
                display: "flex",
                flexWrap: "wrap",
                gap: 4,
              }}
            >
              {templateFiles.map((f) => (
                <Tag key={f} style={{ margin: 0 }}>
                  {f.replace(/\.md$/, "")}
                </Tag>
              ))}
            </div>
          </div>
        )}

        {editingAgent ? (
          <FormField label={t("agent.id")}>
            <TextInput
              {...register("id")}
              className={styles.identityReadOnlyInput}
              readOnly
              aria-readonly="true"
            />
          </FormField>
        ) : (
          <FormField label={t("agent.idLabel")} error={errors.id?.message}>
            <TextInput
              {...register("id")}
              placeholder={t("agent.idPlaceholder")}
            />
            <div style={{ fontSize: 12, color: "var(--text-3)", marginTop: 2 }}>
              {t("agent.idHelp")}
            </div>
          </FormField>
        )}

        <FormField
          label={t("agent.name")}
          error={errors.name?.message}
          required
        >
          <TextInput
            {...register("name")}
            placeholder={t("agent.namePlaceholder")}
          />
        </FormField>

        <FormField
          label={t("agent.description")}
          error={errors.description?.message}
        >
          <TextArea
            {...register("description")}
            placeholder={t("agent.descriptionPlaceholder")}
            rows={3}
          />
        </FormField>

        <FormField
          label={t("agent.workspace")}
          error={errors.workspace_dir?.message}
        >
          <Controller
            name="workspace_dir"
            control={control}
            render={({ field }) => (
              <InputWithExample
                {...field}
                placeholder="~/.rua/workspaces/my-agent"
                readOnly={!!editingAgent}
              />
            )}
          />
          {!editingAgent && (
            <div style={{ fontSize: 12, color: "var(--text-3)", marginTop: 2 }}>
              {t("agent.workspaceHelp")}
            </div>
          )}
        </FormField>
      </div>

      <div style={{ marginTop: 4 }}>
        <div
          style={{
            display: "flex",
            justifyContent: "space-between",
            alignItems: "center",
            marginBottom: 8,
          }}
        >
          <span style={{ color: "var(--text-2)", fontSize: 13 }}>
            {editingAgent
              ? t("agent.addSkillsToAgent")
              : t("agent.initialSkills")}
          </span>
          <div style={{ display: "flex", gap: 4 }}>
            <Btn size="sm" variant="outline" onClick={handleSelectAll}>
              {t("agent.selectAll")}
            </Btn>
            <Btn size="sm" variant="outline" onClick={handleSelectBuiltin}>
              {t("agent.selectBuiltin")}
            </Btn>
            <Btn size="sm" variant="outline" onClick={handleSelectNone}>
              {t("agent.selectNone")}
            </Btn>
          </div>
        </div>

        {loadingSkills ? (
          <div style={{ textAlign: "center", padding: "16px 0" }}>
            <Loading size="sm" />
          </div>
        ) : poolSkills.length === 0 ? (
          <EmptyState description={t("agent.noPoolSkills")} />
        ) : (
          <div className={styles.pickerGrid}>
            {poolSkills.map((skill) => {
              const selected = selectedSkills.includes(skill.name);
              return (
                <div
                  key={skill.name}
                  className={`${styles.pickerCard} ${
                    selected ? styles.pickerCardSelected : ""
                  }`}
                  onClick={() => toggleSkill(skill.name)}
                >
                  {selected && (
                    <span className={styles.pickerCheck}>
                      <Check />
                    </span>
                  )}
                  <div className={styles.pickerCardTitle}>{skill.name}</div>
                </div>
              );
            })}
          </div>
        )}
      </div>
    </Modal>
  );
}
