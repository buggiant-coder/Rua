import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../request", () => ({ request: vi.fn() }));

import { request } from "../request";
import { agentApi } from "./agent";

describe("agentApi core routes", () => {
  afterEach(() => vi.clearAllMocks());

  it("uses Rust /agent health, process, status, and shutdown routes", async () => {
    const body = { input: "hello", session_id: "s1" } as Parameters<
      typeof agentApi.agentApi
    >[0];

    await agentApi.agentRoot();
    await agentApi.healthCheck();
    await agentApi.agentApi(body);
    await agentApi.getProcessStatus();
    await agentApi.shutdownSimple();
    await agentApi.shutdown();

    expect(request).toHaveBeenNthCalledWith(1, "/agent/");
    expect(request).toHaveBeenNthCalledWith(2, "/agent/health");
    expect(request).toHaveBeenNthCalledWith(3, "/agent/process", {
      method: "POST",
      body: JSON.stringify(body),
    });
    expect(request).toHaveBeenNthCalledWith(4, "/agent/admin/status");
    expect(request).toHaveBeenNthCalledWith(5, "/agent/shutdown", {
      method: "POST",
    });
    expect(request).toHaveBeenNthCalledWith(6, "/agent/admin/shutdown", {
      method: "POST",
    });
  });

  it("round-trips running config and language through Rust agent routes", async () => {
    const config = {
      context_compact: { max_tokens: 4096 },
      unknown_future_field: true,
    } as Parameters<typeof agentApi.updateAgentRunningConfig>[0];

    await agentApi.getAgentRunningConfig();
    await agentApi.updateAgentRunningConfig(config);
    await agentApi.getAgentLanguage();
    await agentApi.updateAgentLanguage("zh-CN");

    expect(request).toHaveBeenNthCalledWith(1, "/agent/running-config");
    expect(request).toHaveBeenNthCalledWith(2, "/agent/running-config", {
      method: "PUT",
      body: JSON.stringify(config),
    });
    expect(request).toHaveBeenNthCalledWith(3, "/agent/language");
    expect(request).toHaveBeenNthCalledWith(4, "/agent/language", {
      method: "PUT",
      body: JSON.stringify({ language: "zh-CN" }),
    });
  });
});

describe("agentApi voice and Claude Code routes", () => {
  afterEach(() => vi.clearAllMocks());

  it("uses Rust shared voice/transcription settings routes", async () => {
    await agentApi.getAudioMode();
    await agentApi.updateAudioMode("push_to_talk");
    await agentApi.getTranscriptionProviders();
    await agentApi.updateTranscriptionProvider("openai");
    await agentApi.getTranscriptionProviderType();
    await agentApi.updateTranscriptionProviderType("cloud");
    await agentApi.getLocalWhisperStatus();

    expect(request).toHaveBeenNthCalledWith(1, "/agent/audio-mode");
    expect(request).toHaveBeenNthCalledWith(2, "/agent/audio-mode", {
      method: "PUT",
      body: JSON.stringify({ audio_mode: "push_to_talk" }),
    });
    expect(request).toHaveBeenNthCalledWith(
      3,
      "/agent/transcription-providers",
    );
    expect(request).toHaveBeenNthCalledWith(
      4,
      "/agent/transcription-provider",
      {
        method: "PUT",
        body: JSON.stringify({ provider_id: "openai" }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      5,
      "/agent/transcription-provider-type",
    );
    expect(request).toHaveBeenNthCalledWith(
      6,
      "/agent/transcription-provider-type",
      {
        method: "PUT",
        body: JSON.stringify({ transcription_provider_type: "cloud" }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(7, "/agent/local-whisper-status");
  });

  it("uses Rust TTS pass-through routes", async () => {
    const ttsConfig = {
      tts_provider_type: "cloud",
      tts_provider_id: "dashscope",
      tts_model: "cosyvoice",
      tts_voice: "longxiaochun",
      tts_speed: 1.1,
    };

    await agentApi.getTTSConfig();
    await agentApi.updateTTSConfig(ttsConfig);
    await agentApi.getTTSProviders();

    expect(request).toHaveBeenNthCalledWith(1, "/agent/tts-config");
    expect(request).toHaveBeenNthCalledWith(2, "/agent/tts-config", {
      method: "PUT",
      body: JSON.stringify(ttsConfig),
    });
    expect(request).toHaveBeenNthCalledWith(3, "/agent/tts-providers");
  });
});
