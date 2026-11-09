interface MobileStorageBridge {
  hasAllFilesAccess(): boolean;
  requestAllFilesAccess(): void;
}

declare global {
  interface Window {
    MobileStorage?: MobileStorageBridge;
  }
}

export function isAndroidExternalPath(path: string): boolean {
  return ["/storage", "/sdcard", "/mnt/media_rw"].some(
    (root) => path === root || path.startsWith(`${root}/`),
  );
}

export function needsLocalStoragePermission(
  path: string,
  isSelf: boolean,
): boolean {
  return (
    isSelf &&
    isAndroidExternalPath(path) &&
    !!window.MobileStorage &&
    !window.MobileStorage.hasAllFilesAccess()
  );
}

export class StoragePermissionRequired extends Error {
  constructor() {
    super("Android storage permission required");
  }
}
