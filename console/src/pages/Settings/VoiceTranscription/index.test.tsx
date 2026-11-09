import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import VoiceSettingsPage from "./index";

const { apiMock, toastMock } = vi.hoisted(() => ({
  apiMock: {
    getAudioMode: vi.fn(),
    updateAudioMode: vi.fn(),
    getTranscriptionProviderType: vi.fn(),
    updateTranscriptionProviderType: vi.fn(),
    getTranscriptionProviders: vi.fn(),
    updateTranscriptionProvider: vi.fn(),
    getLocalWhisperStatus: vi.fn(),
    getTTSConfig: vi.fn(),
    updateTTSConfig: vi.fn(),
    getTTSProviders: vi.fn(),
  },
  toastMock: {
    error: vi.fn(),
    success: vi.fn(),
  },
}));

vi.mock("../../../api", () => ({ default: apiMock }));

vi.mock("@/utils/isMobile", () => ({ isMobile: () => false }));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "common.disabled": "Disabled",
        "common.enabled": "Enabled",
        "common.reset": "Reset",
        "common.save": "Save",
        "tts.browserVoiceDesc": "Use browser voices",
        "tts.browserVoiceLabel": "Browser voice",
        "tts.browserVoicePlaceholder": "Select a browser voice",
        "tts.engineBrowser": "Browser TTS",
        "tts.engineBrowserDesc": "Use local browser speech synthesis",
        "tts.engineDescription": "Choose how assistant answers are spoken",
        "tts.engineDisabled": "Disabled TTS",
        "tts.engineDisabledDesc": "Do not speak assistant answers",
        "tts.engineLabel": "TTS engine",
        "tts.engineOpenAI": "OpenAI-compatible TTS",
        "tts.engineOpenAIDesc": "Use a configured provider",
        "tts.modelDesc": "Provider model",
        "tts.modelLabel": "TTS model",
        "tts.noProvidersWarning": "No TTS providers",
        "tts.providerDesc": "Provider used for TTS",
        "tts.providerLabel": "TTS provider",
        "tts.providerPlaceholder": "Select a TTS provider",
        "tts.speedDesc": "Playback speed",
        "tts.speedLabel": "Speed",
        "tts.voiceDesc": "Provider voice",
        "tts.voiceLabel": "TTS voice",
        "voiceSettings.sttSectionTitle": "Speech to Text",
        "voiceSettings.title": "Voice Transcription",
        "voiceSettings.ttsSectionTitle": "Text to Speech",
        "voiceTranscription.audioModeDescription": "Choose audio path",
        "voiceTranscription.audioModeLabel": "Audio mode",
        "voiceTranscription.ffmpegMissing": "FFmpeg missing",
        "voiceTranscription.ffmpegMissingDesc":
          "Install FFmpeg for native audio",
        "voiceTranscription.ffmpegReady": "FFmpeg ready",
        "voiceTranscription.loadFailed": "Failed to load voice settings",
        "voiceTranscription.localWhisperMissing": "Local Whisper missing",
        "voiceTranscription.localWhisperMissingDesc":
          "FFmpeg: {{ffmpeg}}, Whisper: {{whisper}}",
        "voiceTranscription.localWhisperReady": "Local Whisper ready",
        "voiceTranscription.modeAuto": "Auto audio",
        "voiceTranscription.modeAutoDesc": "Let Rust choose the audio path",
        "voiceTranscription.modeNative": "Native audio",
        "voiceTranscription.modeNativeDesc": "Use native local audio",
        "voiceTranscription.noProvidersWarning": "No STT providers",
        "voiceTranscription.providerDescription": "Provider used for STT",
        "voiceTranscription.providerLabel": "STT provider",
        "voiceTranscription.providerPlaceholder": "Select an STT provider",
        "voiceTranscription.providerTypeDescription":
          "Choose transcription provider type",
        "voiceTranscription.providerTypeDisabled": "Disabled STT",
        "voiceTranscription.providerTypeDisabledDesc": "Do not transcribe",
        "voiceTranscription.providerTypeLabel": "Provider type",
        "voiceTranscription.providerTypeLocalWhisper": "Local Whisper",
        "voiceTranscription.providerTypeLocalWhisperDesc": "Use local Whisper",
        "voiceTranscription.providerTypeWhisperApi": "Whisper API",
        "voiceTranscription.providerTypeWhisperApiDesc":
          "Use a cloud Whisper provider",
        "voiceTranscription.saveFailed": "Failed to save voice settings",
        "voiceTranscription.saveSuccess": "Voice settings saved",
        "voiceTranscription.transcriptionInfoDesc":
          "Transcription uses the shared Rust voice config",
        "voiceTranscription.transcriptionInfoDescLocal":
          "Local transcription uses local runtime dependencies",
        "voiceTranscription.transcriptionInfoTitle": "Transcription",
      })[key] ?? key,
  }),
}));

vi.mock("@/ui", async () => {
  const actual = await vi.importActual<typeof import("@/ui")>("@/ui");
  return {
    ...actual,
    toast: toastMock,
  };
});

function resetApiMocks() {
  apiMock.getAudioMode.mockResolvedValue({ audio_mode: "auto" });
  apiMock.updateAudioMode.mockResolvedValue({ audio_mode: "auto" });
  apiMock.getTranscriptionProviderType.mockResolvedValue({
    transcription_provider_type: "disabled",
  });
  apiMock.updateTranscriptionProviderType.mockResolvedValue({
    transcription_provider_type: "disabled",
  });
  apiMock.getTranscriptionProviders.mockResolvedValue({
    configured_provider_id: "dashscope",
    providers: [
      {
        id: "dashscope",
        name: "DashScope",
        available: true,
        type: "dashscope",
      },
      { id: "offline", name: "Offline", available: false, type: "openai" },
    ],
  });
  apiMock.updateTranscriptionProvider.mockResolvedValue({
    provider_id: "dashscope",
  });
  apiMock.getLocalWhisperStatus.mockResolvedValue({
    available: false,
    ffmpeg_installed: false,
    whisper_installed: false,
  });
  apiMock.getTTSConfig.mockResolvedValue({
    tts_provider_type: "browser",
    tts_provider_id: "",
    tts_model: "tts-1",
    tts_voice: "alloy",
    tts_speed: 1,
  });
  apiMock.updateTTSConfig.mockResolvedValue({
    tts_provider_type: "browser",
    tts_provider_id: "",
    tts_model: "tts-1",
    tts_voice: "alloy",
    tts_speed: 1,
  });
  apiMock.getTTSProviders.mockResolvedValue({
    configured_provider_id: "",
    providers: [
      {
        id: "dashscope",
        name: "DashScope",
        available: true,
        type: "dashscope",
      },
    ],
  });
}

describe("VoiceSettingsPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    resetApiMocks();
  });

  it("loads Rust shared ASR/TTS settings and renders the desktop controls", async () => {
    render(<VoiceSettingsPage />);

    await screen.findByText("Speech to Text");
    expect(screen.getByText("Text to Speech")).toBeInTheDocument();
    expect(screen.getByText("Auto audio")).toBeInTheDocument();
    expect(screen.getByText("Disabled STT")).toBeInTheDocument();
    expect(screen.getByText("Browser TTS")).toBeInTheDocument();
    expect(apiMock.getAudioMode).toHaveBeenCalledTimes(1);
    expect(apiMock.getTTSConfig).toHaveBeenCalledTimes(1);
  });

  it("switches to native audio without showing cloud provider controls", async () => {
    render(<VoiceSettingsPage />);

    fireEvent.click(await screen.findByText("Native audio"));

    expect(screen.getByText("FFmpeg missing")).toBeInTheDocument();
    expect(screen.queryByText("Whisper API")).not.toBeInTheDocument();
  });

  it("saves Whisper API provider choice through Rust voice config routes", async () => {
    render(<VoiceSettingsPage />);

    fireEvent.click(await screen.findByText("Whisper API"));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(apiMock.updateAudioMode).toHaveBeenCalledWith("auto");
      expect(apiMock.updateTranscriptionProviderType).toHaveBeenCalledWith(
        "whisper_api",
      );
      expect(apiMock.updateTranscriptionProvider).toHaveBeenCalledWith(
        "dashscope",
      );
      expect(apiMock.updateTTSConfig).toHaveBeenCalledWith({
        tts_provider_type: "browser",
        tts_provider_id: "",
        tts_model: "tts-1",
        tts_voice: "alloy",
        tts_speed: 1,
      });
    });
    expect(toastMock.success).toHaveBeenCalledWith("Voice settings saved");
  });

  it("persists browser voice preference only for browser TTS", async () => {
    const voice = { name: "Alex", lang: "en-US" } as SpeechSynthesisVoice;
    Object.defineProperty(window, "speechSynthesis", {
      configurable: true,
      value: {
        getVoices: () => [voice],
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
      },
    });

    render(<VoiceSettingsPage />);

    fireEvent.focus(
      await screen.findByPlaceholderText("Select a browser voice"),
    );
    fireEvent.click(screen.getByText("Alex (en-US)"));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(localStorage.getItem("tts_browser_voice")).toBe("Alex");
    });
  });

  it("surfaces load and save failures through the page toast boundary", async () => {
    apiMock.getAudioMode.mockRejectedValueOnce(new Error("voice down"));
    const firstRender = render(<VoiceSettingsPage />);

    await waitFor(() =>
      expect(toastMock.error).toHaveBeenCalledWith(
        "Failed to load voice settings",
      ),
    );
    firstRender.unmount();

    resetApiMocks();
    apiMock.updateTTSConfig.mockRejectedValueOnce(new Error("save down"));
    render(<VoiceSettingsPage />);

    await screen.findByText("Speech to Text");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() =>
      expect(toastMock.error).toHaveBeenCalledWith(
        "Failed to save voice settings",
      ),
    );
  });
});
