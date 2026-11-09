function hasUnsafeCharacters(value: string): boolean {
  return (
    value.trim() !== value ||
    Array.from(value).some((char) => {
      const code = char.charCodeAt(0);
      return code < 0x20 || code === 0x7f;
    })
  );
}

export function consolePathFromRuaUri(
  rawUri: string,
  currentPathname = window.location.pathname,
): string | null {
  if (!rawUri || hasUnsafeCharacters(rawUri)) return null;

  let uri: URL;
  try {
    uri = new URL(rawUri);
  } catch {
    return null;
  }
  if (
    uri.protocol !== "rua:" ||
    !uri.hostname ||
    uri.username ||
    uri.password ||
    uri.port
  ) {
    return null;
  }

  const route = `/${uri.hostname}${uri.pathname}`.replace(/\/{2,}/g, "/");
  const basename = /^\/console(?:\/|$)/.test(currentPathname) ? "/console" : "";
  return `${basename}${route}${uri.search}${uri.hash}`;
}

export function activateRuaUri(rawUri: string): boolean {
  const path = consolePathFromRuaUri(rawUri);
  if (!path) return false;
  window.focus();
  window.history.pushState({}, "", path);
  window.dispatchEvent(new PopStateEvent("popstate"));
  return true;
}
