import { withTransferSlot } from "@/api/transferAdmission";
import { useEffect, useState } from "react";
import { buildAuthHeaders } from "@/api/authHeaders";
import { getApiUrl } from "@/api/config";

function apiImageUrl(src: string): string | undefined {
  if (!src) return;
  const url = new URL(src, window.location.href);
  if (url.protocol !== "http:" && url.protocol !== "https:") return;
  const api = new URL(getApiUrl("/"), window.location.href);
  if (url.origin !== api.origin || !url.pathname.startsWith(api.pathname)) {
    return;
  }
  url.searchParams.delete("token");
  return url.href;
}

export function useImageSource(src: string): string | undefined {
  const apiUrl = apiImageUrl(src);
  const [loaded, setLoaded] = useState<{
    src: string;
    url: string;
    signal: AbortSignal;
  }>();

  useEffect(() => {
    if (!apiUrl) return;
    const controller = new AbortController();
    let objectUrl: string | undefined;
    void (async () => {
      try {
        const blob = await withTransferSlot(async () => {
          const response = await fetch(apiUrl, {
            headers: buildAuthHeaders(),
            signal: controller.signal,
            redirect: "error",
          });
          if (!response.ok) {
            await response.body?.cancel();
            return undefined;
          }
          return response.blob();
        }, controller.signal);
        if (!blob) return;
        if (controller.signal.aborted) return;
        objectUrl = URL.createObjectURL(blob);
        setLoaded({ src, url: objectUrl, signal: controller.signal });
      } catch {
        // Failed or cancelled reads must not fall back to unauthenticated URLs.
      }
    })();
    return () => {
      controller.abort();
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [apiUrl, src]);

  return apiUrl
    ? loaded?.src === src && !loaded.signal.aborted
      ? loaded.url
      : undefined
    : src;
}
