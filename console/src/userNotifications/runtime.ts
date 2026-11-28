import { isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import { subscribeConsoleResponse } from "../api/consoleSubscription";
import { readSseChunks } from "../pages/Chat/owned/sse";
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
  // `EventSource` cannot attach the console's bearer credential, so the Runtime
  // would answer 401 and notifications would silently stop. The Console
  // Transport carries the credential and still delivers server-sent events.
  const subscription = subscribeConsoleResponse(
    "/platform/user-notifications/events",
    async (response, _reconnected, signal) => {
      for await (const chunk of readSseChunks(
        response.body?.pipeThrough(new TransformStream(), { signal }) ?? null,
      )) {
        if (signal.aborted) break;
        if (chunk.event !== "user_notification") continue;
        try {
          const request = JSON.parse(chunk.data) as UserNotificationRequest;
          void chatViewer.shouldPresent(request).then((shouldPresent) => {
            if (shouldPresent) void adapter.present(request);
          });
        } catch {}
      }
    },
  );

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
    subscription.stop();
    window.removeEventListener(
      USER_NOTIFICATION_ACTIVATED_EVENT,
      onAndroidActivation,
    );
    void unlistenPromise?.then((unlisten) => unlisten()).catch(() => {});
  };
}
