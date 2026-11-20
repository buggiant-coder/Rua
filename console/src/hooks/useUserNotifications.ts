import { useEffect } from "react";

import { startUserNotificationRuntime } from "../userNotifications/runtime";

export function useUserNotifications(
  subscribeNavigation: (announce: () => void) => () => void,
): void {
  useEffect(
    () => startUserNotificationRuntime(subscribeNavigation),
    [subscribeNavigation],
  );
}
