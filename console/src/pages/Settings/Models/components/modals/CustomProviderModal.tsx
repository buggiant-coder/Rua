import { useState, useEffect } from "react";
import { useForm, Controller } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import api from "../../../../../api";
import type { ProviderInfo } from "../../../../../api/types";
import type { CreateCustomProviderRequest } from "../../../../../api/types";
import { useTranslation } from "react-i18next";
import { InputWithExample } from "../../../../../components/InputWithExample";
import { FormField, Modal, Select, toast } from "@/ui";
import { providerProtocolOptions } from "./providerProtocols";

interface CustomProviderModalProps {
  open: boolean;
  initialTemplateId?: string;
  onClose: () => void;
  onSaved: () => void;
}

interface FormValues {
  template: string;
  id: string;
  name: string;
  default_base_url: string;
  chat_model: string;
}

export function CustomProviderModal({
  open,
  initialTemplateId,
  onClose,
  onSaved,
}: CustomProviderModalProps) {
  const { t } = useTranslation();
  const [saving, setSaving] = useState(false);
  const [templates, setTemplates] = useState<ProviderInfo[]>([]);
  const [selectedTemplate, setSelectedTemplate] = useState<ProviderInfo | null>(
    null,
  );

  const schema = z.object({
    template: z.string(),
    id: z
      .string()
      .min(1, t("models.providerIdLabel"))
      .regex(/^[a-z][a-z0-9_-]{0,63}$/, t("models.providerIdHint")),
    name: z.string().min(1, t("models.providerNameLabel")),
    default_base_url: z.string(),
    chat_model: z.string().min(1, t("models.selectProtocol")),
  });

  const {
    control,
    handleSubmit,
    reset,
    setValue,
    formState: { errors },
  } = useForm<FormValues>({
    resolver: zodResolver(schema),
    defaultValues: {
      template: "",
      id: "",
      name: "",
      default_base_url: "",
      chat_model: "OpenAIChatModel",
    },
  });

  useEffect(() => {
    if (open) {
      reset({
        template: initialTemplateId ?? "",
        id: "",
        name: "",
        default_base_url: "",
        chat_model: "OpenAIChatModel",
      });
      setSelectedTemplate(null);
      api
        .listProviderTemplates()
        .then((items) => {
          setTemplates(items);
          const tpl = initialTemplateId
            ? items.find((item) => item.id === initialTemplateId)
            : null;
          if (tpl) {
            setSelectedTemplate(tpl);
            setValue("id", tpl.id);
            setValue("name", tpl.name);
            setValue("default_base_url", tpl.base_url || "");
            setValue("chat_model", tpl.chat_model || "OpenAIChatModel");
          }
        })
        .catch(() => {});
    }
  }, [open, reset, setValue, initialTemplateId]);

  const handleTemplateChange = (templateId: string) => {
    if (!templateId) {
      setValue("id", "");
      setValue("name", "");
      setValue("default_base_url", "");
      setValue("chat_model", "OpenAIChatModel");
      setSelectedTemplate(null);
      return;
    }
    const tpl = templates.find((t) => t.id === templateId);
    if (tpl) {
      setSelectedTemplate(tpl);
      setValue("id", tpl.id);
      setValue("name", tpl.name);
      setValue("default_base_url", tpl.base_url || "");
      setValue("chat_model", tpl.chat_model || "OpenAIChatModel");
    }
  };

  const onValid = async (values: FormValues) => {
    try {
      setSaving(true);
      const tplModels = selectedTemplate
        ? [
            ...(selectedTemplate.models || []),
            ...(selectedTemplate.extra_models || []),
          ]
        : [];
      const payload: CreateCustomProviderRequest = {
        id: values.id.trim(),
        template_id: selectedTemplate?.id,
        name: values.name.trim(),
        base_url: values.default_base_url?.trim() || "",
        api_key_prefix: selectedTemplate?.api_key_prefix || "",
        chat_model: values.chat_model || "OpenAIChatModel",
        models: tplModels,
      };
      if (selectedTemplate?.api_key_prefixes?.length) {
        payload.api_key_prefixes = selectedTemplate.api_key_prefixes;
      }
      if (selectedTemplate?.custom_headers) {
        payload.custom_headers = selectedTemplate.custom_headers;
      }
      if (selectedTemplate?.provider_group) {
        payload.provider_group = selectedTemplate.provider_group;
      }
      if (selectedTemplate?.provider_group_name) {
        payload.provider_group_name = selectedTemplate.provider_group_name;
      }
      if (selectedTemplate?.provider_variant) {
        payload.provider_variant = selectedTemplate.provider_variant;
      }
      if (selectedTemplate?.provider_adapter) {
        payload.provider_adapter = selectedTemplate.provider_adapter;
      }
      if (selectedTemplate?.base_url_options?.length) {
        payload.base_url_options = selectedTemplate.base_url_options;
      }
      if (selectedTemplate?.thinking_param_style !== undefined) {
        payload.thinking_param_style = selectedTemplate.thinking_param_style;
      }
      if (selectedTemplate?.reasoning_effort_options?.length) {
        payload.reasoning_effort_options =
          selectedTemplate.reasoning_effort_options;
      }
      if (selectedTemplate?.thinking_budget_range) {
        payload.thinking_budget_range = selectedTemplate.thinking_budget_range;
      }
      await api.createCustomProvider(payload);
      toast.success(t("models.providerCreated", { name: values.name.trim() }));
      onSaved();
      onClose();
    } catch (error) {
      const errMsg =
        error instanceof Error
          ? error.message
          : t("models.providerCreateFailed");
      toast.error(errMsg);
    } finally {
      setSaving(false);
    }
  };

  return (
    <Modal
      title={t("models.addProviderTitle")}
      open={open}
      onClose={onClose}
      onOk={handleSubmit(onValid)}
      confirmLoading={saving}
      okText={t("common.create")}
      cancelText={t("models.cancel")}
      destroyOnClose
    >
      <div
        style={{
          display: "flex",
          flexDirection: "column",
          gap: 16,
          marginTop: 16,
        }}
      >
        <FormField label={t("models.providerTemplate")}>
          <Controller
            name="template"
            control={control}
            render={({ field }) => (
              <Select
                allowClear
                showSearch
                placeholder={t("models.providerTemplatePlaceholder")}
                value={field.value || undefined}
                onChange={(val) => {
                  field.onChange(val ?? "");
                  handleTemplateChange(val ?? "");
                }}
                filterOption={(input, option) =>
                  (option?.label?.toString() || "")
                    .toLowerCase()
                    .includes(input.toLowerCase())
                }
                options={templates
                  .map((tpl) => ({
                    value: tpl.id,
                    label: t(`models.providerTemplateNames.${tpl.id}`, {
                      defaultValue: tpl.name,
                    }),
                  }))
                  .sort((a, b) => a.label.localeCompare(b.label))}
              />
            )}
          />
        </FormField>

        <FormField
          label={t("models.providerIdLabel")}
          error={errors.id?.message}
          required
        >
          <Controller
            name="id"
            control={control}
            render={({ field }) => (
              <InputWithExample
                placeholder={t("models.providerIdPlaceholder")}
                example="openai"
                value={field.value}
                onChange={field.onChange}
              />
            )}
          />
          <span
            style={{
              fontSize: 12,
              color: "var(--text-3)",
              marginTop: 4,
              display: "block",
            }}
          >
            {t("models.providerIdHint")}
          </span>
        </FormField>

        <FormField
          label={t("models.providerNameLabel")}
          error={errors.name?.message}
          required
        >
          <Controller
            name="name"
            control={control}
            render={({ field }) => (
              <InputWithExample
                placeholder={t("models.providerNamePlaceholder")}
                example="OpenAI"
                value={field.value}
                onChange={field.onChange}
              />
            )}
          />
        </FormField>

        {selectedTemplate?.base_url_options?.length ? (
          <FormField label={t("models.providerEndpoint")}>
            <Controller
              name="default_base_url"
              control={control}
              render={({ field }) => (
                <Select
                  value={field.value || undefined}
                  onChange={(value) => field.onChange(value ?? "")}
                  options={selectedTemplate.base_url_options?.map((option) => ({
                    value: option.value,
                    label: option.label,
                  }))}
                />
              )}
            />
          </FormField>
        ) : null}

        <FormField label={t("models.defaultBaseUrlLabel")}>
          <Controller
            name="default_base_url"
            control={control}
            render={({ field }) => (
              <InputWithExample
                placeholder={t("models.defaultBaseUrlPlaceholder")}
                example="https://api.example.com"
                value={field.value}
                onChange={field.onChange}
              />
            )}
          />
        </FormField>

        <FormField
          label={t("models.protocol")}
          error={errors.chat_model?.message}
          required
        >
          <Controller
            name="chat_model"
            control={control}
            render={({ field }) => (
              <Select
                value={field.value}
                onChange={field.onChange}
                options={providerProtocolOptions(t)}
              />
            )}
          />
          <span
            style={{
              fontSize: 12,
              color: "var(--text-3)",
              marginTop: 4,
              display: "block",
            }}
          >
            {t("models.protocolHint")}
          </span>
        </FormField>
      </div>
    </Modal>
  );
}
