import type { UserNotificationRequest } from "./types";

const CHAT_ADDRESS_PATTERN = /^(agents|projects)\/([^/]+)\/chats\/([^/]+)$/;
const VIEWER_CHANNEL = "rua:chat-viewer-v1";
const VIEWER_HEARTBEAT_MS = 1_000;
const VIEWER_LEASE_MS = 3_500;

interface ChatViewerMessage {
  kind: "viewer";
  windowId: string;
  chatAddress?: string;
  expiresAt: number;
}

interface PresentationContext {
  currentPathname: string;
  windowActive: boolean;
  remotelyViewedChats: ReadonlySet<string>;
}

function normalizedChatAddress(value: string): string | null {
  const normalized = value.replace(/^\/+|\/+$/g, "");
  return CHAT_ADDRESS_PATTERN.test(normalized) ? normalized : null;
}

export function chatAddressFromRuaUri(rawUri?: string): string | null {
  if (!rawUri) return null;
  try {
    const uri = new URL(rawUri);
    if (uri.protocol !== "rua:" || uri.username || uri.password || uri.port) {
      return null;
    }
    return normalizedChatAddress(`${uri.hostname}${uri.pathname}`);
  } catch {
    return null;
  }
}

export function chatAddressFromConsolePath(pathname: string): string | null {
  const withoutBasename = pathname.replace(/^\/console(?=\/|$)/, "");
  return normalizedChatAddress(withoutBasename);
}

export function shouldPresentChatNotification(
  request: UserNotificationRequest,
  context: PresentationContext,
): boolean {
  const target = chatAddressFromRuaUri(request.rua_uri);
  if (!target) return true;
  if (context.remotelyViewedChats.has(target)) return false;
  return !(
    context.windowActive &&
    chatAddressFromConsolePath(context.currentPathname) === target
  );
}

export interface ChatViewerCoordinator {
  shouldPresent(request: UserNotificationRequest): Promise<boolean>;
  stop(): void;
}

export function startChatViewerCoordinator(): ChatViewerCoordinator {
  const windowId = crypto.randomUUID();
  const remoteViewers = new Map<string, ChatViewerMessage>();
  const channel =
    typeof BroadcastChannel === "undefined"
      ? undefined
      : new BroadcastChannel(VIEWER_CHANNEL);

  const currentViewedChat = () =>
    document.visibilityState === "visible" && document.hasFocus()
      ? chatAddressFromConsolePath(window.location.pathname)
      : null;

  const announce = () => {
    channel?.postMessage({
      kind: "viewer",
      windowId,
      chatAddress: currentViewedChat() ?? undefined,
      expiresAt: Date.now() + VIEWER_LEASE_MS,
    } satisfies ChatViewerMessage);
  };

  if (channel) {
    channel.onmessage = (event: MessageEvent<unknown>) => {
      const message = event.data as Partial<ChatViewerMessage>;
      if (
        message.kind !== "viewer" ||
        typeof message.windowId !== "string" ||
        message.windowId === windowId ||
        typeof message.expiresAt !== "number"
      ) {
        return;
      }
      if (message.chatAddress) {
        const chatAddress = normalizedChatAddress(message.chatAddress);
        if (!chatAddress) return;
        remoteViewers.set(message.windowId, {
          kind: "viewer",
          windowId: message.windowId,
          chatAddress,
          expiresAt: message.expiresAt,
        });
      } else {
        remoteViewers.delete(message.windowId);
      }
    };
  }

  const heartbeat = window.setInterval(announce, VIEWER_HEARTBEAT_MS);
  window.addEventListener("popstate", announce);
  window.addEventListener("focus", announce);
  window.addEventListener("blur", announce);
  document.addEventListener("visibilitychange", announce);
  announce();

  return {
    async shouldPresent(request) {
      const now = Date.now();
      const viewedChats = new Set<string>();
      for (const [remoteWindowId, viewer] of remoteViewers) {
        if (viewer.expiresAt <= now) {
          remoteViewers.delete(remoteWindowId);
        } else if (viewer.chatAddress) {
          viewedChats.add(viewer.chatAddress);
        }
      }
      const shouldPresent = shouldPresentChatNotification(request, {
        currentPathname: window.location.pathname,
        windowActive:
          document.visibilityState === "visible" && document.hasFocus(),
        remotelyViewedChats: viewedChats,
      });
      if (!shouldPresent || !request.rua_uri || !navigator.locks) {
        return shouldPresent;
      }
      const notificationKey = `rua:user-notification:${request.rua_uri}`;
      return navigator.locks.request(notificationKey, () => {
        const shownAt = Number(localStorage.getItem(notificationKey));
        if (Number.isFinite(shownAt) && Date.now() - shownAt < 60_000) {
          return false;
        }
        localStorage.setItem(notificationKey, String(Date.now()));
        return true;
      });
    },
    stop() {
      window.clearInterval(heartbeat);
      window.removeEventListener("popstate", announce);
      window.removeEventListener("focus", announce);
      window.removeEventListener("blur", announce);
      document.removeEventListener("visibilitychange", announce);
      channel?.postMessage({
        kind: "viewer",
        windowId,
        expiresAt: 0,
      } satisfies ChatViewerMessage);
      channel?.close();
    },
  };
}
