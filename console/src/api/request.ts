import { consoleFetch } from "./consoleTransport";
import { getApiUrl, clearAuthToken } from "./config";
import { buildAuthHeaders } from "./authHeaders";

function getErrorMessageFromBody(
  text: string,
  contentType: string,
): string | null {
  if (!text) {
    return null;
  }

  if (!contentType.includes("application/json")) {
    return text;
  }

  try {
    const payload = JSON.parse(text) as {
      detail?: unknown;
      message?: unknown;
      error?: unknown;
    };

    if (typeof payload.detail === "string" && payload.detail) {
      return payload.detail;
    }

    if (Array.isArray(payload.detail) && payload.detail.length > 0) {
      const parts = (payload.detail as Array<Record<string, unknown>>)
        .map((err) => {
          const loc = Array.isArray(err.loc)
            ? (err.loc as unknown[]).filter((p) => p !== "body").join(".")
            : "";
          const msg = typeof err.msg === "string" ? err.msg : String(err);
          return loc ? `${loc}: ${msg}` : msg;
        })
        .filter(Boolean);
      if (parts.length > 0) return parts.join("; ");
    }
    if (typeof payload.message === "string" && payload.message) {
      return payload.message;
    }
    if (typeof payload.error === "string" && payload.error) {
      return payload.error;
    }
  } catch {
    return text;
  }

  return text;
}

function buildHeaders(method?: string, extra?: HeadersInit): Headers {
  const headers = extra instanceof Headers ? extra : new Headers(extra);

  if (method && ["POST", "PUT", "PATCH"].includes(method.toUpperCase())) {
    if (!headers.has("Content-Type")) {
      headers.set("Content-Type", "application/json");
    }
  }

  for (const [key, value] of Object.entries(buildAuthHeaders())) {
    if (!headers.has(key)) {
      headers.set(key, value);
    }
  }

  return headers;
}

export async function request<T = unknown>(
  path: string,
  options: RequestInit = {},
): Promise<T> {
  const url = getApiUrl(path);
  const method = options.method || "GET";
  const headers = buildHeaders(method, options.headers);

  const remoteSnapshot =
    method === "GET" &&
    /\/(?:approvals\/snapshot|acp\/permissions)(?:\?|$)/.test(path);
  const response = await (remoteSnapshot
    ? consoleFetch(path, { ...options, headers })
    : fetch(url, {
        ...options,
        headers,
        cache: "no-store",
      }));

  if (!response.ok) {
    if (!response.ok) {
      if (response.status === 401) {
        clearAuthToken();
        if (window.location.pathname !== "/login") {
          window.location.href = "/login";
        }
        throw new Error("Not authenticated");
      }

      const text = await response.text().catch(() => "");
      const contentType = response.headers.get("content-type") || "";
      const errorMessage = getErrorMessageFromBody(text, contentType);

      const finalMessage = errorMessage
        ? `${errorMessage} - ${text}`
        : `Request failed: ${response.status} ${response.statusText}`;

      throw new Error(finalMessage);
    }

    const text = await response.text().catch(() => "");
    const contentType = response.headers.get("content-type") || "";
    const errorMessage = getErrorMessageFromBody(text, contentType);
    throw new Error(
      errorMessage ||
        `Request failed: ${response.status} ${response.statusText}`,
    );
  }

  if (response.status === 204) {
    return undefined as T;
  }

  const contentType = response.headers.get("content-type") || "";
  if (!contentType.includes("application/json")) {
    return (await response.text()) as unknown as T;
  }

  return (await response.json()) as T;
}
