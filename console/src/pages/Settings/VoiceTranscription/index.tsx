/* eslint-disable react-refresh/only-export-components */
import { useEffect, useMemo, useState } from "react";
import { useLoaderData } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { useForm, Controller } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import api from "../../../api";
import { PageHeader } from "@/components/PageHeader";
import { Page, PageBody } from "@/components/Page";
import { RuaCard } from "@/components/RuaCard";
import Skeleton from "@/components/Skeleton";
import { isMobile } from "@/utils/isMobile";
import { MobileVoicePage } from "./MobileVoicePage";
import styles from "./index.module.less";
import { Save } from "lucide-react";
import {
  Btn,
  FormField,
  InfoBox,
  Radio,
  RadioGroup,
  Select,
  Slider,
  Tooltip,
  toast,
} from "@/ui";

interface TranscriptionProvider {
  id: string;
  name: string;
  available: boolean;
  type?: string;
}

interface LocalWhisperStatus {
  available: boolean;
  ffmpeg_installed: boolean;
  whisper_installed: boolean;
}

const OPENAI_VOICES = [
  { value: "alloy", label: "Alloy" },
  { value: "echo", label: "Echo" },
  { value: "fable", label: "Fable" },
  { value: "onyx", label: "Onyx" },
  { value: "nova", label: "Nova" },
  { value: "shimmer", label: "Shimmer" },
];

const DASHSCOPE_VOICES = [
  { value: "Cherry", label: "Cherry (芊悦 - 阳光女声)" },
  { value: "Serena", label: "Serena (苏瑶 - 温柔女声)" },
  { value: "Ethan", label: "Ethan (晨煦 - 阳光男声)" },
  { value: "Chelsie", label: "Chelsie (千雪 - 甜美女声)" },
  { value: "Bella", label: "Bella" },
  { value: "Ryan", label: "Ryan" },
  { value: "Kai", label: "Kai (凯 - 磁性男声)" },
  { value: "Vivian", label: "Vivian (十三 - 御姐)" },
  { value: "Moon", label: "Moon (月白 - 潮流男声)" },
  { value: "Maia", label: "Maia (四月 - 知性女声)" },
];

const DASHSCOPE_MODELS = [
  { value: "qwen3-tts-flash", label: "Qwen3-TTS-Flash" },
];

const OPENAI_MODELS = [
  { value: "tts-1", label: "TTS-1 (Fast)" },
  { value: "tts-1-hd", label: "TTS-1 HD (High Quality)" },
];

interface VoiceSettingsInitialData {
  mode: Awaited<ReturnType<typeof api.getAudioMode>>;
  providerType: Awaited<ReturnType<typeof api.getTranscriptionProviderType>>;
  providers: Awaited<ReturnType<typeof api.getTranscriptionProviders>>;
  localWhisperStatus: Awaited<ReturnType<typeof api.getLocalWhisperStatus>>;
  ttsConfig: Awaited<ReturnType<typeof api.getTTSConfig>>;
  ttsProviders: Awaited<ReturnType<typeof api.getTTSProviders>>;
}

interface VoiceSettingsPageProps {
  initialData?: VoiceSettingsInitialData | null;
}

export async function loader(): Promise<VoiceSettingsInitialData | null> {
  if (isMobile()) return null;
  const [
    mode,
    providerType,
    providers,
    localWhisperStatus,
    ttsConfig,
    ttsProviders,
  ] = await Promise.all([
    api.getAudioMode(),
    api.getTranscriptionProviderType(),
    api.getTranscriptionProviders(),
    api.getLocalWhisperStatus(),
    api.getTTSConfig(),
    api.getTTSProviders(),
  ]);
  return {
    mode,
    providerType,
    providers,
    localWhisperStatus,
    ttsConfig,
    ttsProviders,
  };
}

export function RouteComponent() {
  return (
    <VoiceSettingsPage
      initialData={useLoaderData() as VoiceSettingsInitialData | null}
    />
  );
}

function VoiceSettingsPage({ initialData }: VoiceSettingsPageProps) {
  const isMobileMode = useMemo(() => isMobile(), []);
  if (isMobileMode) {
    return <MobileVoicePage />;
  }
  return <DesktopVoiceSettingsPage initialData={initialData ?? undefined} />;
}

const voiceSettingsSchema = z.object({
  audioMode: z.string(),
  providerType: z.string(),
  selectedProviderId: z.string(),
  ttsProviderType: z.string(),
  ttsProviderId: z.string(),
  ttsModel: z.string(),
  ttsVoice: z.string(),
  ttsSpeed: z.number(),
  selectedBrowserVoice: z.string(),
});

type VoiceSettingsForm = z.infer<typeof voiceSettingsSchema>;

const FORM_DEFAULTS: VoiceSettingsForm = {
  audioMode: "auto",
  providerType: "disabled",
  selectedProviderId: "",
  ttsProviderType: "browser",
  ttsProviderId: "",
  ttsModel: "tts-1",
  ttsVoice: "alloy",
  ttsSpeed: 1.0,
  selectedBrowserVoice: localStorage.getItem("tts_browser_voice") || "",
};

function DesktopVoiceSettingsPage({
  initialData,
}: {
  initialData?: VoiceSettingsInitialData;
}) {
  const { t } = useTranslation();
  const [loading, setLoading] = useState(initialData === undefined);
  const [saving, setSaving] = useState(false);

  const { control, watch, reset, setValue, getValues } =
    useForm<VoiceSettingsForm>({
      resolver: zodResolver(voiceSettingsSchema),
      defaultValues: FORM_DEFAULTS,
    });

  const [providers, setProviders] = useState<TranscriptionProvider[]>([]);
  const [localWhisperStatus, setLocalWhisperStatus] =
    useState<LocalWhisperStatus | null>(null);
  const [ttsProviders, setTtsProviders] = useState<TranscriptionProvider[]>([]);
  const [browserVoices, setBrowserVoices] = useState<SpeechSynthesisVoice[]>(
    [],
  );

  const audioMode = watch("audioMode");
  const providerType = watch("providerType");
  const ttsProviderType = watch("ttsProviderType");
  const ttsProviderId = watch("ttsProviderId");

  const applySettings = (data: VoiceSettingsInitialData) => {
    setProviders(data.providers.providers ?? []);
    setLocalWhisperStatus(data.localWhisperStatus);
    setTtsProviders(data.ttsProviders.providers ?? []);
    reset({
      audioMode: data.mode.audio_mode ?? "auto",
      providerType: data.providerType.transcription_provider_type ?? "disabled",
      selectedProviderId: data.providers.configured_provider_id ?? "",
      ttsProviderType: data.ttsConfig.tts_provider_type ?? "browser",
      ttsProviderId: data.ttsConfig.tts_provider_id ?? "",
      ttsModel: data.ttsConfig.tts_model ?? "tts-1",
      ttsVoice: data.ttsConfig.tts_voice ?? "alloy",
      ttsSpeed: data.ttsConfig.tts_speed ?? 1.0,
      selectedBrowserVoice: localStorage.getItem("tts_browser_voice") || "",
    });
  };

  useEffect(() => {
    if (typeof window === "undefined" || !window.speechSynthesis) return;
    const loadVoices = () => {
      setBrowserVoices(window.speechSynthesis.getVoices());
    };
    loadVoices();
    window.speechSynthesis.addEventListener("voiceschanged", loadVoices);
    return () => {
      window.speechSynthesis.removeEventListener("voiceschanged", loadVoices);
    };
  }, []);

  const fetchSettings = async () => {
    setLoading(true);
    try {
      const [modeRes, provTypeRes, provRes, lwStatus, ttsConfigRes, ttsProv] =
        await Promise.all([
          api.getAudioMode(),
          api.getTranscriptionProviderType(),
          api.getTranscriptionProviders(),
          api.getLocalWhisperStatus(),
          api.getTTSConfig(),
          api.getTTSProviders(),
        ]);
      setProviders(provRes.providers ?? []);
      setLocalWhisperStatus(lwStatus);
      setTtsProviders(ttsProv.providers ?? []);

      reset({
        audioMode: modeRes.audio_mode ?? "auto",
        providerType: provTypeRes.transcription_provider_type ?? "disabled",
        selectedProviderId: provRes.configured_provider_id ?? "",
        ttsProviderType: ttsConfigRes.tts_provider_type ?? "browser",
        ttsProviderId: ttsConfigRes.tts_provider_id ?? "",
        ttsModel: ttsConfigRes.tts_model ?? "tts-1",
        ttsVoice: ttsConfigRes.tts_voice ?? "alloy",
        ttsSpeed: ttsConfigRes.tts_speed ?? 1.0,
        selectedBrowserVoice: localStorage.getItem("tts_browser_voice") || "",
      });
    } catch (err) {
      console.error("Failed to load voice settings:", err);
      toast.error(t("voiceTranscription.loadFailed"));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    if (initialData) {
      applySettings(initialData);
      setLoading(false);
      return;
    }
    void fetchSettings();

    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [initialData]);

  const handleSave = async () => {
    const values = getValues();
    setSaving(true);
    try {
      const promises: Promise<unknown>[] = [
        api.updateAudioMode(values.audioMode),
        api.updateTranscriptionProviderType(values.providerType),

        api.updateTTSConfig({
          tts_provider_type: values.ttsProviderType,
          tts_provider_id: values.ttsProviderId,
          tts_model: values.ttsModel,
          tts_voice: values.ttsVoice,
          tts_speed: values.ttsSpeed,
        }),
      ];
      if (values.providerType === "whisper_api") {
        promises.push(
          api.updateTranscriptionProvider(values.selectedProviderId),
        );
      }
      await Promise.all(promises);

      if (values.ttsProviderType === "browser") {
        localStorage.setItem("tts_browser_voice", values.selectedBrowserVoice);
      }

      toast.success(t("voiceTranscription.saveSuccess"));
    } catch (err) {
      console.error("Failed to save voice settings:", err);
      toast.error(t("voiceTranscription.saveFailed"));
    } finally {
      setSaving(false);
    }
  };

  if (loading) {
    return (
      <Page className={styles.voiceTranscriptionPage}>
        <Skeleton variant="detail" count={2} />
      </Page>
    );
  }

  const availableProviders = providers.filter((p) => p.available);
  const availableTtsProviders = ttsProviders.filter((p) => p.available);
  const showProviderSection = audioMode !== "native";
  const isLocalWhisper = providerType === "local_whisper";
  const isWhisperApi = providerType === "whisper_api";

  const selectedTtsProvider = availableTtsProviders.find(
    (p) => p.id === ttsProviderId,
  );
  const isDashScope = selectedTtsProvider?.type === "dashscope";
  const currentModels = isDashScope ? DASHSCOPE_MODELS : OPENAI_MODELS;
  const currentVoices = isDashScope ? DASHSCOPE_VOICES : OPENAI_VOICES;

  return (
    <Page className={styles.voiceTranscriptionPage}>
      <PageHeader
        items={[{ title: t("voiceSettings.title") }]}
        onRefresh={fetchSettings}
        refreshing={saving}
        extra={
          <div style={{ display: "flex", gap: "var(--space-2)" }}>
            <Tooltip title={t("common.save")}>
              <Btn
                onClick={handleSave}
                loading={saving}
                aria-label={t("common.save")}
                icon={<Save />}
              />
            </Tooltip>
          </div>
        }
      />

      <PageBody>
        <div className={styles.formContainer}>
          <RuaCard className={styles.formCard} bare>
            <div className={styles.form}>
              {}
              <div
                className={`rua-divider-text rua-divider-text--left ${styles.sectionDivider}`}
              >
                {t("voiceSettings.sttSectionTitle")}
              </div>

              <InfoBox
                type="warning"
                title={t("voiceTranscription.transcriptionInfoTitle")}
                style={{ marginBottom: 16 }}
              >
                {isLocalWhisper
                  ? t("voiceTranscription.transcriptionInfoDescLocal")
                  : t("voiceTranscription.transcriptionInfoDesc")}
              </InfoBox>

              <FormField label={t("voiceTranscription.audioModeLabel")}>
                <p className={styles.fieldDescription}>
                  {t("voiceTranscription.audioModeDescription")}
                </p>
                <Controller
                  name="audioMode"
                  control={control}
                  render={({ field }) => (
                    <RadioGroup
                      value={field.value}
                      onChange={(value) => field.onChange(value)}
                    >
                      <div
                        style={{
                          display: "flex",
                          flexDirection: "column",
                          gap: "var(--space-3)",
                        }}
                      >
                        <Radio value="auto">
                          <span className={styles.optionLabel}>
                            {t("voiceTranscription.modeAuto")}
                          </span>
                          <span className={styles.optionDescription}>
                            {t("voiceTranscription.modeAutoDesc")}
                          </span>
                        </Radio>
                        <Radio value="native">
                          <span className={styles.optionLabel}>
                            {t("voiceTranscription.modeNative")}
                          </span>
                          <span className={styles.optionDescription}>
                            {t("voiceTranscription.modeNativeDesc")}
                          </span>
                        </Radio>
                      </div>
                    </RadioGroup>
                  )}
                />

                {audioMode === "native" && localWhisperStatus && (
                  <div style={{ marginTop: 12 }}>
                    {localWhisperStatus.ffmpeg_installed ? (
                      <InfoBox
                        type="success"
                        title={t("voiceTranscription.ffmpegReady")}
                      />
                    ) : (
                      <InfoBox
                        type="warning"
                        title={t("voiceTranscription.ffmpegMissing")}
                      >
                        {t("voiceTranscription.ffmpegMissingDesc")}
                      </InfoBox>
                    )}
                  </div>
                )}
              </FormField>

              {showProviderSection && (
                <>
                  <FormField label={t("voiceTranscription.providerTypeLabel")}>
                    <p className={styles.fieldDescription}>
                      {t("voiceTranscription.providerTypeDescription")}
                    </p>
                    <Controller
                      name="providerType"
                      control={control}
                      render={({ field }) => (
                        <RadioGroup
                          value={field.value}
                          onChange={(value) => field.onChange(value)}
                        >
                          <div
                            style={{
                              display: "flex",
                              flexDirection: "column",
                              gap: "var(--space-3)",
                            }}
                          >
                            <Radio value="disabled">
                              <span className={styles.optionLabel}>
                                {t("voiceTranscription.providerTypeDisabled")}
                              </span>
                              <span className={styles.optionDescription}>
                                {t(
                                  "voiceTranscription.providerTypeDisabledDesc",
                                )}
                              </span>
                            </Radio>
                            <Radio value="whisper_api">
                              <span className={styles.optionLabel}>
                                {t("voiceTranscription.providerTypeWhisperApi")}
                              </span>
                              <span className={styles.optionDescription}>
                                {t(
                                  "voiceTranscription.providerTypeWhisperApiDesc",
                                )}
                              </span>
                            </Radio>
                            <Radio value="local_whisper">
                              <span className={styles.optionLabel}>
                                {t(
                                  "voiceTranscription.providerTypeLocalWhisper",
                                )}
                              </span>
                              <span className={styles.optionDescription}>
                                {t(
                                  "voiceTranscription.providerTypeLocalWhisperDesc",
                                )}
                              </span>
                            </Radio>
                          </div>
                        </RadioGroup>
                      )}
                    />

                    {isLocalWhisper && localWhisperStatus && (
                      <div style={{ marginTop: 12 }}>
                        {localWhisperStatus.available ? (
                          <InfoBox
                            type="success"
                            title={t("voiceTranscription.localWhisperReady")}
                          />
                        ) : (
                          <InfoBox
                            type="warning"
                            title={t("voiceTranscription.localWhisperMissing")}
                          >
                            {t("voiceTranscription.localWhisperMissingDesc", {
                              ffmpeg: localWhisperStatus.ffmpeg_installed
                                ? t("common.enabled")
                                : t("common.disabled"),
                              whisper: localWhisperStatus.whisper_installed
                                ? t("common.enabled")
                                : t("common.disabled"),
                            })}
                          </InfoBox>
                        )}
                      </div>
                    )}
                  </FormField>

                  {isWhisperApi && (
                    <FormField label={t("voiceTranscription.providerLabel")}>
                      <p className={styles.fieldDescription}>
                        {t("voiceTranscription.providerDescription")}
                      </p>
                      {availableProviders.length === 0 ? (
                        <InfoBox
                          type="warning"
                          title={t("voiceTranscription.noProvidersWarning")}
                        />
                      ) : (
                        <Controller
                          name="selectedProviderId"
                          control={control}
                          render={({ field }) => (
                            <Select
                              value={field.value || undefined}
                              onChange={field.onChange}
                              placeholder={t(
                                "voiceTranscription.providerPlaceholder",
                              )}
                              style={{ width: "100%", maxWidth: 400 }}
                            >
                              {availableProviders.map((p) => (
                                <Select.Option key={p.id} value={p.id}>
                                  {p.name}
                                </Select.Option>
                              ))}
                            </Select>
                          )}
                        />
                      )}
                    </FormField>
                  )}
                </>
              )}

              {}
              <div
                className={`rua-divider-text rua-divider-text--left ${styles.sectionDivider}`}
              >
                {t("voiceSettings.ttsSectionTitle")}
              </div>

              <FormField label={t("tts.engineLabel")}>
                <p className={styles.fieldDescription}>
                  {t("tts.engineDescription")}
                </p>
                <Controller
                  name="ttsProviderType"
                  control={control}
                  render={({ field }) => (
                    <RadioGroup
                      value={field.value}
                      onChange={(value) => field.onChange(value)}
                    >
                      <div
                        style={{
                          display: "flex",
                          flexDirection: "column",
                          gap: "var(--space-3)",
                        }}
                      >
                        <Radio value="disabled">
                          <span className={styles.optionLabel}>
                            {t("tts.engineDisabled")}
                          </span>
                          <span className={styles.optionDescription}>
                            {t("tts.engineDisabledDesc")}
                          </span>
                        </Radio>
                        <Radio value="browser">
                          <span className={styles.optionLabel}>
                            {t("tts.engineBrowser")}
                          </span>
                          <span className={styles.optionDescription}>
                            {t("tts.engineBrowserDesc")}
                          </span>
                        </Radio>
                        <Radio value="openai_tts">
                          <span className={styles.optionLabel}>
                            {t("tts.engineOpenAI")}
                          </span>
                          <span className={styles.optionDescription}>
                            {t("tts.engineOpenAIDesc")}
                          </span>
                        </Radio>
                      </div>
                    </RadioGroup>
                  )}
                />
              </FormField>

              {ttsProviderType === "browser" && browserVoices.length > 0 && (
                <FormField label={t("tts.browserVoiceLabel")}>
                  <p className={styles.fieldDescription}>
                    {t("tts.browserVoiceDesc")}
                  </p>
                  <Controller
                    name="selectedBrowserVoice"
                    control={control}
                    render={({ field }) => (
                      <Select
                        value={field.value || undefined}
                        onChange={field.onChange}
                        placeholder={t("tts.browserVoicePlaceholder")}
                        style={{ width: "100%", maxWidth: 400 }}
                        showSearch
                        optionFilterProp="children"
                      >
                        {browserVoices.map((v) => (
                          <Select.Option key={v.name} value={v.name}>
                            {v.name} ({v.lang})
                          </Select.Option>
                        ))}
                      </Select>
                    )}
                  />
                </FormField>
              )}

              {ttsProviderType === "openai_tts" && (
                <>
                  <FormField label={t("tts.providerLabel")}>
                    <p className={styles.fieldDescription}>
                      {t("tts.providerDesc")}
                    </p>
                    {availableTtsProviders.length === 0 ? (
                      <InfoBox
                        type="warning"
                        title={t("tts.noProvidersWarning")}
                      />
                    ) : (
                      <Controller
                        name="ttsProviderId"
                        control={control}
                        render={({ field }) => (
                          <Select
                            value={field.value || undefined}
                            onChange={(id: string) => {
                              field.onChange(id);
                              const prov = availableTtsProviders.find(
                                (p) => p.id === id,
                              );
                              if (prov?.type === "dashscope") {
                                setValue("ttsModel", "qwen3-tts-flash");
                                setValue("ttsVoice", "Cherry");
                              } else {
                                setValue("ttsModel", "tts-1");
                                setValue("ttsVoice", "alloy");
                              }
                            }}
                            placeholder={t("tts.providerPlaceholder")}
                            style={{ width: "100%", maxWidth: 400 }}
                          >
                            {availableTtsProviders.map((p) => (
                              <Select.Option key={p.id} value={p.id}>
                                {p.name}
                              </Select.Option>
                            ))}
                          </Select>
                        )}
                      />
                    )}
                  </FormField>

                  <FormField label={t("tts.modelLabel")}>
                    <p className={styles.fieldDescription}>
                      {t("tts.modelDesc")}
                    </p>
                    <Controller
                      name="ttsModel"
                      control={control}
                      render={({ field }) => (
                        <Select
                          value={field.value}
                          onChange={field.onChange}
                          style={{ width: "100%", maxWidth: 400 }}
                        >
                          {currentModels.map((m) => (
                            <Select.Option key={m.value} value={m.value}>
                              {m.label}
                            </Select.Option>
                          ))}
                        </Select>
                      )}
                    />
                  </FormField>

                  <FormField label={t("tts.voiceLabel")}>
                    <p className={styles.fieldDescription}>
                      {t("tts.voiceDesc")}
                    </p>
                    <Controller
                      name="ttsVoice"
                      control={control}
                      render={({ field }) => (
                        <Select
                          value={field.value}
                          onChange={field.onChange}
                          style={{ width: "100%", maxWidth: 400 }}
                          showSearch
                        >
                          {currentVoices.map((v) => (
                            <Select.Option key={v.value} value={v.value}>
                              {v.label}
                            </Select.Option>
                          ))}
                        </Select>
                      )}
                    />
                  </FormField>

                  <FormField label={t("tts.speedLabel")}>
                    <p className={styles.fieldDescription}>
                      {t("tts.speedDesc")}
                    </p>
                    <Controller
                      name="ttsSpeed"
                      control={control}
                      render={({ field }) => (
                        <Slider
                          min={0.25}
                          max={4.0}
                          step={0.25}
                          value={field.value}
                          onChange={field.onChange}
                          marks={{
                            0.25: "0.25x",
                            1: "1x",
                            2: "2x",
                            4: "4x",
                          }}
                          style={{ maxWidth: 400 }}
                        />
                      )}
                    />
                  </FormField>
                </>
              )}
            </div>
          </RuaCard>
        </div>
      </PageBody>
    </Page>
  );
}

export default VoiceSettingsPage;
