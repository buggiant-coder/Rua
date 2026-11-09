import { beforeEach, describe, expect, it, vi } from "vitest";

const tauriMock = vi.hoisted(() => ({
  invoke: vi.fn(),
  isTauri: vi.fn(() => false),
}));

vi.mock("@tauri-apps/api/core", () => tauriMock);

import {
  isDesktopShell,
  openDesktopExternal,
  saveDesktopFile,
  syncDesktopLocale,
} from "./runtime";

describe("desktop runtime bridge", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    tauriMock.isTauri.mockReturnValue(false);
    sessionStorage.clear();
    history.replaceState({}, "", "/");
    delete (
      window as Window & {
        __TAURI_INTERNALS__?: unknown;
      }
    ).__TAURI_INTERNALS__;
  });

  it("detects Tauri and the persisted desktop navigation marker", () => {
    tauriMock.isTauri.mockReturnValue(true);
    expect(isDesktopShell()).toBe(true);

    tauriMock.isTauri.mockReturnValue(false);
    history.replaceState({}, "", "/console/?desktop=1");
    expect(isDesktopShell()).toBe(true);
    history.replaceState({}, "", "/console/");
    expect(isDesktopShell()).toBe(true);
  });

  it("invokes the user-mediated native Save command with exact bytes", async () => {
    tauriMock.isTauri.mockReturnValue(true);
    tauriMock.invoke.mockResolvedValue({ status: "saved" });

    await expect(
      saveDesktopFile(new Blob(["hello"], { type: "text/plain" }), "notes.txt"),
    ).resolves.toEqual({ status: "saved" });

    expect(tauriMock.invoke).toHaveBeenCalledWith("save_desktop_file", {
      request: {
        suggestedName: "notes.txt",
        base64: "aGVsbG8=",
        mime: "text/plain",
      },
    });
  });

  it("uses the named external-link command only inside Desktop Shell", async () => {
    await expect(openDesktopExternal("https://rua.example")).resolves.toBe(
      false,
    );
    expect(tauriMock.invoke).not.toHaveBeenCalled();

    tauriMock.isTauri.mockReturnValue(true);
    tauriMock.invoke.mockResolvedValue(undefined);
    await expect(openDesktopExternal("https://rua.example")).resolves.toBe(
      true,
    );
    expect(tauriMock.invoke).toHaveBeenCalledWith("open_external_link", {
      url: "https://rua.example",
    });
  });

  it("syncs the Console language to the native Desktop Shell", async () => {
    await expect(syncDesktopLocale("zh-CN")).resolves.toBe(false);
    expect(tauriMock.invoke).not.toHaveBeenCalled();

    tauriMock.isTauri.mockReturnValue(true);
    tauriMock.invoke.mockResolvedValue(undefined);

    await expect(syncDesktopLocale("ja-JP")).resolves.toBe(true);
    expect(tauriMock.invoke).toHaveBeenCalledWith("set_desktop_locale", {
      locale: "ja-JP",
    });
  });
});
