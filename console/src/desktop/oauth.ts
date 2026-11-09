import { normalizeExternalUrl } from "../utils/openExternal";
import { isDesktopShell, openDesktopExternal } from "./runtime";

export interface OAuthAuthorizationLaunch {
  mode: "external" | "popup";
  popup: Window | null;
}

export async function openOAuthAuthorization(
  url: string,
  name: string,
  features: string,
): Promise<OAuthAuthorizationLaunch> {
  const normalized = normalizeExternalUrl(url);
  if (!normalized) {
    throw new Error("OAuth authorization URL is invalid");
  }
  if (isDesktopShell()) {
    await openDesktopExternal(normalized);
    return { mode: "external", popup: null };
  }
  return {
    mode: "popup",
    popup: window.open(normalized, name, features),
  };
}
