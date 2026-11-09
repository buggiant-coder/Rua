const LAST_PROJECT_KEY = "rua_project_last_project";

export function lastProjectId(): string | null {
  try {
    return localStorage.getItem(LAST_PROJECT_KEY)?.trim() || null;
  } catch {
    return null;
  }
}

export function rememberProjectId(projectId: string): void {
  try {
    localStorage.setItem(LAST_PROJECT_KEY, projectId);
  } catch {
    // Browser privacy modes can disable localStorage; selection still works in-memory.
  }
}

export function forgetRememberedProject(projectId: string): void {
  try {
    if (localStorage.getItem(LAST_PROJECT_KEY) === projectId) {
      localStorage.removeItem(LAST_PROJECT_KEY);
    }
  } catch {
    // Browser privacy modes can disable localStorage; there is nothing to clear.
  }
}
