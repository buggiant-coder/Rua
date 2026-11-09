import { useState, useEffect, useMemo, useRef } from "react";
import type { KeyboardEvent, ReactNode, UIEvent } from "react";
import { useForm, Controller } from "react-hook-form";
import { InputWithExample } from "../../../../../components/InputWithExample";
import type {
  ActiveModelsInfo,
  ProviderConfigRequest,
  ProviderInfo,
} from "../../../../../api/types";
import api from "../../../../../api";
import { useTranslation } from "react-i18next";
import styles from "../../index.module.less";
import { ChevronDown, ChevronRight, Plug } from "lucide-react";
import { Btn, FormField, InfoBox, Modal, Select, TextInput, toast } from "@/ui";
import { providerProtocolOptions } from "./providerProtocols";
import { getLocalizedTestConnectionMessage } from "./testConnectionMessage";
import { openOAuthAuthorization } from "@/desktop/oauth";

function isRedactedApiKey(value?: string): boolean {
  if (!value) return false;
  return (
    value.includes("...") ||
    value.includes("******") ||
    value.split("").every((char) => char === "*")
  );
}

interface ProviderConfigFormValues
  extends Omit<ProviderConfigRequest, "generate_kwargs"> {
  generate_kwargs_text?: string;
  custom_headers_text?: string;
}

interface JsonCodeEditorProps {
  value?: string;
  onChange?: (value: string) => void;
  placeholder?: string;
  rows?: number;
}

function highlightJson(text: string): ReactNode[] {
  const tokens: ReactNode[] = [];
  const pattern =
    /("(?:\\.|[^"\\])*")(\s*:)?|\btrue\b|\bfalse\b|\bnull\b|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?|[{}[\],:]/g;

  let lastIndex = 0;
  let match: RegExpExecArray | null;

  while ((match = pattern.exec(text)) !== null) {
    const [token, stringToken, keySuffix] = match;

    if (match.index > lastIndex) {
      tokens.push(text.slice(lastIndex, match.index));
    }

    if (stringToken) {
      tokens.push(
        <span
          key={`${match.index}-${token}`}
          className={
            keySuffix ? styles.jsonEditorTokenKey : styles.jsonEditorTokenString
          }
        >
          {token}
        </span>,
      );
    } else if (token === "true" || token === "false") {
      tokens.push(
        <span
          key={`${match.index}-${token}`}
          className={styles.jsonEditorTokenBoolean}
        >
          {token}
        </span>,
      );
    } else if (token === "null") {
      tokens.push(
        <span
          key={`${match.index}-${token}`}
          className={styles.jsonEditorTokenNull}
        >
          {token}
        </span>,
      );
    } else if (/^-?\d/.test(token)) {
      tokens.push(
        <span
          key={`${match.index}-${token}`}
          className={styles.jsonEditorTokenNumber}
        >
          {token}
        </span>,
      );
    } else {
      tokens.push(
        <span
          key={`${match.index}-${token}`}
          className={styles.jsonEditorTokenPunctuation}
        >
          {token}
        </span>,
      );
    }

    lastIndex = match.index + token.length;
  }

  if (lastIndex < text.length) {
    tokens.push(text.slice(lastIndex));
  }

  return tokens;
}

function JsonCodeEditor({
  value = "",
  onChange,
  placeholder,
  rows = 8,
}: JsonCodeEditorProps) {
  const indentUnit = "  ";
  const highlightRef = useRef<HTMLDivElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  const handleScroll = (event: UIEvent<HTMLTextAreaElement>) => {
    if (!highlightRef.current) {
      return;
    }

    highlightRef.current.scrollTop = event.currentTarget.scrollTop;
    highlightRef.current.scrollLeft = event.currentTarget.scrollLeft;
  };

  const handleKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key !== "Tab") {
      return;
    }

    event.preventDefault();

    const textarea = event.currentTarget;
    const selectionStart = textarea.selectionStart;
    const selectionEnd = textarea.selectionEnd;
    const hasSelection = selectionStart !== selectionEnd;
    const selectedText = value.slice(selectionStart, selectionEnd);

    if (!hasSelection || !selectedText.includes("\n")) {
      if (event.shiftKey) {
        const lineStart = value.lastIndexOf("\n", selectionStart - 1) + 1;
        const linePrefix = value.slice(lineStart, selectionStart);

        if (!linePrefix.endsWith(indentUnit)) {
          return;
        }

        const nextValue =
          value.slice(0, selectionStart - indentUnit.length) +
          value.slice(selectionStart);

        onChange?.(nextValue);

        requestAnimationFrame(() => {
          textareaRef.current?.setSelectionRange(
            selectionStart - indentUnit.length,
            selectionStart - indentUnit.length,
          );
        });
        return;
      }

      const nextValue =
        value.slice(0, selectionStart) + indentUnit + value.slice(selectionEnd);

      onChange?.(nextValue);

      requestAnimationFrame(() => {
        const nextCursor = selectionStart + indentUnit.length;
        textareaRef.current?.setSelectionRange(nextCursor, nextCursor);
      });
      return;
    }

    const lineStart = value.lastIndexOf("\n", selectionStart - 1) + 1;
    const block = value.slice(lineStart, selectionEnd);
    const lines = block.split("\n");

    if (event.shiftKey) {
      const updatedLines = lines.map((line) =>
        line.startsWith(indentUnit) ? line.slice(indentUnit.length) : line,
      );
      const removedFromFirstLine = lines[0].startsWith(indentUnit)
        ? indentUnit.length
        : 0;
      const removedTotal = lines.reduce(
        (total, line) =>
          total + (line.startsWith(indentUnit) ? indentUnit.length : 0),
        0,
      );
      const nextValue =
        value.slice(0, lineStart) +
        updatedLines.join("\n") +
        value.slice(selectionEnd);

      onChange?.(nextValue);

      requestAnimationFrame(() => {
        textareaRef.current?.setSelectionRange(
          selectionStart - removedFromFirstLine,
          selectionEnd - removedTotal,
        );
      });
      return;
    }

    const updatedLines = lines.map((line) => `${indentUnit}${line}`);
    const nextValue =
      value.slice(0, lineStart) +
      updatedLines.join("\n") +
      value.slice(selectionEnd);

    onChange?.(nextValue);

    requestAnimationFrame(() => {
      textareaRef.current?.setSelectionRange(
        selectionStart + indentUnit.length,
        selectionEnd + indentUnit.length * lines.length,
      );
    });
  };

  return (
    <div className={styles.jsonEditorContainer}>
      <div
        ref={highlightRef}
        aria-hidden="true"
        className={styles.jsonEditorHighlight}
      >
        {value ? highlightJson(value) : placeholder}
        {!value && <span>{"\n"}</span>}
      </div>
      <textarea
        ref={textareaRef}
        rows={rows}
        value={value}
        onChange={(event) => onChange?.(event.target.value)}
        onKeyDown={handleKeyDown}
        onScroll={handleScroll}
        placeholder={placeholder}
        autoComplete="off"
        autoCapitalize="off"
        autoCorrect="off"
        spellCheck={false}
        className={styles.jsonEditorTextarea}
      />
    </div>
  );
}

interface ProviderConfigModalProps {
  provider: ProviderInfo;
  activeModels: ActiveModelsInfo | null;
  open: boolean;
  onClose: () => void;
  onSaved: () => void;
}

export function ProviderConfigModal({
  provider,
  activeModels,
  open,
  onClose,
  onSaved,
}: ProviderConfigModalProps) {
  const { t } = useTranslation();
  const [saving, setSaving] = useState(false);
  const [testing, setTesting] = useState(false);
  const [oauthBusy, setOauthBusy] = useState(false);
  const [formDirty, setFormDirty] = useState(false);
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [connectionFeedback, setConnectionFeedback] = useState<{
    success: boolean;
    message: string;
  } | null>(null);
  const oauthPollRef = useRef<number | null>(null);
  const canEditBaseUrl = !provider.freeze_url || provider.id === "dashscope";

  const {
    register,
    handleSubmit,
    reset,
    watch,
    control,
    getValues,
    formState: { errors },
  } = useForm<ProviderConfigFormValues>({
    defaultValues: {
      api_key: provider.api_key || undefined,
      base_url: provider.base_url || undefined,
      chat_model: provider.chat_model || "OpenAIChatModel",
      generate_kwargs_text:
        provider.generate_kwargs &&
        Object.keys(provider.generate_kwargs).length > 0
          ? JSON.stringify(provider.generate_kwargs, null, 2)
          : undefined,
      custom_headers_text:
        provider.custom_headers &&
        Object.keys(provider.custom_headers).length > 0
          ? JSON.stringify(provider.custom_headers, null, 2)
          : undefined,
    },
  });

  const selectedChatModel = watch("chat_model");
  const apiKeyValue = watch("api_key");

  useEffect(() => {
    return () => {
      if (oauthPollRef.current !== null) {
        window.clearInterval(oauthPollRef.current);
      }
    };
  }, []);

  const parseObjectJson = (
    value: string | undefined,
    invalidJsonKey: string,
    mustBeObjectKey: string,
  ) => {
    const trimmed = value?.trim();
    if (!trimmed) {
      return undefined;
    }

    let parsed: unknown;
    try {
      parsed = JSON.parse(trimmed);
    } catch {
      throw new Error(t(invalidJsonKey));
    }

    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      throw new Error(t(mustBeObjectKey));
    }

    return parsed as Record<string, unknown>;
  };

  const parseGenerateConfig = (value?: string) =>
    parseObjectJson(
      value,
      "models.generateConfigInvalidJson",
      "models.generateConfigMustBeObject",
    );

  const parseCustomHeaders = (value?: string) =>
    parseObjectJson(
      value,
      "models.customHeadersInvalidJson",
      "models.customHeadersMustBeObject",
    );

  const effectiveChatModel = useMemo(() => {
    if (!provider.is_custom && provider.id !== "dashscope") {
      return provider.chat_model;
    }
    return selectedChatModel || provider.chat_model || "OpenAIChatModel";
  }, [provider.chat_model, provider.is_custom, selectedChatModel]);

  const apiKeyPlaceholder = useMemo(() => {
    const prefixes = provider.api_key_prefixes?.length
      ? provider.api_key_prefixes
      : provider.api_key_prefix
      ? [provider.api_key_prefix]
      : [];
    if (provider.api_key) {
      return t("models.leaveBlankKeep");
    }
    if (prefixes.length > 0) {
      return t("models.enterApiKey", { prefix: prefixes.join(" / ") });
    }
    return t("models.enterApiKeyOptional");
  }, [provider.api_key, provider.api_key_prefix, provider.api_key_prefixes, t]);

  const baseUrlExtra = useMemo(() => {
    if (!canEditBaseUrl) {
      return undefined;
    }
    if (provider.id === "azure-openai") {
      return t("models.azureEndpointHint");
    }
    if (provider.id === "anthropic") {
      return t("models.anthropicEndpointHint");
    }
    if (provider.id === "openai") {
      return t("models.openAIEndpoint");
    }
    if (provider.id === "opencode") {
      return t("models.openAICompatibleEndpoint");
    }
    if (provider.id === "ollama") {
      return t("models.ollamaEndpointHint");
    }
    if (provider.id === "lmstudio") {
      return t("models.lmstudioEndpointHint");
    }
    if (provider.is_custom) {
      return effectiveChatModel === "AnthropicChatModel"
        ? t("models.anthropicEndpointHint")
        : t("models.openAICompatibleEndpoint");
    }
    return t("models.apiEndpointHint");
  }, [canEditBaseUrl, provider.id, provider.is_custom, effectiveChatModel, t]);

  const baseUrlPlaceholder = useMemo(() => {
    if (!canEditBaseUrl) {
      return "";
    }
    if (provider.id === "azure-openai") {
      return "https://<resource>.openai.azure.com/openai/v1";
    }
    if (provider.id === "anthropic") {
      return "https://api.anthropic.com";
    }
    if (provider.id === "openai") {
      return "https://api.openai.com/v1";
    }
    if (provider.id === "dashscope") {
      return "https://<WorkspaceId>.cn-beijing.maas.aliyuncs.com/compatible-mode/v1";
    }
    if (provider.id === "opencode") {
      return "https://opencode.ai/zen/v1";
    }
    if (provider.id === "ollama") {
      return "http://localhost:11434";
    }
    if (provider.id === "lmstudio") {
      return "http://localhost:1234/v1";
    }
    if (provider.is_custom && effectiveChatModel === "AnthropicChatModel") {
      return "https://api.anthropic.com";
    }
    return "https://api.example.com";
  }, [canEditBaseUrl, provider.id, provider.is_custom, effectiveChatModel]);

  useEffect(() => {
    if (open) {
      reset({
        api_key: provider.api_key || undefined,
        base_url: provider.base_url || undefined,
        chat_model: provider.chat_model || "OpenAIChatModel",
        generate_kwargs_text:
          provider.generate_kwargs &&
          Object.keys(provider.generate_kwargs).length > 0
            ? JSON.stringify(provider.generate_kwargs, null, 2)
            : undefined,
        custom_headers_text:
          provider.custom_headers &&
          Object.keys(provider.custom_headers).length > 0
            ? JSON.stringify(provider.custom_headers, null, 2)
            : undefined,
      });
      setAdvancedOpen(false);
      setFormDirty(false);
      setConnectionFeedback(null);
    }
  }, [provider, reset, open]);

  const onSubmit = async (values: ProviderConfigFormValues) => {
    try {
      setSaving(true);
      const generateConfig = parseGenerateConfig(values.generate_kwargs_text);
      const hasGenerateConfigInput = Boolean(
        values.generate_kwargs_text?.trim(),
      );
      const customHeaders = parseCustomHeaders(values.custom_headers_text);
      const hasCustomHeadersInput = Boolean(values.custom_headers_text?.trim());
      const apiKeyOverride = isRedactedApiKey(values.api_key)
        ? undefined
        : values.api_key;

      if (provider.support_connection_check) {
        let connectionFailure: string | null = null;
        try {
          const result = await api.testProviderConnection(provider.id, {
            ...(apiKeyOverride !== undefined
              ? { api_key: apiKeyOverride }
              : {}),
            base_url: values.base_url,
            chat_model: values.chat_model,
            ...(hasCustomHeadersInput ? { custom_headers: customHeaders } : {}),
          });
          if (!result.success) {
            connectionFailure =
              result.message || t("models.testConnectionFailed");
          }
        } catch (error) {
          connectionFailure =
            error instanceof Error
              ? error.message
              : t("models.testConnectionError");
        }

        if (connectionFailure) {
          setSaving(false);
          const confirmed = await Modal.confirm({
            title: t("models.testConnectionFailed"),
            content: t("models.providerTestFailedConfirm", {
              message: connectionFailure,
            }),
            okText: t("models.saveAnyway"),
            cancelText: t("models.returnToEdit"),
          });
          if (!confirmed) {
            return;
          }
          setSaving(true);
        }
      }

      await api.configureProvider(provider.id, {
        ...(apiKeyOverride !== undefined ? { api_key: apiKeyOverride } : {}),
        base_url: values.base_url,
        chat_model: values.chat_model,
        generate_kwargs: hasGenerateConfigInput ? generateConfig : {},
        ...(hasCustomHeadersInput ? { custom_headers: customHeaders } : {}),
      });

      await onSaved();
      setFormDirty(false);
      onClose();
      toast.success(t("models.configurationSaved", { name: provider.name }));
    } catch (error) {
      const errMsg =
        error instanceof Error ? error.message : t("models.failedToSaveConfig");
      toast.error(errMsg);
    } finally {
      setSaving(false);
    }
  };

  const handleTest = async () => {
    setTesting(true);
    try {
      const values = getValues();
      const customHeaders = parseCustomHeaders(values.custom_headers_text);
      const hasCustomHeadersInput = Boolean(values.custom_headers_text?.trim());
      const apiKeyOverride = isRedactedApiKey(values.api_key)
        ? undefined
        : values.api_key;
      const result = await api.testProviderConnection(provider.id, {
        ...(apiKeyOverride !== undefined ? { api_key: apiKeyOverride } : {}),
        base_url: values.base_url,
        chat_model: values.chat_model,
        ...(hasCustomHeadersInput ? { custom_headers: customHeaders } : {}),
      });
      const message = getLocalizedTestConnectionMessage(result, t);
      setConnectionFeedback({ success: result.success, message });
      if (result.success) {
        toast.success(message);
      } else {
        toast.warning(message);
      }
    } catch (error) {
      const errMsg =
        error instanceof Error
          ? error.message
          : t("models.testConnectionError");
      setConnectionFeedback({ success: false, message: errMsg });
      toast.error(errMsg);
    } finally {
      setTesting(false);
    }
  };

  const isActiveLlmProvider =
    activeModels?.default_llm?.provider_id === provider.id;

  const needsOAuth =
    !!provider.supports_oauth && !provider.oauth_connected && !provider.api_key;

  const handleAuthorizeOAuth = async () => {
    if (oauthBusy) return;
    setOauthBusy(true);
    try {
      const response = await api.startProviderOAuth(provider.id);
      const launch = await openOAuthAuthorization(
        response.authorize_url,
        "provider-oauth",
        "width=720,height=820",
      );
      const popup = launch.popup;
      const finish = async (success: boolean, message?: string) => {
        if (oauthPollRef.current !== null) {
          window.clearInterval(oauthPollRef.current);
          oauthPollRef.current = null;
        }
        window.removeEventListener("message", handleMessage);
        setOauthBusy(false);
        if (success) {
          toast.success(t("models.oauthConnected", { name: provider.name }));
          await onSaved();
        } else if (message) {
          toast.error(message);
        }
      };
      const handleMessage = (event: MessageEvent) => {
        const data = event.data;
        if (data && data.type !== "provider-oauth") return;
        void finish(data?.status !== "error");
      };
      window.addEventListener("message", handleMessage);
      if (launch.mode === "popup" && !popup) {
        await finish(false, t("models.oauthPopupBlocked"));
        return;
      }
      oauthPollRef.current = window.setInterval(async () => {
        if (popup?.closed) {
          await finish(false);
          await onSaved();
          return;
        }
        try {
          const status = await api.getProviderOAuthStatus(
            provider.id,
            response.state,
          );
          if (status.status === "completed") {
            await finish(true);
          } else if (status.status === "failed") {
            await finish(false, status.error || t("models.oauthFailed"));
          }
        } catch {}
      }, 1200);
    } catch (error) {
      setOauthBusy(false);
      toast.error(
        error instanceof Error ? error.message : t("models.oauthStartFailed"),
      );
    }
  };

  const handleRevoke = () => {
    const confirmContent = isActiveLlmProvider
      ? t("models.revokeConfirmContent", { name: provider.name })
      : t("models.revokeConfirmSimple", { name: provider.name });

    Modal.confirm({
      title: t("models.revokeAuthorization"),
      content: confirmContent,
      okText: t("models.revokeAuthorization"),
      danger: true,
      cancelText: t("models.cancel"),
      onOk: async () => {
        try {
          await api.configureProvider(provider.id, { api_key: "" });
          await onSaved();
          onClose();
          if (isActiveLlmProvider) {
            toast.success(
              t("models.authorizationRevoked", { name: provider.name }),
            );
          } else {
            toast.success(
              t("models.authorizationRevokedSimple", { name: provider.name }),
            );
          }
        } catch (error) {
          const errMsg =
            error instanceof Error ? error.message : t("models.failedToRevoke");
          toast.error(errMsg);
        }
      },
    });
  };

  return (
    <Modal
      className="modal-fullscreen-mobile"
      width={800}
      title={t("models.configureProvider", { name: provider.name })}
      open={open}
      onClose={onClose}
      footer={
        <div className={`${styles.modalFooter} ${styles.providerConfigFooter}`}>
          <div className={styles.modalFooterLeft}>
            {needsOAuth && (
              <Btn onClick={handleAuthorizeOAuth} loading={oauthBusy}>
                {t("models.oauthAuthorize")}
              </Btn>
            )}
            {provider.api_key && (
              <Btn variant="danger" onClick={handleRevoke}>
                {t("models.revokeAuthorization")}
              </Btn>
            )}
            {provider.support_connection_check && (
              <Btn icon={<Plug />} onClick={handleTest} loading={testing}>
                {t("models.testConnection")}
              </Btn>
            )}
          </div>
          <div className={styles.modalFooterRight}>
            <Btn onClick={onClose}>{t("models.cancel")}</Btn>
            <Btn
              loading={saving}
              disabled={!formDirty}
              onClick={handleSubmit(onSubmit)}
            >
              {t("models.save")}
            </Btn>
          </div>
        </div>
      }
      destroyOnClose
    >
      <div
        style={{ display: "flex", flexDirection: "column", gap: 16 }}
        onChange={() => {
          setFormDirty(true);
          setConnectionFeedback(null);
        }}
      >
        {(provider.is_custom || provider.id === "dashscope") && (
          <FormField
            label={t("models.protocol")}
            error={errors.chat_model?.message}
            required
          >
            <Controller
              name="chat_model"
              control={control}
              rules={{ required: t("models.selectProtocol") }}
              render={({ field }) => (
                <Select
                  {...field}
                  disabled={provider.is_custom && provider.id !== "dashscope"}
                  onChange={(value) => {
                    field.onChange(value);
                    setFormDirty(true);
                    setConnectionFeedback(null);
                  }}
                  options={
                    provider.id === "dashscope"
                      ? [
                          {
                            value: "DashScopeResponseModel",
                            label: t("models.protocolOpenAIResponses"),
                          },
                          {
                            value: "DashScopeChatModel",
                            label: t("models.protocolOpenAI"),
                          },
                        ]
                      : providerProtocolOptions(t)
                  }
                />
              )}
            />
            <div style={{ fontSize: 12, color: "var(--text-3)", marginTop: 4 }}>
              {t("models.protocolHint")}
            </div>
          </FormField>
        )}

        {}
        <FormField
          label={t("models.baseURL")}
          error={errors.base_url?.message}
          required={canEditBaseUrl}
        >
          <Controller
            name="base_url"
            control={control}
            rules={
              canEditBaseUrl
                ? {
                    ...(canEditBaseUrl && {
                      required: t("models.pleaseEnterBaseURL"),
                    }),
                    validate: (value) => {
                      if (!value || !value.trim()) return true;
                      try {
                        const url = new URL(value.trim());
                        if (!["http:", "https:"].includes(url.protocol)) {
                          return t("models.pleaseEnterValidURL");
                        }
                        return true;
                      } catch {
                        return t("models.pleaseEnterValidURL");
                      }
                    },
                  }
                : {}
            }
            render={({ field }) => (
              <InputWithExample
                {...field}
                value={field.value ?? ""}
                placeholder={baseUrlPlaceholder}
                disabled={!canEditBaseUrl}
                onChange={(e) => {
                  field.onChange(e);
                  setFormDirty(true);
                }}
              />
            )}
          />
          {baseUrlExtra && (
            <div style={{ fontSize: 12, color: "var(--text-3)", marginTop: 4 }}>
              {baseUrlExtra}
            </div>
          )}
        </FormField>

        {}
        <FormField label={t("models.apiKey")} error={errors.api_key?.message}>
          <TextInput
            type={isRedactedApiKey(apiKeyValue) ? "text" : "password"}
            placeholder={apiKeyPlaceholder}
            {...register("api_key", {
              validate: (value) => {
                if (isRedactedApiKey(value)) {
                  return true;
                }
                if (
                  value &&
                  (provider.api_key_prefixes?.length || provider.api_key_prefix)
                ) {
                  const prefixes = provider.api_key_prefixes?.length
                    ? provider.api_key_prefixes
                    : [provider.api_key_prefix];
                  if (prefixes.some((prefix) => value.startsWith(prefix))) {
                    return true;
                  }
                  return t("models.apiKeyShouldStart", {
                    prefix: prefixes.join(" / "),
                  });
                }
                return true;
              },
              onChange: () => setFormDirty(true),
            })}
            onFocus={(event) => {
              if (isRedactedApiKey(event.currentTarget.value)) {
                event.currentTarget.select();
              }
            }}
          />
        </FormField>

        {connectionFeedback && (
          <InfoBox
            type={connectionFeedback.success ? "success" : "error"}
            title={t("models.connectionTestResult")}
          >
            {connectionFeedback.message}
          </InfoBox>
        )}

        <div className={styles.advancedConfigSection}>
          <Btn
            variant="outline"
            size="sm"
            className={styles.advancedConfigToggle}
            onClick={() => setAdvancedOpen((prev) => !prev)}
            icon={advancedOpen ? <ChevronDown /> : <ChevronRight />}
          >
            {t("models.advancedConfig")}
          </Btn>

          {advancedOpen && (
            <>
              <FormField
                label={t("models.generateConfig")}
                error={errors.generate_kwargs_text?.message}
              >
                <Controller
                  name="generate_kwargs_text"
                  control={control}
                  rules={{
                    validate: (value) => {
                      try {
                        parseGenerateConfig(value);
                        return true;
                      } catch (error) {
                        return error instanceof Error
                          ? error.message
                          : t("models.generateConfigInvalidJson");
                      }
                    },
                  }}
                  render={({ field }) => (
                    <JsonCodeEditor
                      rows={8}
                      placeholder={t("models.generateConfigPlaceholder")}
                      value={field.value ?? ""}
                      onChange={(v) => {
                        field.onChange(v);
                        setFormDirty(true);
                      }}
                    />
                  )}
                />
                <div
                  style={{ fontSize: 12, color: "var(--text-3)", marginTop: 4 }}
                >
                  {t("models.generateConfigHint")}
                </div>
              </FormField>
              <FormField
                label={t("models.customHeaders")}
                error={errors.custom_headers_text?.message}
              >
                <Controller
                  name="custom_headers_text"
                  control={control}
                  rules={{
                    validate: (value) => {
                      try {
                        parseCustomHeaders(value);
                        return true;
                      } catch (error) {
                        return error instanceof Error
                          ? error.message
                          : t("models.customHeadersInvalidJson");
                      }
                    },
                  }}
                  render={({ field }) => (
                    <JsonCodeEditor
                      rows={5}
                      placeholder={t("models.customHeadersPlaceholder")}
                      value={field.value ?? ""}
                      onChange={(v) => {
                        field.onChange(v);
                        setFormDirty(true);
                      }}
                    />
                  )}
                />
                <div
                  style={{ fontSize: 12, color: "var(--text-3)", marginTop: 4 }}
                >
                  {t("models.customHeadersHint")}
                </div>
              </FormField>
            </>
          )}
        </div>
      </div>
    </Modal>
  );
}
