import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  ANDROID_NOTIFICATION_CAPABILITY,
  selectUserNotificationAdapter,
} from "./adapters";
import type { UserNotificationRequest } from "./types";

const request: UserNotificationRequest = {
  target_device_id: "device-a",
  sender: {
    id: "agent:researcher",
    name: "Research Agent",
  },
  title: "Run complete",
  body: "The requested analysis is ready.",
  rua_uri: "rua://agents/researcher/chats/chat-42?turn=7#result",
};

function browserNotificationMock() {
  const instances: Array<{
    title: string;
    options?: NotificationOptions;
    onclick: (() => void) | null;
    close: ReturnType<typeof vi.fn>;
  }> = [];
  class BrowserNotification {
    static permission: NotificationPermission = "granted";
    onclick: (() => void) | null = null;
    close = vi.fn();

    constructor(
      public title: string,
      public options?: NotificationOptions,
    ) {
      instances.push(this);
    }
  }
  return {
    NotificationApi: BrowserNotification as unknown as typeof Notification,
    instances,
  };
}

describe("user notification adapter selection", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("selects the Android bridge once and preserves the request JSON", async () => {
    const showUserNotification = vi.fn(() => true);
    const showTauriNotification = vi.fn();
    const { NotificationApi, instances } = browserNotificationMock();
    const adapter = selectUserNotificationAdapter({
      androidBridge: {
        capability: () => ANDROID_NOTIFICATION_CAPABILITY,
        showUserNotification,
      },
      isTauri: () => true,
      tauriPermissionGranted: async () => true,
      requestTauriPermission: async () => "denied",
      showTauriNotification,
      browserNotification: NotificationApi,
      activateUri: vi.fn(),
      focusWindow: vi.fn(),
    });

    await adapter.present(request);

    expect(adapter.kind).toBe("android");
    expect(showUserNotification).toHaveBeenCalledTimes(1);
    expect(JSON.parse(showUserNotification.mock.calls[0][0])).toEqual(request);
    expect(showTauriNotification).not.toHaveBeenCalled();
    expect(instances).toHaveLength(0);
  });

  it("selects Tauri without invoking the browser adapter", async () => {
    const showTauriNotification = vi.fn(async () => {});
    const { NotificationApi, instances } = browserNotificationMock();
    const adapter = selectUserNotificationAdapter({
      isTauri: () => true,
      tauriPermissionGranted: async () => true,
      requestTauriPermission: async () => "denied",
      showTauriNotification,
      browserNotification: NotificationApi,
      activateUri: vi.fn(),
      focusWindow: vi.fn(),
    });

    await adapter.present(request);

    expect(adapter.kind).toBe("tauri");
    expect(showTauriNotification).toHaveBeenCalledTimes(1);
    expect(showTauriNotification).toHaveBeenCalledWith(request);
    expect(instances).toHaveLength(0);
  });

  it("silently misses when Tauri permission is unavailable", async () => {
    const showTauriNotification = vi.fn();
    const requestTauriPermission = vi.fn(async () => "denied" as const);
    const adapter = selectUserNotificationAdapter({
      isTauri: () => true,
      tauriPermissionGranted: async () => false,
      requestTauriPermission,
      showTauriNotification,
      activateUri: vi.fn(),
      focusWindow: vi.fn(),
    });

    await expect(adapter.present(request)).resolves.toBeUndefined();
    expect(requestTauriPermission).toHaveBeenCalledTimes(1);
    expect(showTauriNotification).not.toHaveBeenCalled();
  });

  it("uses the browser only when no native capability exists", async () => {
    const activateUri = vi.fn(() => true);
    const focusWindow = vi.fn();
    const { NotificationApi, instances } = browserNotificationMock();
    const adapter = selectUserNotificationAdapter({
      isTauri: () => false,
      tauriPermissionGranted: async () => false,
      requestTauriPermission: async () => "denied",
      showTauriNotification: vi.fn(),
      browserNotification: NotificationApi,
      activateUri,
      focusWindow,
    });

    await adapter.present(request);

    expect(adapter.kind).toBe("browser");
    expect(instances).toHaveLength(1);
    expect(instances[0].title).toBe(request.title);
    expect(instances[0].options?.body).toBe(
      "Research Agent\nThe requested analysis is ready.",
    );
    instances[0].onclick?.();
    expect(focusWindow).toHaveBeenCalledTimes(1);
    expect(activateUri).toHaveBeenCalledWith(request.rua_uri);
    expect(instances[0].close).toHaveBeenCalledTimes(1);
  });

  it("does not infer a native adapter from the user agent", () => {
    Object.defineProperty(navigator, "userAgent", {
      value: "Mozilla/5.0 Mobile/9.9 Tauri",
      configurable: true,
    });
    const adapter = selectUserNotificationAdapter({
      isTauri: () => false,
      tauriPermissionGranted: async () => false,
      requestTauriPermission: async () => "denied",
      showTauriNotification: vi.fn(),
      activateUri: vi.fn(),
      focusWindow: vi.fn(),
    });

    expect(adapter.kind).toBe("browser");
  });

  it("does not prompt when browser permission is not already granted", async () => {
    const { NotificationApi, instances } = browserNotificationMock();
    Object.defineProperty(NotificationApi, "permission", {
      value: "default",
      configurable: true,
    });
    const adapter = selectUserNotificationAdapter({
      isTauri: () => false,
      tauriPermissionGranted: async () => false,
      requestTauriPermission: async () => "denied",
      showTauriNotification: vi.fn(),
      browserNotification: NotificationApi,
      activateUri: vi.fn(),
      focusWindow: vi.fn(),
    });

    await expect(adapter.present(request)).resolves.toBeUndefined();
    expect(instances).toHaveLength(0);
  });
});
