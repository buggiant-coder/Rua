import { getApiUrl } from "./config";
import { chatApi } from "./modules/chat";

const mediaIdPattern = /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i;

function parseUrl(source: string) {
  try {
    return new URL(source, window.location.href);
  } catch {
    return undefined;
  }
}

export function chatMediaId(reference?: string): string | undefined {
  const id = reference?.startsWith("media/") ? reference.slice(6) : "";
  return mediaIdPattern.test(id) ? id : undefined;
}

export function isChatMediaReference(source: string): boolean {
  return (
    source.startsWith("media/") ||
    /\/api\/(?:files\/preview|chats\/[^/]+\/media|(?:agents|projects)\/[^/]+\/(?:chats\/[^/]+\/media|files\/preview))(?:[/?#]|$)/i.test(
      parseUrl(source)?.pathname ?? "",
    )
  );
}

export function isChatMediaUrl(source: string): boolean {
  const api = new URL(getApiUrl("/chats/"), window.location.href);
  const url = parseUrl(source);
  if (
    !url ||
    url.origin !== api.origin ||
    !url.pathname.startsWith(api.pathname)
  )
    return false;
  const parts = url.pathname.slice(api.pathname.length).split("/");
  return (
    parts.length === 3 &&
    Boolean(parts[0]) &&
    parts[1] === "media" &&
    mediaIdPattern.test(parts[2])
  );
}

export function chatMediaDisplayUrl(
  source: string | undefined,
  chatId?: string,
): string {
  if (!source) return "";
  const id = chatMediaId(source);
  if (id) return chatId ? chatApi.mediaUrl(chatId, id) : "";
  if (/^(blob:|data:(image|audio|video)\/)/i.test(source)) return source;
  // Canonical root-relative API links also work when Console uses a separate API host.
  if (source.startsWith("/api/")) source = getApiUrl(source.slice(4));
  const url = parseUrl(source);
  if (!url) return "";
  const api = new URL(getApiUrl("/"), window.location.href);
  if (url.origin === api.origin && url.pathname.startsWith(api.pathname)) {
    const id = url.pathname.split("/").pop();
    if (!chatId || !id || !isChatMediaUrl(source)) return "";
    const canonical = chatApi.mediaUrl(chatId, id);
    return new URL(canonical, window.location.href).pathname === url.pathname
      ? canonical
      : "";
  }
  if (isChatMediaReference(source)) return "";
  return /^https?:\/\//i.test(source) ? source : "";
}

export async function mediaReadError(response: Response): Promise<string> {
  const detail = (await response.json().catch(() => null))?.detail;
  return typeof detail === "string"
    ? detail
    : `${response.status} ${response.statusText}`.trim();
}
