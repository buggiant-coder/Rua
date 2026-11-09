const VIEWER_ID_KEY = "rua-terminal-viewer-id";

export function getBrowserViewerId() {
  const existing = window.localStorage.getItem(VIEWER_ID_KEY);
  if (existing) return existing;
  const created = crypto.randomUUID();
  window.localStorage.setItem(VIEWER_ID_KEY, created);
  return created;
}
