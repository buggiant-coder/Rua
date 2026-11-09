import { useState, useEffect, useRef, useMemo, useCallback } from "react";
import type { KeyboardEvent, ReactNode, UIEvent } from "react";
import { useForm, Controller } from "react-hook-form";
import { InputWithExample } from "../../../../../components/InputWithExample";
import {
  AudioLines,
  FileUp,
  Film,
  ImageUp,
  ImagePlus,
  Type,
} from "lucide-react";
import type {
  ProviderInfo,
  SeriesResponse,
  ModelInfo,
} from "../../../../../api/types";
import api from "../../../../../api";
import { useTranslation } from "react-i18next";
import { useTheme } from "../../../../../contexts/ThemeContext";
import styles from "../../index.module.less";
import {
  ChevronDown,
  Eye,
  Filter,
  Plug,
  Plus,
  Settings,
  Trash2,
} from "lucide-react";
import {
  Btn,
  Check,
  FormField,
  InfoBox,
  Modal,
  NumberInput,
  Select,
  Tag,
  toast,
} from "@/ui";
import { getLocalizedTestConnectionMessage } from "./testConnectionMessage";
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

function MiniJsonEditor({
  value = "",
  onChange,
  placeholder,
}: {
  value?: string;
  onChange?: (value: string) => void;
  placeholder?: string;
}) {
  const indentUnit = "  ";
  const highlightRef = useRef<HTMLDivElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  const handleScroll = (event: UIEvent<HTMLTextAreaElement>) => {
    if (!highlightRef.current) return;
    highlightRef.current.scrollTop = event.currentTarget.scrollTop;
    highlightRef.current.scrollLeft = event.currentTarget.scrollLeft;
  };

  const handleKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key !== "Tab") return;
    event.preventDefault();

    const textarea = event.currentTarget;
    const selectionStart = textarea.selectionStart;
    const selectionEnd = textarea.selectionEnd;

    if (event.shiftKey) {
      const lineStart = value.lastIndexOf("\n", selectionStart - 1) + 1;
      const linePrefix = value.slice(lineStart, selectionStart);
      if (!linePrefix.endsWith(indentUnit)) return;
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
  };

  return (
    <div className={styles.jsonEditorContainer} style={{ marginTop: 8 }}>
      <div
        ref={highlightRef}
        aria-hidden="true"
        className={styles.jsonEditorHighlight}
        style={{ minHeight: 100 }}
      >
        {value ? highlightJson(value) : placeholder}
        {!value && <span>{"\n"}</span>}
      </div>
      <textarea
        ref={textareaRef}
        rows={5}
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
        style={{ minHeight: 100 }}
      />
    </div>
  );
}

function ModelConfigEditor({
  providerId,
  model,
  onSaved,
  onClose,
}: {
  providerId: string;
  model: ModelInfo;
  onSaved: () => void | Promise<void>;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const [saving, setSaving] = useState(false);

  const initialText = useMemo(
    () =>
      model.generate_kwargs && Object.keys(model.generate_kwargs).length > 0
        ? JSON.stringify(model.generate_kwargs, null, 2)
        : "",
    [model.generate_kwargs],
  );

  const [text, setText] = useState(initialText);
  const [ctxWindow, setCtxWindow] = useState<number | null>(
    model.context_window,
  );
  const [dirty, setDirty] = useState(false);

  useEffect(() => {
    setText(initialText);
    setCtxWindow(model.context_window);
    setDirty(false);
  }, [initialText, model.context_window]);

  const handleChange = useCallback((val: string) => {
    setText(val);
    setDirty(true);
  }, []);

  const handleCtxWindowChange = useCallback((val: number | null) => {
    setCtxWindow(val);
    setDirty(true);
  }, []);

  const handleSave = async () => {
    const trimmed = text.trim();
    let parsed: Record<string, unknown> = {};
    if (trimmed) {
      try {
        const obj = JSON.parse(trimmed);
        if (!obj || typeof obj !== "object" || Array.isArray(obj)) {
          toast.error(t("models.generateConfigMustBeObject"));
          return;
        }
        parsed = obj;
      } catch {
        toast.error(t("models.generateConfigInvalidJson"));
        return;
      }
    }

    setSaving(true);
    try {
      await api.configureModel(providerId, model.id, {
        generate_kwargs: parsed,
        context_window: ctxWindow,
      });
      toast.success(t("models.modelConfigSaved", { name: model.name }));
      setDirty(false);
      await onSaved();
      onClose();
    } catch (error) {
      const errMsg =
        error instanceof Error
          ? error.message
          : t("models.modelConfigSaveFailed");
      toast.error(errMsg);
    } finally {
      setSaving(false);
    }
  };

  return (
    <div style={{ padding: "8px 0 4px" }}>
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 8,
          marginBottom: 8,
        }}
      >
        <span
          style={{ fontSize: 12, color: "var(--text-3)", whiteSpace: "nowrap" }}
        >
          {t("models.modelIdLabel")}
        </span>
        <span
          style={{
            fontFamily: "var(--font-mono)",
            fontSize: 12,
            color: "var(--text)",
            display: "inline-flex",
            alignItems: "center",
            gap: 4,
            cursor: "pointer",
          }}
          onClick={() => navigator.clipboard.writeText(model.id)}
        >
          {model.id}
        </span>
        <Btn
          size="sm"
          loading={saving}
          disabled={!dirty}
          onClick={handleSave}
          style={{ marginLeft: "auto" }}
        >
          {t("models.save")}
        </Btn>
      </div>
      <div style={{ marginBottom: 8 }}>
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 8,
            marginBottom: 4,
          }}
        >
          <span
            style={{
              fontSize: 12,
              color: "var(--text-3)",
              whiteSpace: "nowrap",
            }}
          >
            {t("models.contextWindowLabel")}
          </span>
          {(
            [
              { label: "200K", value: 200000 },
              { label: "850K", value: 850000 },
              { label: "1M", value: 1000000 },
            ] as const
          ).map((preset) => (
            <Btn
              key={preset.label}
              size="sm"
              onClick={() => handleCtxWindowChange(preset.value)}
            >
              {preset.label}
            </Btn>
          ))}
        </div>
        <NumberInput
          value={ctxWindow}
          onChange={handleCtxWindowChange}
          min={1}
          style={{ width: "100%" }}
          placeholder={t("models.contextWindowPlaceholder")}
        />
      </div>
      <div
        style={{
          fontSize: 12,
          color: "var(--text-3)",
          marginBottom: 4,
        }}
      >
        {t("models.modelGenerateConfigHint")}
      </div>
      <MiniJsonEditor
        value={text}
        onChange={handleChange}
        placeholder={`Example:\n{\n  "extra_body": {\n    "enable_thinking": false\n  },\n  "max_tokens": 2048\n}`}
      />
    </div>
  );
}

interface AddModelFormValues {
  id: string;
  name?: string;
}

interface DiscoveredOpenRouterModel {
  id: string;
  name?: string;
  provider?: string;
  input_modalities?: string[];
  output_modalities?: string[];
  pricing?: {
    prompt?: string;
    completion?: string;
  };
}

interface RemoteModelManageModalProps {
  provider: ProviderInfo;
  open: boolean;
  onClose: () => void;
  onSaved: () => void | Promise<void>;
}

export function RemoteModelManageModal({
  provider,
  open,
  onClose,
  onSaved,
}: RemoteModelManageModalProps) {
  const { t } = useTranslation();
  const { isDark } = useTheme();
  const [adding, setAdding] = useState(false);
  const [saving, setSaving] = useState(false);
  const [testingModelId, setTestingModelId] = useState<string | null>(null);
  const [modelTestResults, setModelTestResults] = useState<
    Record<string, { success: boolean; message: string }>
  >({});
  const [probingModelId, setProbingModelId] = useState<string | null>(null);
  const [configOpenModelId, setConfigOpenModelId] = useState<string | null>(
    null,
  );
  const {
    register,
    handleSubmit,
    reset,
    control,
    formState: { errors },
  } = useForm<AddModelFormValues>({
    defaultValues: { id: "", name: "" },
  });

  const isOpenRouter = provider.id === "openrouter";
  const [showFilters, setShowFilters] = useState(false);
  const [availableSeries, setAvailableSeries] = useState<string[]>([]);
  const [discoveredModels, setDiscoveredModels] = useState<
    DiscoveredOpenRouterModel[]
  >([]);
  const [selectedSeries, setSelectedSeries] = useState<string[]>([]);
  const [selectedInputModality, setSelectedInputModality] = useState<
    string | null
  >(null);
  const [loadingFilters, setLoadingFilters] = useState(false);

  const [loadingDiscoveredModels, setLoadingDiscoveredModels] = useState(false);

  const doAddModel = async (id: string, name: string) => {
    await api.addModel(provider.id, { id, name });
    toast.success(t("models.modelAdded", { name }));
    reset();
    setAdding(false);
    onSaved();
  };

  const handleAddModel = async (values: AddModelFormValues) => {
    const id = values.id.trim();
    const name = values.name?.trim() || id;

    setSaving(true);
    try {
      const testResult = await api.testModelConnection(provider.id, {
        model_id: id,
      });

      if (!testResult.success) {
        setSaving(false);
        Modal.confirm({
          title: t("models.testConnectionFailed"),
          content: t("models.modelTestFailedConfirm", {
            message: testResult.message || t("models.modelTestFailed"),
          }),
          okText: t("models.addModel"),
          cancelText: t("models.cancel"),
          onOk: async () => {
            setSaving(true);
            try {
              await doAddModel(id, name);
            } catch (error) {
              const errMsg =
                error instanceof Error
                  ? error.message
                  : t("models.modelAddFailed");
              toast.error(errMsg);
            } finally {
              setSaving(false);
            }
          },
        });
        return;
      }

      await doAddModel(id, name);
    } catch (error) {
      const errMsg =
        error instanceof Error ? error.message : t("models.modelAddFailed");
      toast.error(errMsg);
    } finally {
      setSaving(false);
    }
  };

  const handleTestModel = async (modelId: string) => {
    setTestingModelId(modelId);
    try {
      const result = await api.testModelConnection(provider.id, {
        model_id: modelId,
      });
      const message = getLocalizedTestConnectionMessage(result, t);
      setModelTestResults((current) => ({
        ...current,
        [modelId]: { success: result.success, message },
      }));
      if (result.success) {
        toast.success(message);
        await onSaved();
      } else {
        toast.warning(message);
      }
    } catch (error) {
      const errMsg =
        error instanceof Error
          ? error.message
          : t("models.testConnectionError");
      setModelTestResults((current) => ({
        ...current,
        [modelId]: { success: false, message: errMsg },
      }));
      toast.error(errMsg);
    } finally {
      setTestingModelId(null);
    }
  };

  const handleProbeMultimodal = async (modelId: string) => {
    setProbingModelId(modelId);
    try {
      const result = await api.probeMultimodal(provider.id, modelId);
      const parts: string[] = [];
      if (result.supports_image) parts.push(t("models.probeImage"));
      if (result.supports_video) parts.push(t("models.probeVideo"));
      const probeMessage = result.message?.trim();
      if (parts.length > 0) {
        toast.success(
          t("models.probeSupported", {
            types: parts.join(", "),
            defaultValue: `支持: ${parts.join(", ")}`,
          }),
        );
      } else {
        const unsupported = t("models.probeNotSupported");
        toast.info(
          probeMessage ? `${unsupported} ${probeMessage}` : unsupported,
        );
      }
      await onSaved();
    } catch (error) {
      const errMsg =
        error instanceof Error ? error.message : t("models.probeFailed");
      toast.error(errMsg);
    } finally {
      setProbingModelId(null);
    }
  };

  const handleRemoveModel = (modelId: string, modelName: string) => {
    Modal.confirm({
      title: t("models.removeModel"),
      content: t("models.removeModelConfirm", {
        name: modelName,
        provider: provider.name,
      }),
      okText: t("common.delete"),
      danger: true,
      cancelText: t("models.cancel"),
      onOk: async () => {
        try {
          await api.removeModel(provider.id, modelId);
          toast.success(t("models.modelRemoved", { name: modelName }));
          await onSaved();
        } catch (error) {
          const errMsg =
            error instanceof Error
              ? error.message
              : t("models.modelRemoveFailed");
          toast.error(errMsg);
        }
      },
    });
  };

  const handleClose = () => {
    setAdding(false);
    setConfigOpenModelId(null);
    setModelTestResults({});
    reset();
    onClose();
  };

  useEffect(() => {
    if (isOpenRouter) {
      api
        .getOpenRouterSeries()
        .then((res: SeriesResponse) => {
          setAvailableSeries(res.series || []);
        })
        .catch(() => {
          setAvailableSeries([]);
        });
    }
  }, [isOpenRouter]);

  const handleFetchModels = async () => {
    if (!isOpenRouter) return;

    setLoadingFilters(true);
    try {
      const filterBody: Record<string, string[]> = {};
      if (selectedSeries.length > 0) {
        filterBody.providers = selectedSeries;
      }
      if (selectedInputModality) {
        filterBody.input_modalities = [selectedInputModality];
      }

      const result = await api.filterOpenRouterModels(filterBody);
      if (result.success) {
        setDiscoveredModels(result.models || []);
        toast.success(
          t("models.filteredModelsLoaded", { count: result.total_count }),
        );
      } else {
        toast.error(t("models.filterFailed"));
      }
    } catch {
      toast.error(t("models.filterFailed"));
    } finally {
      setLoadingFilters(false);
    }
  };

  const handleAddFilteredModel = async (model: DiscoveredOpenRouterModel) => {
    setSaving(true);
    try {
      const modelName = model.name ?? model.id;
      await api.addModel(provider.id, { id: model.id, name: modelName });
      toast.success(t("models.modelAdded", { name: modelName }));
      await onSaved();
      setDiscoveredModels((prev) => prev.filter((m) => m.id !== model.id));
    } catch {
      toast.error(t("models.modelAddFailed"));
    } finally {
      setSaving(false);
    }
  };

  useEffect(() => {
    if (!adding) {
      setDiscoveredModels([]);
      return;
    }
    setLoadingDiscoveredModels(true);
    api
      .discoverModels(provider.id)
      .then((res) => {
        setDiscoveredModels(res.models || []);
      })
      .catch(() => setDiscoveredModels([]))
      .finally(() => setLoadingDiscoveredModels(false));
  }, [adding, provider.id]);

  useEffect(() => {
    if (!isOpenRouter || !adding) return;
    setAdding(false);
    reset();
  }, [adding, reset, isOpenRouter]);

  const all_models = [
    ...(provider.models ?? []),
    ...(provider.extra_models ?? []),
  ];

  return (
    <Modal
      className="modal-fullscreen-mobile"
      title={t("models.manageModelsTitle", { provider: provider.name })}
      open={open}
      onClose={handleClose}
      footer={null}
      width={800}
      destroyOnClose
    >
      {}
      <div className={styles.modelList}>
        {all_models.length === 0 ? (
          <div className={styles.modelListEmpty}>{t("models.noModels")}</div>
        ) : (
          all_models.map((m) => {
            const isConfigOpen = configOpenModelId === m.id;
            const testResult = modelTestResults[m.id];
            return (
              <div key={m.id}>
                <div
                  className={`${styles.modelListItem} ${styles.modelListItemTwoRow}`}
                >
                  <div className={styles.modelListItemRow1}>
                    <span
                      className={styles.modelListItemName}
                      title={m.name || m.id}
                    >
                      {m.name || m.id}
                    </span>
                    <div className={styles.modelListItemMeta}>
                      {m.supports_image === true && (
                        <Tag
                          color="primary"
                          style={{ fontSize: 11, margin: 0 }}
                        >
                          {t("models.tagImage")}
                        </Tag>
                      )}
                      {m.supports_video === true && (
                        <Tag
                          color="primary"
                          style={{ fontSize: 11, margin: 0 }}
                        >
                          {t("models.tagVideo")}
                        </Tag>
                      )}
                      {m.supports_multimodal === false && (
                        <Tag style={{ fontSize: 11, margin: 0 }}>
                          {t("models.tagTextOnly")}
                        </Tag>
                      )}
                      {m.supports_multimodal === null && (
                        <Tag
                          color="default"
                          style={{ fontSize: 11, margin: 0 }}
                        >
                          {t("models.tagNotProbed")}
                        </Tag>
                      )}
                      {m.context_window != null && m.context_window > 0 && (
                        <Tag
                          color="primary"
                          style={{ fontSize: 11, margin: 0 }}
                        >
                          {m.context_window >= 1000000
                            ? `${(m.context_window / 1000000)
                                .toFixed(1)
                                .replace(/\.0$/, "")}M`
                            : m.context_window >= 1000
                            ? `${Math.round(m.context_window / 1000)}K`
                            : `${m.context_window}`}
                        </Tag>
                      )}
                    </div>
                  </div>
                  <div className={styles.modelListItemRow2}>
                    <Btn
                      variant="outline"
                      size="sm"
                      icon={<Eye />}
                      onClick={() => handleProbeMultimodal(m.id)}
                      loading={probingModelId === m.id}
                      style={{
                        color: isDark ? "rgba(255,255,255,0.65)" : undefined,
                      }}
                    >
                      {t("models.probeMultimodal")}
                    </Btn>
                    <Btn
                      variant="outline"
                      size="sm"
                      icon={<Plug />}
                      onClick={() => handleTestModel(m.id)}
                      loading={testingModelId === m.id}
                      style={{
                        color: isDark ? "rgba(255,255,255,0.65)" : undefined,
                      }}
                    >
                      {t("models.testConnection")}
                    </Btn>
                    <Btn
                      variant="outline"
                      size="sm"
                      icon={isConfigOpen ? <ChevronDown /> : <Settings />}
                      aria-label={t("models.modelSettings")}
                      onClick={() =>
                        setConfigOpenModelId(isConfigOpen ? null : m.id)
                      }
                      style={{
                        color: isDark ? "rgba(255,255,255,0.65)" : undefined,
                      }}
                    />
                    <Btn
                      variant="danger"
                      size="sm"
                      icon={<Trash2 />}
                      onClick={() => handleRemoveModel(m.id, m.name)}
                    />
                  </div>
                  {testResult && (
                    <InfoBox
                      type={testResult.success ? "success" : "error"}
                      title={t("models.connectionTestResult")}
                      style={{ width: "100%", boxSizing: "border-box" }}
                    >
                      {testResult.message}
                    </InfoBox>
                  )}
                </div>
                {isConfigOpen && (
                  <div
                    style={{
                      padding: "0 16px 12px",
                      borderBottom: isDark
                        ? "1px solid rgba(255,255,255,0.06)"
                        : "1px solid #f5f5f5",
                    }}
                  >
                    <ModelConfigEditor
                      providerId={provider.id}
                      model={m}
                      onSaved={onSaved}
                      onClose={() => setConfigOpenModelId(null)}
                    />
                  </div>
                )}
              </div>
            );
          })
        )}
      </div>

      {}
      {isOpenRouter && (
        <div style={{ marginTop: 16, marginBottom: 16 }}>
          <Btn
            variant={showFilters ? "filled" : "outline"}
            icon={<Filter />}
            onClick={() => setShowFilters(!showFilters)}
            style={{ width: "100%", marginBottom: showFilters ? 8 : 0 }}
          >
            {t("models.filterModels")}
          </Btn>

          {showFilters && (
            <div
              style={{
                padding: 12,
                background: "var(--bg-disabled)",
                borderRadius: 8,
              }}
            >
              {}
              <div style={{ marginBottom: 12 }}>
                <div style={{ marginBottom: 4, fontWeight: 500 }}>
                  {t("models.filterByProvider")}
                </div>
                <div style={{ display: "flex", flexWrap: "wrap", gap: 8 }}>
                  {availableSeries.map((s) => (
                    <Check
                      key={s}
                      checked={selectedSeries.includes(s)}
                      onChange={(e) => {
                        if (e.target.checked) {
                          setSelectedSeries([...selectedSeries, s]);
                        } else {
                          setSelectedSeries(
                            selectedSeries.filter((v) => v !== s),
                          );
                        }
                      }}
                    >
                      {s}
                    </Check>
                  ))}
                </div>
              </div>

              {}
              <div style={{ marginBottom: 12 }}>
                <div style={{ marginBottom: 4, fontWeight: 500 }}>
                  {t("models.filterByModality")}
                </div>
                <div style={{ display: "flex", flexWrap: "wrap", gap: 8 }}>
                  {(
                    [
                      {
                        label: (
                          <>
                            <ImageUp /> {t("models.modalityVision")}
                          </>
                        ),
                        value: "image",
                      },
                      {
                        label: (
                          <>
                            <AudioLines /> {t("models.modalityAudio")}
                          </>
                        ),
                        value: "audio",
                      },
                      {
                        label: (
                          <>
                            <Film /> {t("models.modalityVideo")}
                          </>
                        ),
                        value: "video",
                      },
                      {
                        label: (
                          <>
                            <FileUp /> {t("models.modalityFile")}
                          </>
                        ),
                        value: "file",
                      },
                      {
                        label: (
                          <>
                            <Type /> {t("models.modalityText")}
                          </>
                        ),
                        value: "text",
                      },
                    ] as const
                  ).map((opt) => (
                    <Check
                      key={opt.value}
                      checked={selectedInputModality === opt.value}
                      onChange={(e) =>
                        setSelectedInputModality(
                          e.target.checked ? opt.value : null,
                        )
                      }
                    >
                      {opt.label}
                    </Check>
                  ))}
                </div>
              </div>

              {}
              <Btn
                onClick={handleFetchModels}
                loading={loadingFilters}
                style={{ width: "100%" }}
              >
                {t("models.getModels")}
              </Btn>

              {}
              {discoveredModels.length > 0 && (
                <div
                  style={{
                    marginTop: 12,
                    maxHeight: "none",
                    overflowY: "auto",
                  }}
                >
                  <div style={{ fontWeight: 500, marginBottom: 4 }}>
                    {t("models.discovered")}
                  </div>
                  {discoveredModels.map((model) => (
                    <div
                      key={model.id}
                      style={{
                        display: "flex",
                        justifyContent: "space-between",
                        alignItems: "center",
                        padding: "4px 8px",
                        background: "var(--surface-2)",
                        marginBottom: 4,
                        borderRadius: 4,
                      }}
                    >
                      <div>
                        <div style={{ fontWeight: 500 }}>{model.name}</div>
                        <div
                          style={{
                            fontSize: 11,
                            color: "var(--text-3)",
                            display: "flex",
                            alignItems: "center",
                            gap: 4,
                          }}
                        >
                          <span>{model.provider}</span>
                          {model.input_modalities?.includes("text") && (
                            <Type style={{ fontSize: 12 }} />
                          )}
                          {model.input_modalities?.includes("image") && (
                            <ImageUp style={{ fontSize: 12 }} />
                          )}
                          {model.input_modalities?.includes("audio") && (
                            <AudioLines style={{ fontSize: 12 }} />
                          )}
                          {model.input_modalities?.includes("video") && (
                            <Film style={{ fontSize: 12 }} />
                          )}
                          {model.input_modalities?.includes("file") && (
                            <FileUp style={{ fontSize: 12 }} />
                          )}
                          {model.output_modalities?.includes("image") && (
                            <ImagePlus
                              style={{ fontSize: 12, color: "purple" }}
                            />
                          )}
                          {model.pricing?.prompt && (
                            <span style={{ color: "green", marginLeft: 4 }}>
                              $
                              {(
                                parseFloat(model.pricing.prompt) * 1_000_000
                              ).toFixed(2)}
                              {t("models.perMillionIn")}
                              {model.pricing?.completion && (
                                <span>
                                  {" "}
                                  · $
                                  {(
                                    parseFloat(model.pricing.completion) *
                                    1_000_000
                                  ).toFixed(2)}
                                  {t("models.perMillionOut")}
                                </span>
                              )}
                            </span>
                          )}
                        </div>
                      </div>
                      <Btn
                        size="sm"
                        onClick={() => handleAddFilteredModel(model)}
                        disabled={saving}
                      >
                        {t("models.add")}
                      </Btn>
                    </div>
                  ))}
                </div>
              )}
            </div>
          )}
        </div>
      )}

      {}
      {!isOpenRouter &&
        (adding ? (
          <div className={styles.modelAddForm}>
            <div
              style={{
                display: "flex",
                flexDirection: "column",
                gap: 12,
                marginBottom: 0,
              }}
            >
              <FormField
                label={t("models.modelIdLabel")}
                error={errors.id?.message}
                required
              >
                <Controller
                  name="id"
                  control={control}
                  rules={{ required: t("models.modelIdLabel") }}
                  render={({ field }) => (
                    <Select
                      showSearch
                      allowCustomValue
                      value={field.value || undefined}
                      onChange={(v) => field.onChange(v as string)}
                      placeholder={t("models.modelIdPlaceholder")}
                      options={discoveredModels.map((model) => ({
                        value: model.id,
                        label: model.id,
                      }))}
                      filterOption={(inputValue, option) =>
                        String(option.value)
                          .toLowerCase()
                          .includes(inputValue.toLowerCase())
                      }
                    />
                  )}
                />
                <div
                  style={{ fontSize: 12, color: "var(--text-3)", marginTop: 4 }}
                >
                  {loadingDiscoveredModels || discoveredModels.length > 0
                    ? t("models.modelIdInputHint")
                    : t("models.modelDiscoveryUnavailableHint")}
                </div>
              </FormField>
              <FormField label={t("models.modelNameLabel")}>
                <InputWithExample
                  {...register("name")}
                  placeholder={t("models.modelNamePlaceholder")}
                  example="GPT-4o"
                />
              </FormField>
              <div
                style={{ display: "flex", gap: 8, justifyContent: "flex-end" }}
              >
                <Btn
                  size="sm"
                  onClick={() => {
                    setAdding(false);
                    reset();
                  }}
                >
                  {t("models.cancel")}
                </Btn>
                <Btn
                  size="sm"
                  loading={saving}
                  onClick={handleSubmit(handleAddModel)}
                >
                  {t("models.addModel")}
                </Btn>
              </div>
            </div>
          </div>
        ) : (
          <div style={{ display: "flex", gap: 8, marginTop: 12 }}>
            <Btn
              variant="outline"
              icon={<Plus />}
              onClick={() => setAdding(true)}
              style={{ flex: 1 }}
            >
              {t("models.addModel")}
            </Btn>
          </div>
        ))}
    </Modal>
  );
}
