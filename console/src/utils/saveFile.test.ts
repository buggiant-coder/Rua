import { beforeEach, describe, expect, it, vi } from "vitest";
import { getDownloadFolderHint, saveFile, saveTextFile } from "./saveFile";

const mobileMock = vi.hoisted(() => ({
  enabled: false,
}));
const desktopMock = vi.hoisted(() => ({
  enabled: false,
  save: vi.fn(),
}));

vi.mock("./isMobile", () => ({
  isMobile: () => mobileMock.enabled,
}));
vi.mock("../desktop/runtime", () => ({
  isDesktopShell: () => desktopMock.enabled,
  saveDesktopFile: desktopMock.save,
}));

type TestMobileBridge = {
  saveBlob: (name: string, base64: string, mime: string) => string;
  saveBlobWithPicker?: (
    requestId: string,
    name: string,
    base64: string,
    mime: string,
  ) => string;
};

type TestWindow = Window & {
  MobileNative?: TestMobileBridge;
  __mobileSaveBlobResult?: (
    requestId: string,
    status: "ok" | "cancelled" | "error",
    message: string,
  ) => void;
};

function testWindow(): TestWindow {
  return window as TestWindow;
}

describe("saveFile", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mobileMock.enabled = false;
    desktopMock.enabled = false;
    desktopMock.save.mockReset();
    delete testWindow().MobileNative;
    delete (window as { showSaveFilePicker?: unknown }).showSaveFilePicker;
    vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:download");
    vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => {});
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
  });

  it("uses the mobile native bridge before browser fallbacks", async () => {
    mobileMock.enabled = true;
    const saveBlob = vi.fn().mockReturnValue("OK");
    window.MobileNative = { saveBlob };

    await expect(saveTextFile("hello", "notes.txt")).resolves.toEqual({
      ok: true,
      usedPicker: false,
    });

    expect(saveBlob).toHaveBeenCalledWith(
      "notes.txt",
      "aGVsbG8=",
      "text/plain;charset=utf-8",
    );
    expect(URL.createObjectURL).not.toHaveBeenCalled();
    expect(getDownloadFolderHint()).toBe("/sdcard/Download/rua/");
  });

  it("uses the mobile native save picker bridge when available", async () => {
    mobileMock.enabled = true;
    const saveBlob = vi.fn().mockReturnValue("OK");
    const saveBlobWithPicker = vi.fn().mockReturnValue("OK");
    testWindow().MobileNative = { saveBlob, saveBlobWithPicker };

    const pending = saveTextFile("hello", "notes.txt");
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(saveBlobWithPicker).toHaveBeenCalledWith(
      expect.any(String),
      "notes.txt",
      "aGVsbG8=",
      "text/plain;charset=utf-8",
    );
    expect(saveBlob).not.toHaveBeenCalled();

    const requestId = saveBlobWithPicker.mock.calls[0][0] as string;
    testWindow().__mobileSaveBlobResult?.(requestId, "ok", "");

    await expect(pending).resolves.toEqual({
      ok: true,
      usedPicker: true,
    });
  });

  it("reports mobile picker bridge startup errors instead of pretending a blob link worked", async () => {
    mobileMock.enabled = true;
    const saveBlobWithPicker = vi.fn().mockReturnValue("ERROR:no_activity");
    testWindow().MobileNative = {
      saveBlob: vi.fn().mockReturnValue("OK"),
      saveBlobWithPicker,
    };

    await expect(saveTextFile("hello", "notes.txt")).resolves.toEqual({
      ok: false,
      usedPicker: true,
    });
    expect(URL.createObjectURL).not.toHaveBeenCalled();
  });

  it("uses the native save picker on desktop when available", async () => {
    const writable = {
      write: vi.fn().mockResolvedValue(undefined),
      close: vi.fn().mockResolvedValue(undefined),
    };
    (window as { showSaveFilePicker?: unknown }).showSaveFilePicker = vi
      .fn()
      .mockResolvedValue({
        createWritable: vi.fn().mockResolvedValue(writable),
      });
    const blob = new Blob(["{}"], { type: "application/json" });

    await expect(saveFile(blob, "data.json")).resolves.toEqual({
      ok: true,
      usedPicker: true,
    });

    expect(window.showSaveFilePicker).toHaveBeenCalledWith({
      suggestedName: "data.json",
      types: [
        {
          description: "JSON",
          accept: { "application/json": [".json"] },
        },
      ],
    });
    expect(writable.write).toHaveBeenCalledWith(blob);
    expect(writable.close).toHaveBeenCalledTimes(1);
    expect(URL.createObjectURL).not.toHaveBeenCalled();
  });

  it("uses the Desktop Shell command before browser picker fallbacks", async () => {
    desktopMock.enabled = true;
    desktopMock.save.mockResolvedValue({ status: "saved" });
    const browserPicker = vi.fn();
    (window as { showSaveFilePicker?: unknown }).showSaveFilePicker =
      browserPicker;

    await expect(saveTextFile("hello", "notes.txt")).resolves.toEqual({
      ok: true,
      usedPicker: true,
    });

    expect(desktopMock.save).toHaveBeenCalledWith(
      expect.any(Blob),
      "notes.txt",
    );
    expect(browserPicker).not.toHaveBeenCalled();
    expect(URL.createObjectURL).not.toHaveBeenCalled();
  });

  it("reports Desktop Shell cancellation separately from write failure", async () => {
    desktopMock.enabled = true;
    desktopMock.save.mockResolvedValueOnce({ status: "cancelled" });

    await expect(saveTextFile("cancel", "cancel.txt")).resolves.toEqual({
      ok: true,
      usedPicker: true,
      cancelled: true,
    });

    desktopMock.save.mockRejectedValueOnce(new Error("disk full"));
    await expect(saveTextFile("failure", "failure.txt")).resolves.toEqual({
      ok: false,
      usedPicker: true,
    });
    expect(URL.createObjectURL).not.toHaveBeenCalled();
  });

  it("treats save-picker cancellation as a handled user choice", async () => {
    (window as { showSaveFilePicker?: unknown }).showSaveFilePicker = vi
      .fn()
      .mockRejectedValue(new DOMException("cancelled", "AbortError"));

    await expect(saveTextFile("cancel", "cancel.md")).resolves.toEqual({
      ok: true,
      usedPicker: true,
      cancelled: true,
    });
    expect(URL.createObjectURL).not.toHaveBeenCalled();
  });

  it("falls back to a download link when picker or bridge cannot save", async () => {
    (window as { showSaveFilePicker?: unknown }).showSaveFilePicker = vi
      .fn()
      .mockRejectedValue(new Error("security"));

    await expect(saveTextFile("fallback", "fallback.txt")).resolves.toEqual({
      ok: true,
      usedPicker: false,
    });

    expect(URL.createObjectURL).toHaveBeenCalled();
    expect(HTMLAnchorElement.prototype.click).toHaveBeenCalledTimes(1);
    expect(getDownloadFolderHint()).toBe("");
  });
});
