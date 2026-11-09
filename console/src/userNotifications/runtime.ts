import { isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import { getApiUrl } from "../api/config";
import { selectUserNotificationAdapter } from "./adapters";
import { startChatViewerCoordinator } from "./chatViewer";
import { activateRuaUri } from "./ruaUri";
import type { UserNotificationRequest } from "./types";

export const USER_NOTIFICATION_ACTIVATED_EVENT =
  "rua:user-notification-activated";

interface UserNotificationActivationEvent extends Event {
  detail?: {
    uri?: unknown;
  };
}

export function startUserNotificationRuntime(): () => void {
  const adapter = selectUserNotificationAdapter();
  const chatViewer = startChatViewerCoordinator();
  const source = new EventSource(
    getApiUrl("/platform/user-notifications/events"),
  );
  source.addEventListener("user_notification", (event) => {
    try {
      const request = JSON.parse(
        (event as MessageEvent<string>).data,
      ) as UserNotificationRequest;
      void chatViewer.shouldPresent(request).then((shouldPresent) => {
        if (shouldPresent) void adapter.present(request);
      });
    } catch {}
  });

  const onAndroidActivation = (event: Event) => {
    const uri = (event as UserNotificationActivationEvent).detail?.uri;
    if (typeof uri === "string") activateRuaUri(uri);
  };
  window.addEventListener(
    USER_NOTIFICATION_ACTIVATED_EVENT,
    onAndroidActivation,
  );

  let unlistenPromise: Promise<UnlistenFn> | undefined;
  if (isTauri()) {
    unlistenPromise = listen<string>(
      USER_NOTIFICATION_ACTIVATED_EVENT,
      (event) => {
        if (typeof event.payload === "string") {
          activateRuaUri(event.payload);
        }
      },
    );
  }

  return () => {
    chatViewer.stop();
    source.close();
    window.removeEventListener(
      USER_NOTIFICATION_ACTIVATED_EVENT,
      onAndroidActivation,
    );
    void unlistenPromise?.then((unlisten) => unlisten()).catch(() => {});
  };
}
