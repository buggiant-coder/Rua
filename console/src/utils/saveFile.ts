import { isMobile } from "./isMobile";
import { isDesktopShell, saveDesktopFile } from "../desktop/runtime";

interface MobileBridge {
  saveBlobWithPicker?(
    requestId: string,
    name: string,
    base64: string,
    mime: string,
  ): string;
  saveBlob(name: string, base64: string, mime: string): string;
}

declare global {
  interface Window {
    MobileNative?: MobileBridge;
    __mobileSaveBlobResult?: (
      requestId: string,
      status: "ok" | "cancelled" | "error",
      message: string,
    ) => void;
  }
}

export interface SaveFileResult {
  ok: boolean;

  usedPicker: boolean;

  cancelled?: boolean;
}

interface PickerType {
  description: string;
  accept: Record<string, string[]>;
}

interface FileSystemWritableFileStreamLike {
  write(data: Blob): Promise<void>;
  close(): Promise<void>;
}

interface FileSystemFileHandleLike {
  createWritable(): Promise<FileSystemWritableFileStreamLike>;
}

interface SavePickerWindow extends Window {
  showSaveFilePicker?: (options: {
    suggestedName: string;
    types: PickerType[];
  }) => Promise<FileSystemFileHandleLike>;
}

interface NativeSaveResult {
  ok: boolean;
  usedPicker: boolean;
  cancelled?: boolean;
}

type MobileSaveCallback = (result: {
  status: "ok" | "cancelled" | "error";
  message: string;
}) => void;

const mobileSaveCallbacks = new Map<string, MobileSaveCallback>();
let mobileResultHandlerInstalled = false;

function makeRequestId(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return crypto.randomUUID();
  }
  return `save-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

function ensureMobileResultHandler(): void {
  if (mobileResultHandlerInstalled) return;
  const previous = window.__mobileSaveBlobResult;
  window.__mobileSaveBlobResult = (requestId, status, message) => {
    const callback = mobileSaveCallbacks.get(requestId);
    if (callback) {
      mobileSaveCallbacks.delete(requestId);
      callback({ status, message });
      return;
    }
    previous?.(requestId, status, message);
  };
  mobileResultHandlerInstalled = true;
}

async function saveWithPicker(
  blob: Blob,
  suggestedName: string,
  types: PickerType[],
): Promise<SaveFileResult | null> {
  const picker = (window as SavePickerWindow).showSaveFilePicker;
  if (!picker) return null;
  try {
    const handle = await picker({
      suggestedName,
      types,
    });
    const writable = await handle.createWritable();
    await writable.write(blob);
    await writable.close();
    return { ok: true, usedPicker: true };
  } catch (e: unknown) {
    if (e instanceof DOMException && e.name === "AbortError") {
      return { ok: true, usedPicker: true, cancelled: true };
    }
    return null;
  }
}

async function blobToBase64(blob: Blob): Promise<string> {
  const buf = await blob.arrayBuffer();
  const bytes = new Uint8Array(buf);

  let binary = "";
  const CHUNK = 0x8000;
  for (let i = 0; i < bytes.length; i += CHUNK) {
    binary += String.fromCharCode.apply(
      null,
      Array.from(bytes.subarray(i, i + CHUNK)),
    );
  }
  return btoa(binary);
}

async function saveWithMobileBridge(
  blob: Blob,
  suggestedName: string,
): Promise<NativeSaveResult | null> {
  const bridge = window.MobileNative;
  if (!bridge) return null;
  try {
    const base64 = await blobToBase64(blob);
    const mime = blob.type || "application/octet-stream";
    if (bridge.saveBlobWithPicker) {
      ensureMobileResultHandler();
      const requestId = makeRequestId();
      const resultPromise = new Promise<NativeSaveResult>((resolve) => {
        mobileSaveCallbacks.set(requestId, ({ status, message }) => {
          if (status === "ok") {
            resolve({ ok: true, usedPicker: true });
          } else if (status === "cancelled") {
            resolve({ ok: true, usedPicker: true, cancelled: true });
          } else {
            console.error("MobileNative.saveBlobWithPicker:", message);
            resolve({ ok: false, usedPicker: true });
          }
        });
      });
      const started = bridge.saveBlobWithPicker(
        requestId,
        suggestedName,
        base64,
        mime,
      );
      if (typeof started === "string" && started.startsWith("ERROR:")) {
        mobileSaveCallbacks.delete(requestId);
        console.error("MobileNative.saveBlobWithPicker:", started);
        return { ok: false, usedPicker: true };
      }
      return resultPromise;
    }
    if (!bridge.saveBlob) return null;
    const result = bridge.saveBlob(suggestedName, base64, mime);
    if (typeof result === "string" && result.startsWith("ERROR:")) {
      console.error("MobileNative.saveBlob:", result);
      return { ok: false, usedPicker: false };
    }
    return { ok: true, usedPicker: false };
  } catch (e) {
    console.error("MobileNative.saveBlob threw:", e);
    return { ok: false, usedPicker: false };
  }
}

function saveWithLink(blob: Blob, suggestedName: string): void {
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = suggestedName;
  a.rel = "noopener";
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);

  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

function inferTypesFromName(name: string): PickerType[] {
  const ext = name.slice(name.lastIndexOf(".")).toLowerCase();
  switch (ext) {
    case ".md":
      return [
        { description: "Markdown", accept: { "text/markdown": [".md"] } },
      ];
    case ".json":
      return [
        { description: "JSON", accept: { "application/json": [".json"] } },
      ];
    case ".txt":
      return [{ description: "Text", accept: { "text/plain": [".txt"] } }];
    default:
      return [
        {
          description: "File",
          accept: { "application/octet-stream": [ext || ""] },
        },
      ];
  }
}

export async function saveFile(
  blob: Blob,
  suggestedName: string,
  types?: PickerType[],
): Promise<SaveFileResult> {
  if (isMobile()) {
    const result = await saveWithMobileBridge(blob, suggestedName);
    if (result) return result;
  }

  if (isDesktopShell()) {
    try {
      const result = await saveDesktopFile(blob, suggestedName);
      if (result?.status === "saved") return { ok: true, usedPicker: true };
      if (result?.status === "cancelled") {
        return { ok: true, usedPicker: true, cancelled: true };
      }
    } catch (error) {
      console.error("save_desktop_file:", error);
      return { ok: false, usedPicker: true };
    }
  }

  if (!isMobile() && "showSaveFilePicker" in window) {
    const result = await saveWithPicker(
      blob,
      suggestedName,
      types ?? inferTypesFromName(suggestedName),
    );
    if (result) return result;
  }

  saveWithLink(blob, suggestedName);
  return { ok: true, usedPicker: false };
}

export async function saveTextFile(
  content: string,
  suggestedName: string,
  mime = "text/plain;charset=utf-8",
): Promise<SaveFileResult> {
  const blob = new Blob([content], { type: mime });
  return saveFile(blob, suggestedName);
}

export function getDownloadFolderHint(): string {
  if (isMobile()) return "/sdcard/Download/rua/";
  return "";
}
