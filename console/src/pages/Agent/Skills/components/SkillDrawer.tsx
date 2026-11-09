import { useState, useEffect, useCallback, useRef } from "react";
import { useForm, Controller } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { useTranslation } from "react-i18next";
import type { SkillSpec, InstallMeta } from "../../../../api/types";
import { MarkdownCopy } from "../../../../components/MarkdownCopy/MarkdownCopy";
import { InputWithExample } from "../../../../components/InputWithExample";
import { api } from "../../../../api";
import { SkillUpdateButton } from "./SkillUpdateButton";
import { SkillInstallInfo } from "./SkillInstallInfo";
import { Ban, Zap } from "lucide-react";
import { Btn, Drawer, FormField, Loading, TextInput, toast } from "@/ui";

export function parseFrontmatter(
  content: string,
): Record<string, string> | null {
  const trimmed = content.trim();
  if (!trimmed.startsWith("---")) return null;

  const endIndex = trimmed.indexOf("---", 3);
  if (endIndex === -1) return null;

  const frontmatterBlock = trimmed.slice(3, endIndex).trim();
  if (!frontmatterBlock) return null;

  const result: Record<string, string> = {};
  for (const line of frontmatterBlock.split("\n")) {
    const colonIndex = line.indexOf(":");
    if (colonIndex > 0) {
      const key = line.slice(0, colonIndex).trim();
      const value = line.slice(colonIndex + 1).trim();
      result[key] = value;
    }
  }
  return result;
}

export interface SkillDrawerFormValues {
  name: string;
  description?: string;
  content: string;
  enabled?: boolean;
  channels?: string[];
  source?: string;
  config?: Record<string, unknown>;
}

interface SkillDrawerProps {
  open: boolean;
  editingSkill: SkillSpec | null;
  agentId: string;
  onClose: () => void;
  onSubmit: (values: SkillSpec) => void;
  onContentChange?: (content: string) => void;
}

const schema = z.object({
  name: z.string().min(1),
  content: z.string().min(1),
});

type FormValues = z.infer<typeof schema>;

export function SkillDrawer({
  open,
  editingSkill,
  agentId,
  onClose,
  onSubmit,
  onContentChange,
}: SkillDrawerProps) {
  const { t, i18n } = useTranslation();
  const [showMarkdown, setShowMarkdown] = useState(true);
  const [contentValue, setContentValue] = useState("");
  const [contentLoading, setContentLoading] = useState(false);
  const [optimizing, setOptimizing] = useState(false);
  const abortControllerRef = useRef<AbortController | null>(null);
  const [detailMeta, setDetailMeta] = useState<InstallMeta | null>(null);
  const {
    control,
    handleSubmit,
    reset,
    setValue,
    formState: { errors },
  } = useForm<FormValues>({
    resolver: zodResolver(schema),
    defaultValues: { name: "", content: "" },
  });

  const validateFrontmatter = useCallback(
    (value: string) => {
      const content = contentValue || value;
      if (!content || !content.trim()) {
        return t("skills.pleaseInputContent");
      }
      const fm = parseFrontmatter(content);
      if (!fm) {
        return t("skills.frontmatterRequired");
      }
      if (!fm.name) {
        return t("skills.frontmatterNameRequired");
      }
      if (!fm.description) {
        return t("skills.frontmatterDescriptionRequired");
      }
      return true;
    },
    [contentValue, t],
  );

  useEffect(() => {
    if (editingSkill) {
      setContentValue(editingSkill.content);
      reset({ name: editingSkill.name, content: editingSkill.content });
      let active = true;
      setContentLoading(true);
      setDetailMeta(editingSkill.install_meta ?? null);
      api
        .getSkill(editingSkill.name, agentId)
        .then((detail) => {
          if (!active) return;
          setContentValue(detail.content);
          setValue("content", detail.content);
          if (detail.install_meta) setDetailMeta(detail.install_meta);
        })
        .catch(() => {})
        .finally(() => {
          if (active) setContentLoading(false);
        });
      return () => {
        active = false;
      };
    } else {
      setContentValue("");
      setContentLoading(false);
      setDetailMeta(null);
      reset({ name: "", content: "" });
    }
  }, [agentId, editingSkill, reset, setValue]);

  const onValid = (values: FormValues) => {
    onSubmit({
      ...editingSkill,
      ...values,
      content: contentValue || values.content,
      source: editingSkill?.source || "",
    });
  };

  const handleContentChange = (content: string) => {
    setContentValue(content);
    setValue("content", content);
    if (onContentChange) {
      onContentChange(content);
    }
  };

  const handleOptimize = async () => {
    if (!contentValue.trim()) {
      toast.warning(t("skills.noContentToOptimize"));
      return;
    }

    setOptimizing(true);
    abortControllerRef.current = new AbortController();
    const originalContent = contentValue;
    setContentValue("");

    try {
      await api.streamOptimizeSkill(
        originalContent,
        (textChunk) => {
          setContentValue((prev) => {
            const newContent = prev + textChunk;
            setValue("content", newContent);
            return newContent;
          });
        },
        abortControllerRef.current.signal,
        i18n.language,
      );
      toast.success(t("skills.optimizeSuccess"));
    } catch (error: unknown) {
      const aborted =
        error instanceof DOMException && error.name === "AbortError";
      if (!aborted) {
        toast.error(
          error instanceof Error ? error.message : t("skills.optimizeFailed"),
        );
      }
    } finally {
      setOptimizing(false);
      abortControllerRef.current = null;
    }
  };

  const handleStopOptimize = () => {
    if (abortControllerRef.current) {
      abortControllerRef.current.abort();
      setOptimizing(false);
      abortControllerRef.current = null;
    }
  };

  const drawerFooter = !editingSkill ? (
    <div
      style={{
        display: "flex",
        justifyContent: "space-between",
        width: "100%",
      }}
    >
      <div>
        {!optimizing ? (
          <Btn
            variant="outline"
            icon={<Zap />}
            onClick={handleOptimize}
            disabled={!contentValue.trim()}
          >
            {t("skills.optimizeWithAI")}
          </Btn>
        ) : (
          <Btn variant="danger" icon={<Ban />} onClick={handleStopOptimize}>
            {t("skills.stopOptimize")}
          </Btn>
        )}
      </div>
      <div style={{ display: "flex", gap: 8 }}>
        <Btn variant="outline" onClick={onClose}>
          {t("common.cancel")}
        </Btn>
        <Btn variant="filled" onClick={handleSubmit(onValid)}>
          {t("skills.create")}
        </Btn>
      </div>
    </div>
  ) : (
    <div style={{ display: "flex", justifyContent: "flex-end", gap: 8 }}>
      <Btn variant="outline" onClick={onClose}>
        {t("common.cancel")}
      </Btn>
      <Btn variant="filled" onClick={handleSubmit(onValid)}>
        {t("common.save")}
      </Btn>
    </div>
  );

  return (
    <Drawer
      placement="right"
      title={editingSkill ? t("skills.viewSkill") : t("skills.createSkill")}
      open={open}
      onClose={onClose}
      destroyOnClose
      footer={drawerFooter}
      styles={{ body: { overflow: "auto" } }}
    >
      <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
        {!editingSkill ? (
          <FormField
            label={t("common.name")}
            error={errors.name?.message}
            required
          >
            <Controller
              name="name"
              control={control}
              render={({ field }) => (
                <InputWithExample
                  {...field}
                  placeholder={t("skills.skillNamePlaceholder")}
                />
              )}
            />
          </FormField>
        ) : (
          <FormField label={t("common.name")}>
            <Controller
              name="name"
              control={control}
              render={({ field }) => <TextInput {...field} />}
            />
          </FormField>
        )}

        <FormField error={errors.content?.message}>
          <Controller
            name="content"
            control={control}
            rules={{ validate: validateFrontmatter }}
            render={() => (
              <Loading spinning={contentLoading}>
                <MarkdownCopy
                  content={contentValue}
                  showMarkdown={showMarkdown}
                  onShowMarkdownChange={setShowMarkdown}
                  showCopyButton={false}
                  editable={true}
                  onContentChange={handleContentChange}
                  markdownViewerProps={{
                    style: {
                      height: detailMeta
                        ? "calc(100vh - 410px)"
                        : "calc(100vh - 260px)",
                    },
                  }}
                  textareaProps={{
                    ...(!editingSkill && {
                      placeholder: t("skills.contentPlaceholder"),
                    }),
                    style: {
                      height: detailMeta
                        ? "calc(100vh - 410px)"
                        : "calc(100vh - 260px)",
                    },
                  }}
                />
              </Loading>
            )}
          />
        </FormField>

        {editingSkill && detailMeta && (
          <div>
            <SkillInstallInfo meta={detailMeta} />
            {detailMeta.source_url && (
              <div style={{ marginTop: 8 }}>
                <SkillUpdateButton
                  agentId={agentId}
                  skillName={editingSkill.name}
                  onUpdated={() => {
                    api
                      .getSkill(editingSkill.name, agentId)
                      .then((detail) => {
                        setContentValue(detail.content);
                        setValue("content", detail.content);
                        if (detail.install_meta)
                          setDetailMeta(detail.install_meta);
                      })
                      .catch(() => {});
                  }}
                />
              </div>
            )}
          </div>
        )}
      </div>
    </Drawer>
  );
}
