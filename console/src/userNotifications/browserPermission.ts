import { isTauri } from "@tauri-apps/api/core";

interface BrowserNotificationPermissionApi {
  permission: NotificationPermission;
  requestPermission(): Promise<NotificationPermission>;
}

interface BrowserPermissionEnvironment {
  isSecureContext: boolean;
  notification?: BrowserNotificationPermissionApi;
  isNativeShell: boolean;
}

function defaultEnvironment(): BrowserPermissionEnvironment {
  return {
    isSecureContext: window.isSecureContext,
    notification: window.Notification,
    isNativeShell: Boolean(window.RuaNotificationNative) || isTauri(),
  };
}

export function requestBrowserNotificationPermissionFromUserGesture(
  environment: BrowserPermissionEnvironment = defaultEnvironment(),
): void {
  if (
    environment.isNativeShell ||
    !environment.isSecureContext ||
    !environment.notification ||
    environment.notification.permission !== "default"
  ) {
    return;
  }
  void environment.notification.requestPermission().catch(() => {});
}
