import { afterEach, describe, expect, it, vi } from "vitest";
import { isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import {
  startUserNotificationRuntime,
  USER_NOTIFICATION_ACTIVATED_EVENT,
} from "./runtime";

vi.mock("../api/config", () => ({
  getApiUrl: (path: string) => `/api${path}`,
}));
vi.mock("@tauri-apps/api/core", () => ({ isTauri: vi.fn(), invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

describe("native notification activation", () => {
  afterEach(() => {
    delete window.RuaNotificationNative;
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
    vi.clearAllMocks();
    history.replaceState({}, "", "/");
  });

  function start() {
    vi.stubGlobal(
      "EventSource",
      class extends EventTarget {
        close() {}
      },
    );
    vi.spyOn(window, "focus").mockImplementation(() => {});
    history.replaceState({}, "", "/console/settings");
    return startUserNotificationRuntime(() => () => {});
  }

  it("opens the pending Android click after startup and handles later clicks once", () => {
    window.RuaNotificationNative = {
      capability: () => "user-notification-v1",
      showUserNotification: () => true,
      takePendingActivation: vi
        .fn()
        .mockReturnValueOnce("rua://chats/first?turn=1#done")
        .mockReturnValueOnce("rua://chats/second?turn=2&from=mobile#result")
        .mockReturnValue(null),
    };
    const stop = start();
    try {
      expect(location.pathname + location.search + location.hash).toBe(
        "/console/chats/first?turn=1#done",
      );
      window.dispatchEvent(new Event(USER_NOTIFICATION_ACTIVATED_EVENT));
      window.dispatchEvent(new Event(USER_NOTIFICATION_ACTIVATED_EVENT));
      expect(location.pathname + location.search + location.hash).toBe(
        "/console/chats/second?turn=2&from=mobile#result",
      );
      expect(window.focus).toHaveBeenCalledTimes(2);
    } finally {
      stop();
    }
  });

  it("opens a click delivered to the main Tauri window", () => {
    vi.mocked(isTauri).mockReturnValue(true);
    vi.mocked(listen).mockResolvedValue(() => {});
    const stop = start();
    try {
      const [name, activate] = vi.mocked(listen).mock.calls[0];
      expect(name).toBe(USER_NOTIFICATION_ACTIVATED_EVENT);
      activate({
        event: name,
        id: 1,
        payload: "rua://chats/desktop?turn=t&from=desktop#result",
      });
      expect(location.pathname + location.search + location.hash).toBe(
        "/console/chats/desktop?turn=t&from=desktop#result",
      );
    } finally {
      stop();
    }
  });
});
