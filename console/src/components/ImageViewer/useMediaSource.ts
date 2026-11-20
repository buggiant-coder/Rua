import { useEffect, useState } from "react";
import { buildAuthHeaders } from "@/api/authHeaders";
import { isChatMediaUrl, mediaReadError } from "@/api/chatMedia";
import { withTransferSlot } from "@/api/transferAdmission";

type MediaRead = {
  controller: AbortController;
  url: Promise<string>;
  objectUrl?: string;
  users: number;
};

// Share a read while its previews are mounted. Source URLs include the Chat
// identity and the active credential, so another Chat or login gets a new read.
const reads = new Map<string, MediaRead>();

function acquireRead(src: string): MediaRead {
  const existing = reads.get(src);
  if (existing) {
    existing.users++;
    return existing;
  }
  const controller = new AbortController();
  const url = new URL(src, window.location.href);
  url.searchParams.delete("token");
  const read: MediaRead = {
    users: 1,
    controller,
    url: withTransferSlot(async () => {
      const response = await fetch(url.href, {
        headers: buildAuthHeaders(),
        signal: controller.signal,
        redirect: "error",
      });
      if (!response.ok) throw new Error(await mediaReadError(response));
      const blob = await response.blob();
      controller.signal.throwIfAborted();
      read.objectUrl = URL.createObjectURL(blob);
      return read.objectUrl;
    }, controller.signal).catch((error) => {
      if (reads.get(src) === read) reads.delete(src);
      throw error;
    }),
  };
  reads.set(src, read);
  return read;
}

function releaseRead(src: string, read: MediaRead) {
  if (--read.users) return;
  if (reads.get(src) === read) reads.delete(src);
  read.controller.abort();
  if (read.objectUrl) URL.revokeObjectURL(read.objectUrl);
}

export function useMediaSource(src: string, open: boolean) {
  const managed = isChatMediaUrl(src);
  const [requested, setRequested] = useState({ src: "", attempt: 0 });
  const [result, setResult] = useState<{
    src: string;
    url?: string;
    error?: string;
    signal: AbortSignal;
  }>();
  const retry = () =>
    setRequested((old) => ({ src, attempt: old.attempt + 1 }));

  useEffect(() => {
    if (managed && open && requested.src !== src)
      setRequested({ src, attempt: 0 });
    if (requested.src && requested.src !== src && !open)
      setRequested({ src: "", attempt: 0 });
  }, [managed, open, requested.src, src]);

  useEffect(() => {
    if (!managed || requested.src !== src) return;
    const read = acquireRead(src);
    const signal = read.controller.signal;
    let active = true;
    setResult(undefined);
    void read.url.then(
      (url) => {
        if (active) setResult({ src, url, signal });
      },
      (error) => {
        if (active)
          setResult({
            src,
            error: error instanceof Error ? error.message : String(error),
            signal,
          });
      },
    );
    return () => {
      active = false;
      releaseRead(src, read);
    };
  }, [managed, requested, src]);

  const current =
    result?.src === src && !result.signal.aborted ? result : undefined;
  return { url: managed ? current?.url : src, error: current?.error, retry };
}
