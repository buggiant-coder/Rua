import { useEffect } from "react";

import { startUserNotificationRuntime } from "../userNotifications/runtime";

export function useUserNotifications(): void {
  useEffect(() => startUserNotificationRuntime(), []);
}
