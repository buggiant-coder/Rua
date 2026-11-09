import { test, expect } from "./fixtures";
import type { APIRequestContext } from "@playwright/test";

type AudioMode = { audio_mode: string };
type TranscriptionProviderType = { transcription_provider_type: string };
type TTSConfig = {
  tts_provider_type: string;
  tts_provider_id: string;
  tts_model: string;
  tts_voice: string;
  tts_speed: number;
};

function apiBase(baseURL?: string): string {
  return baseURL ?? "http://127.0.0.1:8090";
}

async function getJson<T>(
  request: APIRequestContext,
  baseURL: string | undefined,
  path: string,
): Promise<T> {
  const response = await request.get(`${apiBase(baseURL)}${path}`);
  expect(response.ok()).toBeTruthy();
  return (await response.json()) as T;
}

async function putJson(
  request: APIRequestContext,
  baseURL: string | undefined,
  path: string,
  data: Record<string, unknown>,
): Promise<void> {
  const response = await request.put(`${apiBase(baseURL)}${path}`, { data });
  expect(response.ok()).toBeTruthy();
}

test.describe("voice transcription", () => {
  test("voice page reflects and persists Rust shared ASR/TTS settings", async ({
    page,
    request,
    baseURL,
  }) => {
    const [originalAudioMode, originalProviderType, originalTTSConfig] =
      await Promise.all([
        getJson<AudioMode>(request, baseURL, "/api/agent/audio-mode"),
        getJson<TranscriptionProviderType>(
          request,
          baseURL,
          "/api/agent/transcription-provider-type",
        ),
        getJson<TTSConfig>(request, baseURL, "/api/agent/tts-config"),
      ]);

    try {
      await putJson(request, baseURL, "/api/agent/audio-mode", {
        audio_mode: "auto",
      });
      await putJson(
        request,
        baseURL,
        "/api/agent/transcription-provider-type",
        {
          transcription_provider_type: "disabled",
        },
      );
      await putJson(request, baseURL, "/api/agent/tts-config", {
        tts_provider_type: "browser",
        tts_provider_id: "",
        tts_model: "tts-1",
        tts_voice: "alloy",
        tts_speed: 1,
      });

      for (const path of [
        "/api/agent/transcription-providers",
        "/api/agent/local-whisper-status",
        "/api/agent/tts-providers",
      ]) {
        const response = await request.get(`${apiBase(baseURL)}${path}`);
        expect(response.ok()).toBeTruthy();
      }

      await page.goto("/voice-transcription", {
        waitUntil: "domcontentloaded",
      });
      await expect(
        page.getByText(/Voice Transcription|语音设置/).first(),
      ).toBeVisible();
      await expect(
        page.getByText(/Speech to Text|语音转文字/).first(),
      ).toBeVisible();
      await expect(
        page.getByText(/Text to Speech|文字转语音/).first(),
      ).toBeVisible();
      await expect(page.getByText(/Audio mode|音频模式/).first()).toBeVisible();
      await expect(
        page.getByText(/Provider type|提供商类型/).first(),
      ).toBeVisible();
      await expect(page.getByText(/TTS engine|TTS 引擎/).first()).toBeVisible();
      await expect(page.getByText(/Auto audio|自动音频/).first()).toBeVisible();
      await expect(
        page.getByText(/Disabled STT|禁用 STT/).first(),
      ).toBeVisible();
      await expect(
        page.getByText(/Browser TTS|浏览器 TTS/).first(),
      ).toBeVisible();

      await page
        .getByText(/Local Whisper|本地 Whisper/)
        .first()
        .click();
      await page
        .getByText(/Text-to-speech is turned off|关闭文字转语音/)
        .first()
        .click();
      await page.getByRole("button", { name: /Save|保存/ }).click();
      await expect
        .poll(
          async () => {
            const [providerType, ttsConfig] = await Promise.all([
              getJson<TranscriptionProviderType>(
                request,
                baseURL,
                "/api/agent/transcription-provider-type",
              ),
              getJson<TTSConfig>(request, baseURL, "/api/agent/tts-config"),
            ]);
            return {
              transcription_provider_type:
                providerType.transcription_provider_type,
              tts_provider_type: ttsConfig.tts_provider_type,
            };
          },
          { timeout: 10_000 },
        )
        .toEqual({
          transcription_provider_type: "local_whisper",
          tts_provider_type: "disabled",
        });

      await page
        .getByText(/Native audio|原生音频/)
        .first()
        .click();
      await page.getByRole("button", { name: /Save|保存/ }).click();
      await expect
        .poll(
          async () =>
            (
              await getJson<AudioMode>(
                request,
                baseURL,
                "/api/agent/audio-mode",
              )
            ).audio_mode,
          { timeout: 10_000 },
        )
        .toBe("native");

      await page.reload({ waitUntil: "domcontentloaded" });
      await expect(
        page.getByText(/Native audio|原生音频/).first(),
      ).toBeVisible();
    } finally {
      await putJson(request, baseURL, "/api/agent/audio-mode", {
        audio_mode: originalAudioMode.audio_mode,
      });
      await putJson(
        request,
        baseURL,
        "/api/agent/transcription-provider-type",
        {
          transcription_provider_type:
            originalProviderType.transcription_provider_type,
        },
      );
      await putJson(
        request,
        baseURL,
        "/api/agent/tts-config",
        originalTTSConfig,
      );
    }
  });
});
