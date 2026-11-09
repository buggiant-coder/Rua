import { useState, useCallback, useRef, useEffect } from "react";
import { useTTSStore } from "../stores/ttsStore";
import { getApiUrl } from "../api";
import { buildAuthHeaders } from "../api/authHeaders";
import {
  speakWithBrowser,
  type BrowserTTSHandle,
  loadBrowserTTSConfig,
  isBrowserTTSSupported,
} from "../utils/browserTTS";
import { isMobile } from "../utils/isMobile";

const CHUNK_MAX_CHARS = 300;

function splitTextChunks(text: string): string[] {
  const sentences = text.split(/(?<=[。！？.!?\n])\s*/);
  const chunks: string[] = [];
  let buf = "";
  for (const s of sentences) {
    const trimmed = s.trim();
    if (!trimmed) continue;
    if (buf && buf.length + trimmed.length > CHUNK_MAX_CHARS) {
      chunks.push(buf);
      buf = trimmed;
    } else {
      buf = buf ? buf + " " + trimmed : trimmed;
    }
  }
  if (buf) chunks.push(buf);

  if (chunks.length === 0 && text.trim()) {
    const t = text.trim();
    for (let i = 0; i < t.length; i += CHUNK_MAX_CHARS) {
      chunks.push(t.slice(i, i + CHUNK_MAX_CHARS));
    }
  }
  return chunks;
}

interface UseTTSReturn {
  speak: (text: string) => void;
  stop: () => void;
  isSpeaking: boolean;
  isLoading: boolean;
  isDisabled: boolean;
  error: string | null;
}

export function useTTS(): UseTTSReturn {
  const providerType = useTTSStore((s) => s.providerType);
  const [isSpeaking, setIsSpeaking] = useState(false);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const blobUrlRef = useRef<string | null>(null);
  const abortRef = useRef<AbortController | null>(null);
  const browserHandleRef = useRef<BrowserTTSHandle | null>(null);

  const cleanup = useCallback(() => {
    if (abortRef.current) {
      abortRef.current.abort();
      abortRef.current = null;
    }

    browserHandleRef.current?.cancel();
    browserHandleRef.current = null;

    if (audioRef.current) {
      audioRef.current.pause();
      audioRef.current.currentTime = 0;
      audioRef.current = null;
    }
    if (blobUrlRef.current) {
      URL.revokeObjectURL(blobUrlRef.current);
      blobUrlRef.current = null;
    }
  }, []);

  useEffect(() => cleanup, [cleanup]);

  const stop = useCallback(() => {
    cleanup();
    setIsSpeaking(false);
    setIsLoading(false);
  }, [cleanup]);

  const speakBrowser = useCallback((text: string) => {
    if (!isBrowserTTSSupported()) return;
    setIsSpeaking(true);

    const handle = speakWithBrowser({ text });
    browserHandleRef.current = handle;
    handle.promise
      .catch(() => {})
      .finally(() => {
        if (browserHandleRef.current !== handle) return;
        browserHandleRef.current = null;
        setIsSpeaking(false);
      });
  }, []);

  const fetchChunk = async (
    text: string,
    signal: AbortSignal,
  ): Promise<string> => {
    const url = getApiUrl("/agent/tts/synthesize");
    const res = await fetch(url, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        ...buildAuthHeaders(),
      },
      body: JSON.stringify({ text }),
      signal,
    });
    if (!res.ok) {
      let detail = `TTS failed (${res.status})`;
      try {
        const errJson = await res.json();
        detail = errJson.detail || errJson.message || detail;
      } catch {
        // Keep the HTTP status when a TTS error response has no JSON body.
      }
      throw new Error(detail);
    }
    const blob = await res.blob();
    if (blob.size === 0) throw new Error("TTS returned empty audio");
    return URL.createObjectURL(blob);
  };

  const playBlobUrl = (blobUrl: string, signal: AbortSignal): Promise<void> =>
    new Promise((resolve, reject) => {
      if (signal.aborted) {
        URL.revokeObjectURL(blobUrl);
        reject(new DOMException("Aborted", "AbortError"));
        return;
      }

      if (blobUrlRef.current) URL.revokeObjectURL(blobUrlRef.current);
      blobUrlRef.current = blobUrl;

      const audio = new Audio(blobUrl);
      audioRef.current = audio;

      const onAbort = () => {
        audio.pause();
        reject(new DOMException("Aborted", "AbortError"));
      };
      signal.addEventListener("abort", onAbort, { once: true });

      audio.onended = () => {
        signal.removeEventListener("abort", onAbort);
        audioRef.current = null;
        resolve();
      };
      audio.onerror = () => {
        signal.removeEventListener("abort", onAbort);
        audioRef.current = null;
        reject(new Error("Audio playback failed"));
      };
      audio.play().catch(reject);
    });

  const speakServer = useCallback(async (text: string) => {
    if (abortRef.current) abortRef.current.abort();
    const ac = new AbortController();
    abortRef.current = ac;

    const chunks = splitTextChunks(text);
    setIsLoading(true);
    setIsSpeaking(true);
    setError(null);

    try {
      for (let i = 0; i < chunks.length; i++) {
        if (ac.signal.aborted) break;

        const fetchCurrent = fetchChunk(chunks[i], ac.signal);
        const fetchNext =
          i + 1 < chunks.length ? fetchChunk(chunks[i + 1], ac.signal) : null;

        const blobUrl = await fetchCurrent;

        if (i === 0) setIsLoading(false);

        await playBlobUrl(blobUrl, ac.signal);

        if (fetchNext) {
          const nextBlobUrl = await fetchNext;
          i++;
          if (ac.signal.aborted) {
            URL.revokeObjectURL(nextBlobUrl);
            break;
          }
          await playBlobUrl(nextBlobUrl, ac.signal);
        }
      }
    } catch (err) {
      if (err instanceof DOMException && err.name === "AbortError") {
        return;
      }
      console.error("TTS error:", err);
      setError(err instanceof Error ? err.message : "TTS failed");
    } finally {
      setIsLoading(false);
      setIsSpeaking(false);
    }
  }, []);

  const inMobile = isMobile();
  const mobileBrowserEnabled =
    inMobile && isBrowserTTSSupported()
      ? loadBrowserTTSConfig().enabled
      : false;

  const effectiveProvider = inMobile
    ? mobileBrowserEnabled
      ? "browser"
      : "disabled"
    : providerType;

  const speak = useCallback(
    (text: string) => {
      if (!text || effectiveProvider === "disabled") return;

      stop();
      setError(null);
      if (effectiveProvider === "openai_tts") {
        void speakServer(text);
      } else {
        speakBrowser(text);
      }
    },
    [effectiveProvider, stop, speakServer, speakBrowser],
  );

  return {
    speak,
    stop,
    isSpeaking,
    isLoading,
    isDisabled: effectiveProvider === "disabled",
    error,
  };
}
