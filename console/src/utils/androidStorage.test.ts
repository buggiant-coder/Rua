import { afterEach, describe, expect, it, vi } from "vitest";
import {
  isAndroidExternalPath,
  needsLocalStoragePermission,
} from "./androidStorage";

afterEach(() => {
  delete window.MobileStorage;
});

describe("Android storage permission routing", () => {
  it("recognizes external paths without matching similarly named private folders", () => {
    expect(isAndroidExternalPath("/storage/emulated/0")).toBe(true);
    expect(isAndroidExternalPath("/sdcard/Download")).toBe(true);
    expect(isAndroidExternalPath("/storage-backup")).toBe(false);
    expect(
      isAndroidExternalPath("/data/data/com.rua.mobile/files/home"),
    ).toBe(false);
  });
  it("requires permission only for local external paths in the APK", () => {
    window.MobileStorage = {
      hasAllFilesAccess: vi.fn(() => false),
      requestAllFilesAccess: vi.fn(),
    };
    expect(needsLocalStoragePermission("/storage/emulated/0", true)).toBe(true);
    expect(needsLocalStoragePermission("/storage/emulated/0", false)).toBe(
      false,
    );
    expect(needsLocalStoragePermission("/data/private", true)).toBe(false);
    window.MobileStorage.hasAllFilesAccess = () => true;
    expect(needsLocalStoragePermission("/storage/emulated/0", true)).toBe(
      false,
    );
    delete window.MobileStorage;
    expect(needsLocalStoragePermission("/storage/emulated/0", true)).toBe(
      false,
    );
  });
});
