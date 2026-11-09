import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  cancelBrowserTTS,
  getBrowserVoices,
  loadBrowserTTSConfig,
  saveBrowserTTSConfig,
  MOBILE_TTS_CONFIG_KEY,
  speakWithBrowser,
  subscribeBrowserVoices,
} from "./browserTTS";

type UtteranceLike = {
  text: string;
  voice?: SpeechSynthesisVoice;
  lang: string;
  rate: number;
  pitch: number;
  volume: number;
  onend: (() => void) | null;
  onerror: ((event: { error?: string }) => void) | null;
};

const voices = [
  { name: "Legacy Voice", voiceURI: "legacy-uri", lang: "en-US" },
  { name: "Configured Voice", voiceURI: "configured-uri", lang: "zh-CN" },
] as SpeechSynthesisVoice[];

let lastUtterance: UtteranceLike | null = null;
const synth = {
  cancel: vi.fn(),
  speak: vi.fn((utterance: UtteranceLike) => {
    lastUtterance = utterance;
  }),
  getVoices: vi.fn(() => voices),
  addEventListener: vi.fn(),
  removeEventListener: vi.fn(),
};

class FakeSpeechSynthesisUtterance {
  text: string;
  voice?: SpeechSynthesisVoice;
  lang = "";
  rate = 1;
  pitch = 1;
  volume = 1;
  onend: (() => void) | null = null;
  onerror: ((event: { error?: string }) => void) | null = null;

  constructor(text: string) {
    this.text = text;
  }
}

describe("browserTTS", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    lastUtterance = null;
    localStorage.clear();
    Object.defineProperty(window, "speechSynthesis", {
      value: synth,
      configurable: true,
    });
    vi.stubGlobal("SpeechSynthesisUtterance", FakeSpeechSynthesisUtterance);
  });

  it("loads normalized config and falls back to a legacy voice name", () => {
    localStorage.setItem(
      MOBILE_TTS_CONFIG_KEY,
      JSON.stringify({
        rate: 99,
        pitch: -1,
        volume: 2,
      }),
    );
    localStorage.setItem("tts_browser_voice", "Legacy Voice");

    expect(loadBrowserTTSConfig()).toMatchObject({
      enabled: true,
      voiceURI: "legacy-uri",
      rate: 10,
      pitch: 0,
      volume: 1,
    });
  });

  it("saves config by merging with existing normalized values", () => {
    localStorage.setItem(
      MOBILE_TTS_CONFIG_KEY,
      JSON.stringify({
        enabled: false,
        voiceURI: "configured-uri",
        lang: "zh-CN",
        rate: 1.25,
      }),
    );

    saveBrowserTTSConfig({ volume: 0.4 });

    expect(JSON.parse(localStorage.getItem(MOBILE_TTS_CONFIG_KEY)!)).toEqual({
      enabled: false,
      voiceURI: "configured-uri",
      lang: "zh-CN",
      rate: 1.25,
      pitch: 1,
      volume: 0.4,
    });
  });

  it("speaks with the configured voice and resolves on end", async () => {
    localStorage.setItem(
      MOBILE_TTS_CONFIG_KEY,
      JSON.stringify({
        voiceURI: "configured-uri",
        lang: "zh-CN",
        rate: 1.1,
        pitch: 1.2,
        volume: 0.7,
      }),
    );

    const handle = speakWithBrowser({ text: "你好" });

    expect(synth.cancel).toHaveBeenCalledTimes(1);
    expect(synth.speak).toHaveBeenCalledTimes(1);
    expect(lastUtterance).toMatchObject({
      text: "你好",
      voice: voices[1],
      lang: "zh-CN",
      rate: 1.1,
      pitch: 1.2,
      volume: 0.7,
    });

    lastUtterance?.onend?.();
    await expect(handle.promise).resolves.toBeUndefined();
  });

  it("an older handle cannot cancel the newer utterance", async () => {
    const previous = speakWithBrowser({ text: "previous" });
    const current = speakWithBrowser({ text: "current" });
    synth.cancel.mockClear();
    previous.cancel();
    expect(synth.cancel).not.toHaveBeenCalled();
    await expect(previous.promise).resolves.toBeUndefined();
    current.cancel();
    expect(synth.cancel).toHaveBeenCalledTimes(1);
    await expect(current.promise).resolves.toBeUndefined();
  });

  it("treats browser cancellation as a clean stop", async () => {
    const handle = speakWithBrowser({ text: "stop me" });

    lastUtterance?.onerror?.({ error: "canceled" });

    await expect(handle.promise).resolves.toBeUndefined();
  });

  it("subscribes and unsubscribes voice-change listeners", () => {
    const callback = vi.fn();
    const unsubscribe = subscribeBrowserVoices(callback);

    expect(synth.addEventListener).toHaveBeenCalledWith(
      "voiceschanged",
      expect.any(Function),
    );

    unsubscribe();

    expect(synth.removeEventListener).toHaveBeenCalledWith(
      "voiceschanged",
      expect.any(Function),
    );
    expect(getBrowserVoices()).toBe(voices);
    cancelBrowserTTS();
    expect(synth.cancel).toHaveBeenCalled();
  });
});
