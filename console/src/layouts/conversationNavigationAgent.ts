type ProjectNavigationAgent = {
  projectId: string;
  chatId: string;
  agentId: string;
};

let current: ProjectNavigationAgent | null = null;
const listeners = new Set<() => void>();

export const getProjectNavigationAgent = () => current;
export function subscribeProjectNavigationAgent(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** The visible Project selector supplies navigation context, not Agent ownership. */
export function registerProjectNavigationAgent(value: ProjectNavigationAgent) {
  current = value;
  for (const listener of listeners) listener();
  return () => {
    if (current !== value) return;
    current = null;
    for (const listener of listeners) listener();
  };
}
