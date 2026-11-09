import { beforeEach, describe, expect, it, vi } from "vitest";
import { useTTSStore } from "./ttsStore";

const apiMock = vi.hoisted(() => ({
  getTTSConfig: vi.fn(),
}));

vi.mock("../api", () => ({
  default: apiMock,
}));

function resetStore() {
  useTTSStore.setState({
    providerType: "browser",
    providerId: "",
    model: "tts-1",
    voice: "alloy",
    speed: 1.0,
    loaded: false,
  });
}

describe("ttsStore", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    resetStore();
  });

  it("loads Rust shared TTS config into the browser store shape", async () => {
    apiMock.getTTSConfig.mockResolvedValue({
      tts_provider_type: "openai_tts",
      tts_provider_id: "dashscope",
      tts_model: "cosyvoice-v1",
      tts_voice: "longxiaochun",
      tts_speed: 1.25,
    });

    await useTTSStore.getState().fetchConfig();

    expect(apiMock.getTTSConfig).toHaveBeenCalledTimes(1);
    expect(useTTSStore.getState()).toMatchObject({
      providerType: "openai_tts",
      providerId: "dashscope",
      model: "cosyvoice-v1",
      voice: "longxiaochun",
      speed: 1.25,
      loaded: true,
    });
  });

  it("fills defaults for absent backend TTS fields", async () => {
    apiMock.getTTSConfig.mockResolvedValue({});

    await useTTSStore.getState().fetchConfig();

    expect(useTTSStore.getState()).toMatchObject({
      providerType: "browser",
      providerId: "",
      model: "tts-1",
      voice: "alloy",
      speed: 1.0,
      loaded: true,
    });
  });

  it("marks config as loaded when the backend read fails", async () => {
    apiMock.getTTSConfig.mockRejectedValue(new Error("offline"));

    await useTTSStore.getState().fetchConfig();

    expect(useTTSStore.getState()).toMatchObject({
      providerType: "browser",
      providerId: "",
      model: "tts-1",
      voice: "alloy",
      speed: 1.0,
      loaded: true,
    });
  });
});
