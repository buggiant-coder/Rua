import { redirect } from "react-router-dom";
import { authApi } from "@/api/modules/auth";
import { clearAuthToken, getApiToken, getApiUrl } from "@/api/config";

type RouteAccess = "ok" | "auth-required";

let cachedKey: string | null = null;
let cachedAccess: Promise<RouteAccess> | null = null;

export function verifyRouteAccess(): Promise<RouteAccess> {
  const token = getApiToken();
  const key = token ?? "";
  if (cachedAccess && cachedKey === key) return cachedAccess;

  cachedKey = key;
  const access = (async () => {
    const status = await authApi.getStatus();
    if (!status.enabled) return "ok";
    if (!token) return "auth-required";

    const response = await fetch(getApiUrl("/auth/verify"), {
      headers: { Authorization: `Bearer ${token}` },
    });
    if (response.ok) return "ok";
    if (response.status !== 401 && response.status !== 403) {
      throw new Error("Failed to verify auth token");
    }

    clearAuthToken();
    return "auth-required";
  })();
  cachedAccess = access;
  void access.catch(() => {
    if (cachedAccess === access) {
      cachedKey = null;
      cachedAccess = null;
    }
  });
  return access;
}

export async function requireRouteAccess(request: Request) {
  if ((await verifyRouteAccess()) === "ok") return;
  const url = new URL(request.url);
  const pathname = url.pathname.replace(/^\/console(?=\/|$)/, "") || "/";
  const hash =
    url.hash ||
    (url.pathname === window.location.pathname &&
    url.search === window.location.search
      ? window.location.hash
      : "");
  throw redirect(
    `/login?redirect=${encodeURIComponent(pathname + url.search + hash)}`,
  );
}

export function __resetRouteAccessForTests() {
  cachedKey = null;
  cachedAccess = null;
}
