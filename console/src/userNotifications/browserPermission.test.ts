import { describe, expect, it, vi } from "vitest";

import { requestBrowserNotificationPermissionFromUserGesture } from "./browserPermission";

describe("browser notification permission warmup", () => {
  it("requests permission from a user gesture when permission is undecided", () => {
    const requestPermission = vi.fn(() => Promise.resolve("granted" as const));

    requestBrowserNotificationPermissionFromUserGesture({
      isSecureContext: true,
      notification: { permission: "default", requestPermission },
      isNativeShell: false,
    });

    expect(requestPermission).toHaveBeenCalledTimes(1);
  });

  it.each(["granted", "denied"] as const)(
    "does not re-request when permission is %s",
    (permission) => {
      const requestPermission = vi.fn();
      requestBrowserNotificationPermissionFromUserGesture({
        isSecureContext: true,
        notification: { permission, requestPermission },
        isNativeShell: false,
      });
      expect(requestPermission).not.toHaveBeenCalled();
    },
  );

  it("does not request permission outside a secure context", () => {
    const requestPermission = vi.fn();
    requestBrowserNotificationPermissionFromUserGesture({
      isSecureContext: false,
      notification: { permission: "default", requestPermission },
      isNativeShell: false,
    });
    expect(requestPermission).not.toHaveBeenCalled();
  });
});
