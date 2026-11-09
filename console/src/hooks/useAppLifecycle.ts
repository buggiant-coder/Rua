import { useEffect } from "react";
import { getApiUrl } from "../api/config";
import { buildAuthHeaders } from "../api/authHeaders";

export function useAppLifecycle(): void {
  useEffect(() => {
    const handler = () => {
      if (document.visibilityState === "visible") {
        fetch(getApiUrl("/platform/lifecycle"), {
          method: "POST",
          headers: {
            "Content-Type": "application/json",
            ...buildAuthHeaders(),
          },
          body: JSON.stringify({ event: "foreground", scope: "document" }),
        }).catch(() => {});
      }
    };
    document.addEventListener("visibilitychange", handler);
    handler();
    return () => document.removeEventListener("visibilitychange", handler);
  }, []);
}
