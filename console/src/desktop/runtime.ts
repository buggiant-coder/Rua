import { getApiUrl } from "@/api/config";
import { invoke, isTauri } from "@tauri-apps/api/core";

const DESKTOP_QUERY_KEY = "desktop";
const DESKTOP_SESSION_KEY = "ruaDesktopShell";

export interface DesktopSaveResult {
  status: "saved" | "cancelled";
}

export function isDesktopShell(): boolean {
  if (typeof window === "undefined") return false;
  if (isTauri() || hasTauriInvokeBridge()) return true;
  try {
    if (sessionStorage.getItem(DESKTOP_SESSION_KEY) === "1") return true;
    const params = new URLSearchParams(window.location.search);
    if (params.get(DESKTOP_QUERY_KEY) === "1") {
      sessionStorage.setItem(DESKTOP_SESSION_KEY, "1");
      return true;
    }
  } catch {}
  return false;
}

export async function saveDesktopFile(
  blob: Blob,
  suggestedName: string,
): Promise<DesktopSaveResult | null> {
  if (!isDesktopShell()) return null;
  return invoke<DesktopSaveResult>("save_desktop_file", {
    request: {
      suggestedName,
      base64: await blobToBase64(blob),
      mime: blob.type || "application/octet-stream",
    },
  });
}

export async function openDesktopExternal(url: string): Promise<boolean> {
  if (!isDesktopShell()) return false;
  await invoke<void>("open_external_link", { url });
  return true;
}

export async function syncDesktopLocale(locale: string): Promise<boolean> {
  if (!isDesktopShell()) return false;
  await invoke<void>("set_desktop_locale", { locale });
  return true;
}

export async function openLlmDebugCaptureDirectory(
  path: string,
): Promise<boolean> {
  if (!isDesktopShell()) return false;
  await invoke<void>("open_llm_debug_capture_directory", { path });
  return true;
}

function hasTauriInvokeBridge(): boolean {
  return (
    typeof (
      window as Window & {
        __TAURI_INTERNALS__?: { invoke?: unknown };
      }
    ).__TAURI_INTERNALS__?.invoke === "function"
  );
}

async function blobToBase64(blob: Blob): Promise<string> {
  const bytes = new Uint8Array(await blob.arrayBuffer());
  let binary = "";
  const chunkSize = 0x8000;
  for (let offset = 0; offset < bytes.length; offset += chunkSize) {
    binary += String.fromCharCode.apply(
      null,
      Array.from(bytes.subarray(offset, offset + chunkSize)),
    );
  }
  return btoa(binary);
}

export function canOpenProjectDirectory(): boolean {
  if (typeof window === "undefined" || !(isTauri() || hasTauriInvokeBridge()))
    return false;
  const host = new URL(getApiUrl("/"), window.location.href).hostname;
  return ["127.0.0.1", "localhost", "[::1]"].includes(host);
}
export async function openProjectDirectory(path: string): Promise<void> {
  await invoke<void>("open_project_resource_directory", { path });
}
