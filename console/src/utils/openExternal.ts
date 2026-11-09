import { isDesktopShell, openDesktopExternal } from "../desktop/runtime";

export function normalizeExternalUrl(rawUrl: string): string | null {
  if (!rawUrl || rawUrl.trim() !== rawUrl) return null;
  if (rawUrl === "#" || rawUrl.startsWith("#")) return null;
  if (
    rawUrl.split("").some((character) => {
      const code = character.charCodeAt(0);
      return code <= 0x1f || code === 0x7f;
    })
  ) {
    return null;
  }
  if (/^https?:[^/]/i.test(rawUrl)) return null;

  try {
    const parsed = new URL(rawUrl, window.location.origin);
    if (!["http:", "https:", "mailto:", "tel:"].includes(parsed.protocol)) {
      return null;
    }
    if (
      (parsed.protocol === "http:" || parsed.protocol === "https:") &&
      !parsed.hostname
    ) {
      return null;
    }
    return parsed.href;
  } catch {
    return null;
  }
}

export async function openExternal(url: string): Promise<boolean> {
  const normalized = normalizeExternalUrl(url);
  if (!normalized) return false;
  if (isDesktopShell()) {
    try {
      return await openDesktopExternal(normalized);
    } catch {
      return false;
    }
  }
  try {
    const a = document.createElement("a");
    a.href = normalized;
    a.target = "_blank";

    a.rel = "noopener noreferrer external";
    a.style.display = "none";
    document.body.appendChild(a);
    a.click();
    a.remove();
  } catch {}
  try {
    window.open(normalized, "_blank", "noopener,noreferrer");
  } catch {}
  return true;
}
