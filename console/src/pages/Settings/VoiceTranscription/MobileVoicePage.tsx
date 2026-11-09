import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { PageHeader } from "@/components/PageHeader";
import { Page, PageBody } from "@/components/Page";
import { RuaCard } from "@/components/RuaCard";
import {
  DEFAULT_BROWSER_TTS_CONFIG,
  isBrowserTTSSupported,
  loadBrowserTTSConfig,
  saveBrowserTTSConfig,
  speakWithBrowser,
  cancelBrowserTTS,
  subscribeBrowserVoices,
  getBrowserVoices,
  type BrowserTTSConfig,
} from "@/utils/browserTTS";
import styles from "./index.module.less";
import { Save, Volume2 } from "lucide-react";
import { Btn, Select, Slider, Toggle, Tooltip, toast } from "@/ui";

const VOICE_LANG_PREFIXES = ["zh", "en", "ja", "ru"];

function filterRelevantVoices(
  voices: SpeechSynthesisVoice[],
): SpeechSynthesisVoice[] {
  return voices.filter((v) => {
    const l = (v.lang || "").toLowerCase();
    return VOICE_LANG_PREFIXES.some((p) => l.startsWith(p));
  });
}

export function MobileVoicePage() {
  const { t } = useTranslation();
  const supported = isBrowserTTSSupported();

  const [cfg, setCfg] = useState<BrowserTTSConfig>(() =>
    supported ? loadBrowserTTSConfig() : { ...DEFAULT_BROWSER_TTS_CONFIG },
  );
  const [voices, setVoices] = useState<SpeechSynthesisVoice[]>(() =>
    supported ? filterRelevantVoices(getBrowserVoices()) : [],
  );
  const [testing, setTesting] = useState(false);
  const handleRef = useRef<{ cancel: () => void } | null>(null);

  useEffect(() => {
    if (!supported) return;
    return subscribeBrowserVoices((vs) => setVoices(filterRelevantVoices(vs)));
  }, [supported]);

  useEffect(() => {
    return () => {
      handleRef.current?.cancel();
      cancelBrowserTTS();
    };
  }, []);

  const updateCfg = (patch: Partial<BrowserTTSConfig>) => {
    setCfg((prev) => ({ ...prev, ...patch }));
  };

  const handleSave = () => {
    saveBrowserTTSConfig(cfg);
    toast.success(t("voiceTranscription.saveSuccess"));
  };

  const handleReset = () => {
    const fresh = supported
      ? loadBrowserTTSConfig()
      : DEFAULT_BROWSER_TTS_CONFIG;
    setCfg({ ...fresh });
  };

  const handleTest = () => {
    if (!supported) {
      toast.error(t("ttsMobile.unsupported"));
      return;
    }
    handleRef.current?.cancel();
    setTesting(true);
    const handle = speakWithBrowser({
      text: t("ttsMobile.testPhrase"),
      voiceURI: cfg.voiceURI,
      lang: cfg.lang,
      rate: cfg.rate,
      pitch: cfg.pitch,
      volume: cfg.volume,
    });
    handleRef.current = handle;
    handle.promise
      .catch((err) => {
        toast.error(
          err instanceof Error ? err.message : t("ttsMobile.testFailed"),
        );
      })
      .finally(() => setTesting(false));
  };

  const voiceOptions = useMemo(
    () =>
      voices.map((v) => ({
        value: v.voiceURI,
        label: `${v.name} (${v.lang})`,
      })),
    [voices],
  );

  return (
    <Page className={styles.voiceTranscriptionPage}>
      <PageHeader
        items={[{ title: t("voiceSettings.title") }]}
        onRefresh={handleReset}
        extra={
          <div style={{ display: "flex", gap: "var(--space-2)" }}>
            <Tooltip title={t("common.save")}>
              <Btn onClick={handleSave} icon={<Save />} />
            </Tooltip>
          </div>
        }
      />

      <PageBody className={styles.content}>
        {}
        <RuaCard className={styles.card} bare>
          {!supported && (
            <div className={styles.section}>
              <div className={styles.sectionTitle}>
                {t("ttsMobile.unsupported")}
              </div>
              <p className={styles.cardDescription}>
                {t("ttsMobile.unsupportedDesc")}
              </p>
            </div>
          )}

          <div className={styles.section}>
            <div className={styles.sectionTitle}>
              {t("ttsMobile.engineLabel")}
            </div>
            <p className={styles.cardDescription}>
              {t("ttsMobile.engineDesc")}
            </p>
            <Toggle
              checked={cfg.enabled}
              disabled={!supported}
              onChange={(e) => updateCfg({ enabled: e.target.checked })}
            />
          </div>

          <div className={styles.section}>
            <div className={styles.sectionTitle}>
              {t("ttsMobile.voiceLabel")}
            </div>
            <p className={styles.cardDescription}>{t("ttsMobile.voiceDesc")}</p>
            {voiceOptions.length === 0 ? (
              <p className={styles.cardDescription}>
                {t("ttsMobile.voiceNoneFound")}
              </p>
            ) : (
              <Select
                value={cfg.voiceURI || undefined}
                onChange={(v: string) => {
                  const match = voices.find((x) => x.voiceURI === v);
                  updateCfg({ voiceURI: v, lang: match?.lang ?? cfg.lang });
                }}
                placeholder={t("ttsMobile.voicePlaceholder")}
                style={{ width: "100%", maxWidth: 400 }}
                showSearch
                optionFilterProp="children"
                disabled={!supported || !cfg.enabled}
              >
                {voiceOptions.map((opt) => (
                  <Select.Option key={opt.value} value={opt.value}>
                    {opt.label}
                  </Select.Option>
                ))}
              </Select>
            )}
          </div>

          <div className={styles.section}>
            <div className={styles.sectionTitle}>
              {t("ttsMobile.rateLabel")}
            </div>
            <p className={styles.cardDescription}>{t("ttsMobile.rateDesc")}</p>
            <Slider
              min={0.5}
              max={2}
              step={0.05}
              value={cfg.rate}
              onChange={(v: number) => updateCfg({ rate: v })}
              marks={{ 0.5: "0.5x", 1: "1x", 1.5: "1.5x", 2: "2x" }}
              disabled={!supported || !cfg.enabled}
              style={{ maxWidth: 400 }}
            />
          </div>

          <div className={styles.section}>
            <div className={styles.sectionTitle}>
              {t("ttsMobile.pitchLabel")}
            </div>
            <p className={styles.cardDescription}>{t("ttsMobile.pitchDesc")}</p>
            <Slider
              min={0.5}
              max={1.5}
              step={0.05}
              value={cfg.pitch}
              onChange={(v: number) => updateCfg({ pitch: v })}
              marks={{ 0.5: "0.5", 1: "1", 1.5: "1.5" }}
              disabled={!supported || !cfg.enabled}
              style={{ maxWidth: 400 }}
            />
          </div>

          <div className={styles.section}>
            <div className={styles.sectionTitle}>
              {t("ttsMobile.volumeLabel")}
            </div>
            <p className={styles.cardDescription}>
              {t("ttsMobile.volumeDesc")}
            </p>
            <Slider
              min={0}
              max={1}
              step={0.05}
              value={cfg.volume}
              onChange={(v: number) => updateCfg({ volume: v })}
              marks={{ 0: "0", 0.5: "0.5", 1: "1" }}
              disabled={!supported || !cfg.enabled}
              style={{ maxWidth: 400 }}
            />
          </div>

          <div className={styles.section}>
            <div className={styles.sectionTitle}>
              {t("ttsMobile.testLabel")}
            </div>
            <p className={styles.cardDescription}>{t("ttsMobile.testDesc")}</p>
            <div style={{ display: "flex", gap: "var(--space-2)" }}>
              <Btn
                icon={<Volume2 />}
                onClick={handleTest}
                loading={testing}
                disabled={!supported || !cfg.enabled}
              >
                {testing
                  ? t("ttsMobile.testButtonStop")
                  : t("ttsMobile.testButton")}
              </Btn>
              {testing && (
                <Btn
                  onClick={() => {
                    handleRef.current?.cancel();
                    cancelBrowserTTS();
                    setTesting(false);
                  }}
                >
                  {t("ttsMobile.stop")}
                </Btn>
              )}
            </div>
          </div>
        </RuaCard>
      </PageBody>
    </Page>
  );
}

export default MobileVoicePage;
