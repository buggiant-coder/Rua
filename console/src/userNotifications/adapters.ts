import { invoke, isTauri } from "@tauri-apps/api/core";
import {
  isPermissionGranted,
  requestPermission,
} from "@tauri-apps/plugin-notification";

import { activateRuaUri } from "./ruaUri";
import type {
  AndroidUserNotificationBridge,
  UserNotificationRequest,
} from "./types";

export const ANDROID_NOTIFICATION_CAPABILITY = "user-notification-v1";

export type UserNotificationAdapterKind = "android" | "tauri" | "browser";

export interface UserNotificationAdapter {
  readonly kind: UserNotificationAdapterKind;
  present(request: UserNotificationRequest): Promise<void>;
}

interface NotificationEnvironment {
  androidBridge?: AndroidUserNotificationBridge;
  isTauri: () => boolean;
  tauriPermissionGranted: () => Promise<boolean>;
  requestTauriPermission: () => Promise<NotificationPermission>;
  showTauriNotification: (request: UserNotificationRequest) => Promise<void>;
  browserNotification?: typeof Notification;
  activateUri: (uri: string) => boolean;
  focusWindow: () => void;
}

function defaultEnvironment(): NotificationEnvironment {
  return {
    androidBridge: window.RuaNotificationNative,
    isTauri,
    tauriPermissionGranted: isPermissionGranted,
    requestTauriPermission: requestPermission,
    showTauriNotification: (request) =>
      invoke<void>("show_user_notification", { request }),
    browserNotification:
      typeof window.Notification === "function"
        ? window.Notification
        : undefined,
    activateUri: activateRuaUri,
    focusWindow: () => window.focus(),
  };
}

export function selectUserNotificationAdapter(
  environment: NotificationEnvironment = defaultEnvironment(),
): UserNotificationAdapter {
  const bridge = environment.androidBridge;
  if (bridge?.capability() === ANDROID_NOTIFICATION_CAPABILITY) {
    return {
      kind: "android",
      async present(request) {
        try {
          bridge.showUserNotification(JSON.stringify(request));
        } catch {}
      },
    };
  }

  if (environment.isTauri()) {
    return {
      kind: "tauri",
      async present(request) {
        try {
          const granted =
            (await environment.tauriPermissionGranted()) ||
            (await environment.requestTauriPermission()) === "granted";
          if (!granted) return;
          await environment.showTauriNotification(request);
        } catch {}
      },
    };
  }

  return {
    kind: "browser",
    async present(request) {
      const NotificationApi = environment.browserNotification;
      if (!NotificationApi || NotificationApi.permission !== "granted") return;
      try {
        const notification = new NotificationApi(request.title, {
          body: request.body
            ? `${request.sender.name}\n${request.body}`
            : request.sender.name,
        });
        notification.onclick = () => {
          environment.focusWindow();
          if (request.rua_uri) environment.activateUri(request.rua_uri);
          notification.close();
        };
      } catch {}
    },
  };
}
