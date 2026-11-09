interface MobileRuntimeBridge {
  openEnvironment(): void;
  openPermissions(): void;
}

declare global {
  interface Window {
    MobileRuntime?: MobileRuntimeBridge;
  }
}

export function androidRuntimeActions(): MobileRuntimeBridge | undefined {
  return window.MobileRuntime;
}
