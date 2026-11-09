export interface BrowserTTSConfig {
  enabled: boolean;
  voiceURI: string;
  lang: string;
  rate: number;
  pitch: number;
  volume: number;
}

export const MOBILE_TTS_CONFIG_KEY = "mobile-tts-config";
const LEGACY_VOICE_NAME_KEY = "tts_browser_voice";

export const DEFAULT_BROWSER_TTS_CONFIG: BrowserTTSConfig = {
  enabled: true,
  voiceURI: "",
  lang: "",
  rate: 1.0,
  pitch: 1.0,
  volume: 1.0,
};

export function isBrowserTTSSupported(): boolean {
  return typeof window !== "undefined" && "speechSynthesis" in window;
}

export function getBrowserVoices(): SpeechSynthesisVoice[] {
  if (!isBrowserTTSSupported()) return [];
  return window.speechSynthesis.getVoices();
}

export function subscribeBrowserVoices(
  cb: (voices: SpeechSynthesisVoice[]) => void,
): () => void {
  if (!isBrowserTTSSupported()) return () => {};
  const handler = () => cb(getBrowserVoices());
  window.speechSynthesis.addEventListener("voiceschanged", handler);
  return () => {
    window.speechSynthesis.removeEventListener("voiceschanged", handler);
  };
}

function clamp(n: number, min: number, max: number): number {
  if (Number.isNaN(n)) return min;
  return Math.max(min, Math.min(max, n));
}

function normalizeConfig(raw: Partial<BrowserTTSConfig>): BrowserTTSConfig {
  return {
    enabled: raw.enabled !== false,
    voiceURI: typeof raw.voiceURI === "string" ? raw.voiceURI : "",
    lang: typeof raw.lang === "string" ? raw.lang : "",
    rate: clamp(typeof raw.rate === "number" ? raw.rate : 1, 0.1, 10),
    pitch: clamp(typeof raw.pitch === "number" ? raw.pitch : 1, 0, 2),
    volume: clamp(typeof raw.volume === "number" ? raw.volume : 1, 0, 1),
  };
}

export function loadBrowserTTSConfig(): BrowserTTSConfig {
  if (typeof window === "undefined") return { ...DEFAULT_BROWSER_TTS_CONFIG };
  let parsed: Partial<BrowserTTSConfig> = {};
  try {
    const raw = window.localStorage.getItem(MOBILE_TTS_CONFIG_KEY);
    if (raw) parsed = JSON.parse(raw);
  } catch {
    // Unavailable or invalid local preferences use the defaults.
  }
  const cfg = normalizeConfig(parsed);

  if (!cfg.voiceURI) {
    try {
      const legacyName = window.localStorage.getItem(LEGACY_VOICE_NAME_KEY);
      if (legacyName) {
        const match = getBrowserVoices().find((v) => v.name === legacyName);
        if (match) cfg.voiceURI = match.voiceURI;
      }
    } catch {
      // The legacy preference is optional.
    }
  }
  return cfg;
}

export function saveBrowserTTSConfig(cfg: Partial<BrowserTTSConfig>): void {
  if (typeof window === "undefined") return;
  const merged = normalizeConfig({ ...loadBrowserTTSConfig(), ...cfg });
  window.localStorage.setItem(MOBILE_TTS_CONFIG_KEY, JSON.stringify(merged));
}

export interface BrowserTTSOptions {
  text: string;

  lang?: string;

  rate?: number;

  pitch?: number;

  volume?: number;

  voiceURI?: string;
}

export interface BrowserTTSHandle {
  cancel: () => void;
  promise: Promise<void>;
}

let activeUtterance: SpeechSynthesisUtterance | null = null;

export function speakWithBrowser(opts: BrowserTTSOptions): BrowserTTSHandle {
  if (!isBrowserTTSSupported()) {
    return {
      cancel: () => {},
      promise: Promise.reject(new Error("SpeechSynthesis not supported")),
    };
  }
  const synth = window.speechSynthesis;

  synth.cancel();

  const cfg = loadBrowserTTSConfig();
  const utterance = new SpeechSynthesisUtterance(opts.text);
  activeUtterance = utterance;

  const wantedVoiceURI = opts.voiceURI ?? cfg.voiceURI;
  if (wantedVoiceURI) {
    const match = getBrowserVoices().find((v) => v.voiceURI === wantedVoiceURI);
    if (match) utterance.voice = match;
  }

  const lang = opts.lang ?? cfg.lang;
  if (lang) utterance.lang = lang;
  utterance.rate = opts.rate ?? cfg.rate;
  utterance.pitch = opts.pitch ?? cfg.pitch;
  utterance.volume = opts.volume ?? cfg.volume;

  let resolved = false;
  let resolveFn!: () => void;
  let rejectFn!: (err: Error) => void;
  const promise = new Promise<void>((res, rej) => {
    resolveFn = res;
    rejectFn = rej;
  });

  utterance.onend = () => {
    if (resolved) return;
    resolved = true;
    if (activeUtterance === utterance) activeUtterance = null;
    resolveFn();
  };
  utterance.onerror = (ev: SpeechSynthesisErrorEvent) => {
    if (resolved) return;
    resolved = true;
    if (activeUtterance === utterance) activeUtterance = null;

    if (ev.error === "canceled" || ev.error === "interrupted") {
      resolveFn();
      return;
    }
    rejectFn(new Error(ev.error || "SpeechSynthesis error"));
  };

  synth.speak(utterance);

  return {
    cancel: () => {
      if (resolved) return;
      resolved = true;
      if (activeUtterance === utterance) {
        activeUtterance = null;
        synth.cancel();
      }
      resolveFn();
    },
    promise,
  };
}

export function cancelBrowserTTS(): void {
  if (!isBrowserTTSSupported()) return;
  activeUtterance = null;
  window.speechSynthesis.cancel();
}
